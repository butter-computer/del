//! Low-level async client for `codex app-server` over newline-delimited stdio.
//!
//! The wire format is JSON-RPC 2.0 with the `jsonrpc` member omitted. One
//! reader task classifies each line (response / server request / notification),
//! one writer task owns stdin so lines are never interleaved.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

/// JSON-RPC error code the server returns when its ingress queue is full.
pub const OVERLOADED_CODE: i64 = -32001;
/// Longest stderr tail kept for diagnostics.
const STDERR_TAIL_LINES: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::Int(i) => write!(f, "{i}"),
            RequestId::Str(s) => f.write_str(s),
        }
    }
}

impl RequestId {
    /// Inverse of `Display`, for ids round-tripped through opaque strings.
    pub fn parse(s: &str) -> RequestId {
        s.parse::<i64>()
            .map(RequestId::Int)
            .unwrap_or_else(|_| RequestId::Str(s.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("codex rejected the request: {0}")]
    Rpc(RpcError),
    #[error("codex app-server is not running: {0}")]
    Disconnected(String),
    #[error("could not start `{program}`: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("unexpected response shape for `{method}`: {source}")]
    Decode {
        method: String,
        source: serde_json::Error,
    },
    #[error("request `{0}` timed out")]
    Timeout(String),
}

/// Something the server sent that is not a response to one of our requests.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Notification {
        method: String,
        params: Value,
    },
    /// A server-initiated request (approvals, user input). Must be answered
    /// with [`CodexClient::respond`] or [`CodexClient::respond_error`].
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// The transport ended (process exit / EOF). Always the last item.
    Closed {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    pub max_retries: u32,
    pub initial: Duration,
    pub max: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            max_retries: 6,
            initial: Duration::from_millis(200),
            max: Duration::from_secs(5),
        }
    }
}

impl BackoffPolicy {
    fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.initial.saturating_mul(factor).min(self.max)
    }
}

#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// Path or name of the codex executable.
    pub program: PathBuf,
    /// Extra args placed before `app-server` (e.g. `-c key=value`).
    pub global_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            program: PathBuf::from("codex"),
            global_args: Vec::new(),
            env: Vec::new(),
        }
    }
}

type Pending = Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, RpcError>>>>;

struct Inner {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicI64,
    closed: AtomicBool,
    close_reason: Mutex<String>,
    stderr_tail: Mutex<VecDeque<String>>,
    child: tokio::sync::Mutex<Option<Child>>,
    stop_writer: tokio::sync::Notify,
    backoff: BackoffPolicy,
}

/// Cheap-to-clone handle to one running app-server connection.
#[derive(Clone)]
pub struct CodexClient {
    inner: Arc<Inner>,
}

impl CodexClient {
    pub async fn process_id(&self) -> Option<u32> {
        self.inner.child.lock().await.as_ref()?.id()
    }

    /// Spawn `codex app-server` (stdio transport) and connect to it.
    pub fn spawn(
        opts: &SpawnOptions,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Incoming>), ClientError> {
        let mut cmd = Command::new(&opts.program);
        cmd.args(&opts.global_args)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group: `codex` is a Node wrapper over a native binary.
        #[cfg(unix)]
        cmd.process_group(0);
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        del_agent::environment::clear_worker_session_env(cmd.as_std_mut());
        let mut child = cmd.spawn().map_err(|source| ClientError::Spawn {
            program: opts.program.display().to_string(),
            source,
        })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (client, rx) = Self::from_io(stdout, stdin);
        *client.inner.child.try_lock().expect("fresh mutex") = Some(child);
        let inner = client.inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = inner.stderr_tail.lock().unwrap();
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
        Ok((client, rx))
    }

    /// Connect over arbitrary byte streams (used by tests and replay).
    pub fn from_io<R, W>(reader: R, writer: W) -> (Self, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        Self::from_io_with(reader, writer, BackoffPolicy::default())
    }

    pub fn from_io_with<R, W>(
        reader: R,
        mut writer: W,
        backoff: BackoffPolicy,
    ) -> (Self, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            out: out_tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
            close_reason: Mutex::new(String::new()),
            stderr_tail: Mutex::new(VecDeque::new()),
            child: tokio::sync::Mutex::new(None),
            stop_writer: tokio::sync::Notify::new(),
            backoff,
        });

        // Writer: ends (closing the server's stdin) on shutdown or once every
        // sender is dropped.
        let weak_w = Arc::downgrade(&inner);
        tokio::spawn(async move {
            loop {
                let Some(inner) = weak_w.upgrade() else { break };
                let line = tokio::select! {
                    line = out_rx.recv() => line,
                    _ = inner.stop_writer.notified() => None,
                };
                drop(inner);
                let Some(line) = line else { break };
                if writer.write_all(line.as_bytes()).await.is_err()
                    || writer.write_all(b"\n").await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });

        // Reader: demultiplex until EOF, then fail everything still pending.
        let weak = Arc::downgrade(&inner);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            let reason = loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let Some(inner) = weak.upgrade() else { return };
                        if let Some(incoming) = inner.dispatch(&line) {
                            let _ = in_tx.send(incoming);
                        }
                    }
                    Ok(None) => break "app-server closed its output".to_owned(),
                    Err(e) => break format!("read error: {e}"),
                }
            };
            if let Some(inner) = weak.upgrade() {
                inner.fail_all(&reason);
            }
            let _ = in_tx.send(Incoming::Closed { reason });
        });

        (Self { inner }, in_rx)
    }

    /// Recent stderr lines from the child (diagnostics only).
    pub fn stderr_tail(&self) -> Vec<String> {
        self.inner
            .stderr_tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// Send a request and await its raw result. Retries with exponential
    /// backoff when the server answers `-32001` (overloaded).
    pub async fn request(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<Value, ClientError> {
        let params = serde_json::to_value(params).map_err(|source| ClientError::Decode {
            method: method.to_owned(),
            source,
        })?;
        let mut attempt = 0;
        loop {
            match self.request_once(method, &params).await {
                Err(ClientError::Rpc(e))
                    if e.code == OVERLOADED_CODE && attempt < self.inner.backoff.max_retries =>
                {
                    tokio::time::sleep(self.inner.backoff.delay(attempt)).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    /// Like [`request`](Self::request) but decodes the result.
    pub async fn call<R: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<R, ClientError> {
        let value = self.request(method, params).await?;
        serde_json::from_value(value).map_err(|source| ClientError::Decode {
            method: method.to_owned(),
            source,
        })
    }

    async fn request_once(&self, method: &str, params: &Value) -> Result<Value, ClientError> {
        let id = RequestId::Int(self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        {
            // Check `closed` under the lock so a concurrent fail_all cannot miss us.
            let mut pending = self.inner.pending.lock().unwrap();
            if self.inner.closed.load(Ordering::SeqCst) {
                return Err(ClientError::Disconnected(
                    self.inner.close_reason.lock().unwrap().clone(),
                ));
            }
            pending.insert(id.clone(), tx);
        }
        let mut msg = json!({ "id": id, "method": method });
        if !params.is_null() {
            msg["params"] = params.clone();
        }
        if self.inner.out.send(msg.to_string()).is_err() {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(ClientError::Disconnected("writer stopped".into()));
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(ClientError::Rpc(e)),
            Err(_) => Err(ClientError::Disconnected(
                self.inner.close_reason.lock().unwrap().clone(),
            )),
        }
    }

    /// Send a notification (no response expected).
    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ClientError> {
        let mut msg = json!({ "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.inner
            .out
            .send(msg.to_string())
            .map_err(|_| ClientError::Disconnected("writer stopped".into()))
    }

    /// Answer a server-initiated request.
    pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), ClientError> {
        self.inner
            .out
            .send(json!({ "id": id, "result": result }).to_string())
            .map_err(|_| ClientError::Disconnected("writer stopped".into()))
    }

    pub fn respond_error(
        &self,
        id: &RequestId,
        code: i64,
        message: &str,
    ) -> Result<(), ClientError> {
        self.inner
            .out
            .send(json!({ "id": id, "error": { "code": code, "message": message } }).to_string())
            .map_err(|_| ClientError::Disconnected("writer stopped".into()))
    }

    /// Perform the mandatory `initialize` / `initialized` handshake.
    pub async fn initialize(&self, name: &str, version: &str) -> Result<Value, ClientError> {
        let result = self
            .request(
                "initialize",
                json!({ "clientInfo": { "name": name, "version": version }, "capabilities": {"experimentalApi": true} }),
            )
            .await?;
        self.notify("initialized", None)?;
        Ok(result)
    }

    /// Close stdin so the server exits on its own; kill it after `grace`.
    pub async fn shutdown(&self, grace: Duration) {
        self.inner.fail_all("client shut down");
        self.inner.stop_writer.notify_one(); // writer drops stdin -> EOF for the server
        let child = self.inner.child.lock().await.take();
        if let Some(mut child) = child {
            let pid = child.id();
            if tokio::time::timeout(grace, child.wait()).await.is_err() {
                match pid {
                    Some(pid) => {
                        del_agent::procgroup::terminate_group(pid, &mut child, grace).await
                    }
                    None => {
                        let _ = child.kill().await;
                    }
                }
            } else if let Some(pid) = pid {
                // The wrapper exited; make sure nothing it started lingers.
                del_agent::procgroup::signal_group(pid, true);
            }
        }
    }
}

impl Inner {
    /// Classify one stdout line. Responses are routed here; the rest is returned.
    fn dispatch(&self, line: &str) -> Option<Incoming> {
        if line.trim().is_empty() {
            return None;
        }
        let Ok(mut v) = serde_json::from_str::<Value>(line) else {
            return None; // non-JSON noise on stdout; ignore
        };
        let method = v.get("method").and_then(Value::as_str).map(str::to_owned);
        let id = v
            .get("id")
            .and_then(|i| serde_json::from_value::<RequestId>(i.clone()).ok());
        match (method, id) {
            (Some(method), Some(id)) => Some(Incoming::Request {
                id,
                method,
                params: v["params"].take(),
            }),
            (Some(method), None) => Some(Incoming::Notification {
                method,
                params: v["params"].take(),
            }),
            (None, Some(id)) => {
                let tx = self.pending.lock().unwrap().remove(&id)?;
                let outcome = match v.get("error") {
                    Some(err) => Err(serde_json::from_value::<RpcError>(err.clone()).unwrap_or(
                        RpcError {
                            code: -32603,
                            message: err.to_string(),
                            data: None,
                        },
                    )),
                    None => Ok(v["result"].take()),
                };
                let _ = tx.send(outcome);
                None
            }
            (None, None) => None,
        }
    }

    fn fail_all(&self, reason: &str) {
        let mut pending = self.pending.lock().unwrap();
        self.closed.store(true, Ordering::SeqCst);
        *self.close_reason.lock().unwrap() = reason.to_owned();
        pending.clear(); // dropping senders wakes awaiters with Disconnected
    }
}
