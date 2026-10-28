//! One authenticated loopback server per workspace. OpenCode owns persistence.
use del_agent::{AgentError, Result};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
};

/// Longest startup log line kept before it is treated as noise.
const MAX_LINE_BYTES: usize = 1 << 20;

/// Owns the child until startup succeeds; a cancelled or failed start kills the
/// whole process group instead of relying on `kill_on_drop` for the leader only.
struct Starting {
    child: Option<Child>,
    pid: u32,
}
impl Drop for Starting {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            del_agent::procgroup::signal_group(self.pid, false);
            let _ = child.start_kill();
            del_agent::procgroup::signal_group(self.pid, true);
        }
    }
}

pub struct Server {
    child: std::sync::Mutex<Child>,
    /// Held open for `--stdio` mode, which exits when it closes.
    _stdin: Option<tokio::process::ChildStdin>,
    /// The leader was reaped, so `pid` must no longer be signalled.
    reaped: std::sync::atomic::AtomicBool,
    pub(crate) pid: u32,
    client: reqwest::Client,
    url: String,
    password: String,
    directory: String,
    /// Del's browser credential for this workspace server: one MCP config
    /// serves all its chats, so it is a shared group (id, token).
    pub(crate) browser: std::sync::Mutex<Option<(del_agent::SessionId, String)>>,
}
fn error(e: impl std::fmt::Display) -> AgentError {
    AgentError::Backend(format!("OpenCode: {e}"))
}
enum StartError {
    /// The process ended before announcing its address (e.g. unknown flag).
    Exited(AgentError),
    Other(AgentError),
}
impl StartError {
    fn into_agent(self) -> AgentError {
        match self {
            StartError::Exited(e) | StartError::Other(e) => e,
        }
    }
}
impl Server {
    /// Start `opencode serve` for `cwd`.
    ///
    /// `--stdio` mode removes the password from the server's environment after
    /// reading it, so shells and tools the agent runs cannot inherit it and
    /// call this API (e.g. to approve their own permissions). It also exits
    /// when Del closes its stdin. Del's MCP config, which carries a bearer
    /// token, is installed over the API rather than through the environment.
    /// Builds without `--stdio` fall back to the plain listener.
    pub async fn start(cwd: &Path, environment: &[(String, String)]) -> Result<Self> {
        match Self::start_mode(cwd, environment, true).await {
            Err(StartError::Exited(_)) => Self::start_mode(cwd, environment, false)
                .await
                .map_err(StartError::into_agent),
            result => result.map_err(StartError::into_agent),
        }
    }

    async fn start_mode(
        cwd: &Path,
        environment: &[(String, String)],
        stdio: bool,
    ) -> std::result::Result<Self, StartError> {
        let binary = del_agent::detect::find_binary("opencode").ok_or_else(|| {
            StartError::Other(error(
                "not installed; install OpenCode and refresh Providers",
            ))
        })?;
        let password = uuid::Uuid::new_v4().to_string();
        let mut command = Command::new(binary);
        command.envs(environment.iter().cloned());
        command.arg("serve");
        if stdio {
            command.arg("--stdio");
        }
        command
            .args(["--hostname", "127.0.0.1", "--port", "0"])
            .current_dir(cwd)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("OPENCODE_SERVER_USERNAME", "opencode")
            .stdin(if stdio { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        del_agent::environment::prepare_provider_command(command.as_std_mut());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|e| StartError::Other(e.into()))?;
        let pid = child
            .id()
            .ok_or_else(|| StartError::Other(error("server exited during startup")))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut starting = Starting {
            child: Some(child),
            pid,
        };
        let mut reader = BufReader::new(stdout);
        let address = tokio::time::timeout(Duration::from_secs(20), async {
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                // Bytes, not `lines()`: one non-UTF-8 log line must not end the read.
                let read = (&mut reader)
                    .take(MAX_LINE_BYTES as u64)
                    .read_until(b'\n', &mut bytes)
                    .await?;
                if read == 0 {
                    break;
                }
                let line = String::from_utf8_lossy(&bytes);
                let url = match serde_json::from_str::<Value>(line.trim()) {
                    Ok(v) => v["url"].as_str().map(str::to_owned),
                    Err(_) => line.strip_prefix("server listening on ").map(str::to_owned),
                };
                if let Some(url) = url {
                    let url = reqwest::Url::parse(url.trim()).map_err(std::io::Error::other)?;
                    if url.scheme() == "http"
                        && url.host_str() == Some("127.0.0.1")
                        && url.port().is_some()
                    {
                        return Ok(Some(url.as_str().trim_end_matches('/').to_owned()));
                    }
                }
            }
            Ok::<_, std::io::Error>(None)
        })
        .await;
        let url = match address {
            Ok(Ok(Some(url))) => url,
            result => {
                let mut child = starting
                    .child
                    .take()
                    .expect("child retained until startup ends");
                del_agent::procgroup::terminate_group(pid, &mut child, Duration::from_millis(200))
                    .await;
                return Err(match result {
                    Ok(Ok(None)) => StartError::Exited(error(
                        "server exited before becoming ready; check opencode serve in Terminal",
                    )),
                    other => StartError::Other(error(format!("startup failed: {other:?}"))),
                });
            }
        };
        let child = starting
            .child
            .take()
            .expect("child retained until startup ends");
        // Drain logs as raw bytes without retaining potentially sensitive provider
        // output; stopping before EOF would block the server once the pipe fills.
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
        });
        Ok(Self {
            child: std::sync::Mutex::new(child),
            _stdin: stdin,
            reaped: Default::default(),
            pid,
            client: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .map_err(|e| StartError::Other(error(e)))?,
            url,
            password,
            directory: cwd.to_string_lossy().into_owned(),
            browser: Default::default(),
        })
    }
    /// Whether `opencode serve` is still running.
    pub fn is_alive(&self) -> bool {
        use std::sync::atomic::Ordering;
        if self.reaped.load(Ordering::SeqCst) {
            return false;
        }
        match self.child.lock().unwrap().try_wait() {
            Ok(None) => true,
            _ => {
                // Sweep anything it left in its group while the id is still ours.
                del_agent::procgroup::signal_group(self.pid, true);
                self.reaped.store(true, Ordering::SeqCst);
                false
            }
        }
    }
    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.url))
            .basic_auth("opencode", Some(&self.password))
            .query(&[("location[directory]", &self.directory)])
    }
    pub async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut request = self.request(method, path).timeout(Duration::from_secs(30));
        if let Some(body) = body {
            request = request.json(body);
        }
        decode(request.send().await.map_err(error)?).await
    }
    /// Capture a native durable-log watermark without running a model.
    pub async fn watermark(
        &self,
        id: &del_agent::SessionId,
        after: Option<u64>,
    ) -> Result<Option<u64>> {
        let mut request = self
            .request(
                reqwest::Method::GET,
                &format!("/api/experimental/session/{}/log", crate::seg(id.as_str())),
            )
            .query(&[("follow", "false")])
            .timeout(Duration::from_secs(30));
        if let Some(after) = after {
            request = request.query(&[("after", after)]);
        }
        let text = request
            .send()
            .await
            .map_err(error)?
            .error_for_status()
            .map_err(error)?
            .text()
            .await
            .map_err(error)?;
        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                let event: Value = serde_json::from_str(data).map_err(error)?;
                if event["type"] == "log.synced" {
                    return Ok(event["seq"].as_u64());
                }
            }
        }
        Err(error("session log did not return a synchronization marker"))
    }

    pub async fn events(&self) -> Result<reqwest::Response> {
        self.events_after(None).await
    }

    /// Subscribe to the event stream, asking the server to replay durable events
    /// newer than `after` when it supports that query; otherwise subscribe fresh.
    pub async fn events_after(&self, after: Option<u64>) -> Result<reqwest::Response> {
        if let Some(after) = after {
            let request = self
                .request(reqwest::Method::GET, "/api/event")
                .query(&[("after", after)]);
            if let Ok(response) = request.send().await
                && let Ok(response) = response.error_for_status()
            {
                return Ok(response);
            }
        }
        self.request(reqwest::Method::GET, "/api/event")
            .send()
            .await
            .map_err(error)?
            .error_for_status()
            .map_err(error)
    }
}
pub async fn decode(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().await.map_err(error)?;
    if !status.is_success() {
        return Err(error(format!(
            "HTTP {status}: {}",
            text.chars().take(1000).collect::<String>()
        )));
    }
    if text.is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text).map_err(error)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if !*self.reaped.get_mut() {
            del_agent::procgroup::signal_group(self.pid, true);
        }
        let _ = self.child.get_mut().unwrap().start_kill();
    }
}
