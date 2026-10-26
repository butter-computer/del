//! One authenticated loopback server per workspace. OpenCode owns persistence.
use del_agent::{AgentError, Result};
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

pub struct Server {
    child: Child,
    pub(crate) pid: u32,
    client: reqwest::Client,
    url: String,
    password: String,
    directory: String,
}
fn error(e: impl std::fmt::Display) -> AgentError {
    AgentError::Backend(format!("OpenCode: {e}"))
}
impl Server {
    pub async fn start(
        cwd: &Path,
        tools: Option<&del_agent::backend::BrowserTools>,
    ) -> Result<Self> {
        let binary = del_agent::detect::find_binary("opencode")
            .ok_or_else(|| error("not installed; install OpenCode and refresh Providers"))?;
        let password = uuid::Uuid::new_v4().to_string();
        let mut command = Command::new(binary);
        command
            .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .current_dir(cwd)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("OPENCODE_SERVER_USERNAME", "opencode")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        del_agent::environment::prepare_provider_command(command.as_std_mut());
        if let Some(tools) = tools {
            command.env(
                "OPENCODE_CONFIG_CONTENT",
                serde_json::json!({"mcp":{"del":{
                    "type":"remote", "url":tools.url,
                    "headers":{"Authorization":format!("Bearer {}", tools.token)}, "oauth":false
                }}})
                .to_string(),
            );
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| error("server exited during startup"))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();
        let address = tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(line) = lines.next_line().await? {
                if let Some(url) = line.strip_prefix("server listening on ") {
                    let url = reqwest::Url::parse(url.trim()).map_err(std::io::Error::other)?;
                    if url.scheme() == "http"
                        && url.host_str() == Some("127.0.0.1")
                        && url.port().is_some()
                    {
                        return Ok(url.as_str().trim_end_matches('/').to_owned());
                    }
                }
            }
            Err(std::io::Error::other(
                "server exited before becoming ready; check opencode serve in Terminal",
            ))
        })
        .await;
        let url = match address {
            Ok(Ok(url)) => url,
            result => {
                del_agent::procgroup::terminate_group(pid, &mut child, Duration::from_millis(200))
                    .await;
                return Err(error(format!("startup failed: {result:?}")));
            }
        };
        // Drain logs without retaining potentially sensitive provider output.
        tokio::spawn(async move { while matches!(lines.next_line().await, Ok(Some(_))) {} });
        Ok(Self {
            child,
            pid,
            client: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .map_err(error)?,
            url,
            password,
            directory: cwd.to_string_lossy().into_owned(),
        })
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
                &format!("/api/experimental/session/{id}/log"),
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
        del_agent::procgroup::signal_group(self.pid, true);
        let _ = self.child.start_kill();
    }
}
