//! Newline-delimited JSON-RPC 2.0 over a child process's stdio.
//!
//! One `Rpc` owns one agent process (as its own process group, so tools and MCP
//! servers die with it). Responses resolve pending requests; notifications are
//! handled inline, in wire order; agent→client requests run on their own task
//! so a long permission prompt never blocks the stream.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

/// Longest stdout line accepted; a larger one is skipped rather than buffered.
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Read one `\n`-terminated line of at most `max` bytes. `Ok(None)` is EOF;
/// `Err(len)` inside reports a skipped oversized line. Invalid UTF-8 is
/// replaced rather than treated as a read error.
async fn read_line_capped<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Option<Result<String, usize>>> {
    let mut buf = Vec::new();
    let mut skipped = 0usize;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.is_empty() && skipped == 0 {
                return Ok(None);
            }
            break;
        }
        let (used, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if skipped > 0 || buf.len() + used > max {
            skipped += buf.len() + used;
            buf = Vec::new();
        } else {
            buf.extend_from_slice(&available[..used]);
        }
        reader.consume(used);
        if done {
            break;
        }
    }
    if skipped > 0 {
        return Ok(Some(Err(skipped)));
    }
    while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        buf.pop();
    }
    Ok(Some(Ok(String::from_utf8_lossy(&buf).into_owned())))
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("method not supported by del: {method}"))
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }
    /// ACP's `auth_required`.
    pub fn is_auth_required(&self) -> bool {
        self.code == -32000
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)?;
        if let Some(d) = self.data.as_ref().filter(|d| !d.is_null()) {
            write!(f, ": {d}")?;
        }
        Ok(())
    }
}

/// Receives everything the agent sends that is not a response.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    /// Called inline from the reader, in wire order. Must not block.
    fn notification(&self, method: &str, params: Value);
    /// Answer an agent→client request.
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError>;
    /// The agent's stdout closed. `exit` is the exit code if it had one.
    fn closed(&self, exit: Option<i32>, stderr_tail: String);
}

type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>;

pub struct Rpc {
    tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicI64,
    closed: AtomicBool,
    terminated: AtomicBool,
    child: tokio::sync::Mutex<Child>,
    pid: Option<u32>,
}

/// How a request ended without a response.
pub fn closed_error() -> RpcError {
    RpcError::new(-32099, "agent process exited")
}

impl Rpc {
    pub fn process_id(&self) -> Option<u32> {
        self.pid
    }

    pub fn spawn(
        program: &Path,
        args: &[String],
        env: &[(String, String)],
        cwd: &Path,
        handler: Arc<dyn Handler>,
    ) -> std::io::Result<Arc<Rpc>> {
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        del_agent::environment::prepare_provider_command(command.as_std_mut());
        let mut child = command.spawn()?;
        let pid = child.id();
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let rpc = Arc::new(Rpc {
            tx,
            pending: Mutex::default(),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
            terminated: AtomicBool::new(false),
            child: tokio::sync::Mutex::new(child),
            pid,
        });

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let tail = Arc::new(Mutex::new(String::new()));
        let t = tail.clone();
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr);
            while let Ok(Some(l)) = read_line_capped(&mut stderr, 64 * 1024).await {
                let Ok(l) = l else { continue };
                let mut t = t.lock().unwrap();
                if t.len() > 2000 {
                    t.clear();
                }
                t.push_str(&l);
                t.push('\n');
            }
        });

        let r = rpc.clone();
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            while let Ok(Some(line)) = read_line_capped(&mut stdout, MAX_LINE_BYTES).await {
                match line {
                    Ok(line) => r.dispatch(&line, &handler),
                    Err(len) => eprintln!("acp: skipped a {len}-byte agent line over the size limit"),
                }
            }
            r.closed.store(true, Ordering::SeqCst);
            let waiters: Vec<_> = r.pending.lock().unwrap().drain().collect();
            for (_, w) in waiters {
                let _ = w.send(Err(closed_error()));
            }
            let tail = tail.lock().unwrap().trim().to_owned();
            // Report the real exit code when the process has already gone; do
            // not wait for it here: a process can close stdout yet ignore
            // SIGTERM, and close() still needs to kill it.
            let exit = match r.child.try_lock() {
                Ok(mut child) => child.try_wait().ok().flatten().and_then(|s| s.code()),
                Err(_) => None,
            };
            handler.closed(exit, tail);
            r.kill(Duration::from_secs(2)).await;
        });
        Ok(rpc)
    }

    fn dispatch(self: &Arc<Self>, line: &str, handler: &Arc<dyn Handler>) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return; // agents may print stray non-JSON lines
        };
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").filter(|v| !v.is_null()).cloned();
        match (method, id) {
            (Some(m), None) => handler.notification(m, msg["params"].clone()),
            (Some(m), Some(id)) => {
                let (rpc, handler) = (self.clone(), handler.clone());
                let (m, params) = (m.to_owned(), msg["params"].clone());
                tokio::spawn(async move {
                    let reply = match handler.request(&m, params).await {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err(e) => json!({"jsonrpc":"2.0","id":id,"error":
                            {"code":e.code,"message":e.message,"data":e.data}}),
                    };
                    rpc.send_raw(reply.to_string());
                });
            }
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let Some(w) = self.pending.lock().unwrap().remove(&id) else {
                    return;
                };
                let res = match msg.get("error") {
                    Some(e) => Err(RpcError {
                        code: e["code"].as_i64().unwrap_or(-32603),
                        message: e["message"].as_str().unwrap_or("error").to_owned(),
                        data: e.get("data").cloned(),
                    }),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = w.send(res);
            }
            (None, None) => {}
        }
    }

    fn send_raw(&self, line: String) {
        let _ = self.tx.send(line);
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send_raw(json!({"jsonrpc":"2.0","method":method,"params":params}).to_string());
    }

    pub fn request(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> impl std::future::Future<Output = Result<Value, RpcError>> + Send + 'static + use<> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().unwrap();
        if self.closed.load(Ordering::SeqCst) {
            let _ = tx.send(Err(closed_error()));
        } else {
            pending.insert(id, tx);
        }
        drop(pending);
        self.send_raw(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string());
        let method = method.to_owned();
        let cleanup = RequestCleanup {
            rpc: self.clone(),
            id,
        };
        async move {
            let _cleanup = cleanup;
            let wait = async { rx.await.unwrap_or_else(|_| Err(closed_error())) };
            match timeout {
                Some(t) => match tokio::time::timeout(t, wait).await {
                    Ok(r) => r,
                    Err(_) => Err(RpcError::new(-32098, format!("`{method}` timed out"))),
                },
                None => wait.await,
            }
        }
    }

    /// Terminate the agent and everything it started.
    pub async fn kill(&self, grace: Duration) {
        let mut child = self.child.lock().await;
        if self.terminated.load(Ordering::SeqCst) {
            return;
        }
        match self.pid {
            Some(pid) => del_agent::procgroup::terminate_group(pid, &mut child, grace).await,
            None => {
                let _ = child.start_kill();
            }
        }
        self.terminated.store(true, Ordering::SeqCst);
    }

    pub fn kill_now(&self) {
        if let Some(pid) = self.pid.filter(|_| !self.terminated.load(Ordering::SeqCst)) {
            del_agent::procgroup::signal_group(pid, true);
        }
    }
}

struct RequestCleanup {
    rpc: Arc<Rpc>,
    id: i64,
}

impl Drop for RequestCleanup {
    fn drop(&mut self) {
        self.rpc.pending.lock().unwrap().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capped_lines_skip_oversized_and_tolerate_bad_utf8() {
        let data: &[u8] = b"ok\n0123456789abcdef\nbad\xff\n";
        let mut reader = BufReader::with_capacity(4, data);
        let mut got = Vec::new();
        while let Some(line) = read_line_capped(&mut reader, 10).await.unwrap() {
            got.push(line);
        }
        assert_eq!(
            got,
            vec![Ok("ok".to_owned()), Err(17), Ok("bad\u{fffd}".to_owned())]
        );
    }
}
