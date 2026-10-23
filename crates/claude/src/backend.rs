//! [`AgentBackend`] over the user's installed `claude` CLI.
//!
//! One long-lived `claude -p --input-format stream-json` process per session.
//! Prompts, interrupts and approval answers go to its stdin; events come back on
//! stdout. Authentication is whatever the CLI already has (`claude auth login`
//! or `ANTHROPIC_API_KEY` in the inherited environment); del never touches
//! credentials.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use del_agent::{
    AgentBackend, AgentError, AgentEvent, ApprovalResponse, EVENT_CHANNEL_CAPACITY, PermissionMode,
    Provider, Result, SessionConfig, SessionHandle, SessionId, UserInput,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

use crate::protocol::{Translator, interrupt_line, user_message_line};
use base64::Engine as _;

#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    /// Path or name of the `claude` executable.
    pub binary: PathBuf,
    /// Value for `--setting-sources` (e.g. `"project"`); `None` loads all of the user's settings.
    pub setting_sources: Option<String>,
    /// Extra raw CLI arguments appended after del's own.
    pub extra_args: Vec<String>,
}

impl Default for ClaudeOptions {
    fn default() -> Self {
        Self {
            binary: PathBuf::from(Provider::Claude.binary()),
            setting_sources: None,
            extra_args: vec![],
        }
    }
}

/// How long `claude` gets to exit after SIGTERM before its group is killed.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct Live {
    events: broadcast::Sender<AgentEvent>,
    stdin: AsyncMutex<ChildStdin>,
    translator: Mutex<Translator>,
    child: AsyncMutex<Child>,
    /// Leader of the process group `claude` and its tools run in.
    pid: Option<u32>,
    closing: std::sync::atomic::AtomicBool,
}

impl Live {
    fn emit(&self, evs: Vec<AgentEvent>) {
        for e in evs {
            let _ = self.events.send(e);
        }
    }

    async fn write_line(&self, line: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }
}

#[derive(Default)]
pub struct ClaudeBackend {
    options: ClaudeOptions,
    sessions: Mutex<HashMap<SessionId, Arc<Live>>>,
    titles: Arc<Mutex<crate::titles::Titles>>,
}

impl ClaudeBackend {
    pub fn new(options: ClaudeOptions) -> Self {
        Self {
            options,
            sessions: Mutex::default(),
            titles: Default::default(),
        }
    }

    fn live(&self, id: &SessionId) -> Result<Arc<Live>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    fn args(&self, config: &SessionConfig, session_flag: [&str; 2]) -> Vec<String> {
        let mode = match config.permission_mode {
            PermissionMode::Ask => "default",
            PermissionMode::AutoEdit => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::FullAuto => "bypassPermissions",
        };
        let mut a: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            mode,
        ]
        .map(String::from)
        .to_vec();
        if config.permission_mode == PermissionMode::FullAuto {
            a.push("--allow-dangerously-skip-permissions".into());
        }
        a.extend(session_flag.map(String::from));
        if let Some(m) = &config.model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(effort) = &config.reasoning_effort {
            a.extend(["--effort".into(), effort.clone()]);
        }
        if let Some(s) = &self.options.setting_sources {
            a.extend(["--setting-sources".into(), s.clone()]);
        }
        a.extend(self.options.extra_args.iter().cloned());
        a
    }

    fn spawn(
        &self,
        config: &SessionConfig,
        id: SessionId,
        session_flag: [&str; 2],
    ) -> Result<SessionHandle> {
        if let Some(live) = self.sessions.lock().unwrap().get(&id) {
            return Ok(SessionHandle::new(
                id,
                Provider::Claude,
                live.events.clone(),
            ));
        }
        let mut command = Command::new(&self.options.binary);
        command
            .args(self.args(config, session_flag))
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Own process group, so closing takes down the MCP servers and tools too.
            .process_group(0);
        del_agent::environment::clear_worker_session_env(command.as_std_mut());
        let mut child = command.spawn().map_err(|e| {
            AgentError::Backend(format!(
                "failed to start `{}`: {e}",
                self.options.binary.display()
            ))
        })?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let live = Arc::new(Live {
            events: tx.clone(),
            stdin: AsyncMutex::new(stdin),
            translator: Mutex::new(Translator::new()),
            child: AsyncMutex::new(child),
            pid,
            closing: Default::default(),
        });

        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                if t.len() > 2000 {
                    t.clear();
                }
                t.push_str(&l);
                t.push('\n');
            }
        });

        let reader = live.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let evs = reader.translator.lock().unwrap().handle_line(&line);
                reader.emit(evs);
            }
            // stdout closed: the process exited.
            let code = reader
                .child
                .lock()
                .await
                .wait()
                .await
                .ok()
                .and_then(|s| s.code());
            let evs = reader.translator.lock().unwrap().abort();
            reader.emit(evs);
            if !reader.closing.load(std::sync::atomic::Ordering::SeqCst) {
                let stderr = stderr_tail.lock().unwrap().trim().to_owned();
                let code = code.map_or("signal".to_owned(), |c| c.to_string());
                let message = format!(
                    "claude exited unexpectedly (code {code}){}",
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(": {stderr}")
                    }
                );
                reader.emit(vec![AgentEvent::Error {
                    message,
                    recoverable: false,
                }]);
            }
        });

        self.sessions.lock().unwrap().insert(id.clone(), live);
        Ok(SessionHandle::new(id, Provider::Claude, tx))
    }

    async fn submit(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let live = self.live(session)?;
        let line = input_line(&input).await?;
        let turn_id = uuid::Uuid::new_v4().to_string();
        live.write_line(&line).await?;
        let evs = live.translator.lock().unwrap().enqueue_turn(turn_id);
        live.emit(evs);
        Ok(())
    }
}

async fn input_line(input: &UserInput) -> Result<String> {
    let mut content = vec![serde_json::json!({"type":"text", "text":input.text})];
    for attachment in &input.attachments {
        if let Some(mime) = attachment
            .mime
            .as_deref()
            .filter(|mime| mime.starts_with("image/"))
        {
            let bytes = tokio::fs::read(&attachment.path).await?;
            content.push(
                serde_json::json!({"type":"image","source":{"type":"base64","media_type":mime,
                    "data":base64::engine::general_purpose::STANDARD.encode(bytes)}}),
            );
        } else {
            content.push(serde_json::json!({"type":"text","text":format!("[Attached file: {}]", attachment.path.display())}));
        }
    }
    let line = if input.attachments.is_empty() {
        user_message_line(&input.text)
    } else {
        serde_json::json!({"type":"user","message":{"role":"user","content":content}}).to_string()
    };
    Ok(line)
}

#[async_trait]
impl AgentBackend for ClaudeBackend {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    async fn list_models(&self, cwd: &std::path::Path) -> Result<Vec<del_agent::ModelInfo>> {
        crate::models::discover(&self.options, cwd).await
    }

    async fn session_title(
        &self,
        session: &SessionId,
        cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        let Some(root) = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
        else {
            return Ok(None);
        };
        let (titles, cwd, id) = (
            self.titles.clone(),
            cwd.to_owned(),
            session.as_str().to_owned(),
        );
        tokio::task::spawn_blocking(move || titles.lock().unwrap().read(&root, &cwd, &id))
            .await
            .map_err(|e| AgentError::Backend(e.to_string()))?
            .map_err(AgentError::from)
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let id = uuid::Uuid::new_v4().to_string();
        self.spawn(&config, SessionId::new(id.clone()), ["--session-id", &id])
    }

    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()> {
        if self.live(session)?.translator.lock().unwrap().is_busy() {
            return Err(AgentError::Busy(
                "a turn is already running; use steer to queue a follow-up".into(),
            ));
        }
        self.submit(session, input).await
    }

    /// The CLI has no mid-turn injection over stream-json: a user message sent
    /// while a turn runs is queued and handled as the next turn once the current
    /// one finishes (verified against claude 2.1.308). When idle this behaves
    /// like `send_prompt`.
    async fn steer(&self, session: &SessionId, text: String) -> Result<()> {
        self.submit(session, UserInput::text(text)).await
    }

    async fn steer_input(&self, session: &SessionId, input: UserInput) -> Result<()> {
        self.submit(session, input).await
    }

    async fn interrupt(&self, session: &SessionId) -> Result<()> {
        let live = self.live(session)?;
        if !live.translator.lock().unwrap().is_busy() {
            return Ok(());
        }
        live.translator.lock().unwrap().mark_interrupting();
        live.write_line(&interrupt_line(&uuid::Uuid::new_v4().to_string()))
            .await
    }

    async fn resume(&self, config: SessionConfig, session: &SessionId) -> Result<SessionHandle> {
        let cwd = config.cwd.clone();
        let id = session.as_str().to_owned();
        let history = tokio::task::spawn_blocking(move || crate::history::load(&cwd, &id))
            .await
            .map_err(|e| AgentError::Backend(e.to_string()))??;
        Ok(self
            .spawn(&config, session.clone(), ["--resume", session.as_str()])?
            .with_history(history))
    }

    async fn respond_approval(
        &self,
        session: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        let live = self.live(session)?;
        let line = live
            .translator
            .lock()
            .unwrap()
            .approval_reply(approval_id, response);
        match line {
            Some(l) => live.write_line(&l).await,
            None => Err(AgentError::Backend(format!(
                "no pending approval {approval_id}"
            ))),
        }
    }

    async fn close(&self, session: &SessionId) -> Result<()> {
        let live = self
            .sessions
            .lock()
            .unwrap()
            .remove(session)
            .ok_or_else(|| AgentError::UnknownSession(session.clone()))?;
        live.closing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut child = live.child.lock().await;
        match live.pid {
            Some(pid) => del_agent::procgroup::terminate_group(pid, &mut child, CLOSE_GRACE).await,
            None => {
                let _ = child.start_kill();
            }
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let ids: Vec<_> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.close(&id).await;
        }
        Ok(())
    }
}

/// Result of `claude auth status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub logged_in: bool,
    /// e.g. `claude.ai` (subscription login) or `api_key`.
    pub method: Option<String>,
}

/// Ask the installed CLI whether it is logged in. Reads only the status the CLI
/// prints; no credentials are touched.
pub async fn auth_status(options: &ClaudeOptions) -> Result<AuthStatus> {
    let out = Command::new(&options.binary)
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| {
            AgentError::Backend(format!("failed to run `{}`: {e}", options.binary.display()))
        })?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| AgentError::Backend(format!("unexpected `claude auth status` output: {e}")))?;
    Ok(AuthStatus {
        logged_in: v["loggedIn"].as_bool().unwrap_or(false),
        method: v["authMethod"].as_str().map(str::to_owned),
    })
}

#[cfg(test)]
mod model_config_tests {
    use super::*;
    #[test]
    fn effort_is_forwarded_only_when_selected() {
        let backend = ClaudeBackend::new(ClaudeOptions::default());
        let mut config = SessionConfig {
            cwd: "/tmp".into(),
            model: Some("sonnet".into()),
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Claude,
            permission_mode: PermissionMode::Ask,
        };
        assert!(
            !backend
                .args(&config, ["--session-id", "test"])
                .contains(&"--effort".into())
        );
        config.reasoning_effort = Some("high".into());
        let args = backend.args(&config, ["--session-id", "test"]);
        assert!(args.windows(2).any(|w| w == ["--effort", "high"]));
        assert!(args.windows(2).any(|w| w == ["--model", "sonnet"]));
    }
}

#[cfg(test)]
mod image_input_tests {
    use super::*;
    #[tokio::test]
    async fn documents_and_videos_are_file_references_not_image_payloads() {
        for (name, mime) in [
            ("report.pdf", "application/pdf"),
            (
                "book.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("notes.md", "text/markdown"),
            ("clip.mp4", "video/mp4"),
        ] {
            let input = UserInput {
                text: "Read this".into(),
                attachments: vec![del_agent::Attachment {
                    path: format!("/attachments/{name}").into(),
                    mime: Some(mime.into()),
                }],
            };
            let value: serde_json::Value =
                serde_json::from_str(&input_line(&input).await.unwrap()).unwrap();
            let block = &value["message"]["content"][1];
            assert_eq!(block["type"], "text");
            assert!(
                block["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("/attachments/{name}"))
            );
        }
    }
    #[tokio::test]
    async fn images_are_encoded_in_the_message_not_only_named_in_text() {
        let path =
            std::env::temp_dir().join(format!("del-image-wire-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, [1, 2, 3]).unwrap();
        let input = UserInput {
            text: "look".into(),
            attachments: vec![del_agent::Attachment {
                path: path.clone(),
                mime: Some("image/png".into()),
            }],
        };
        let line = input_line(&input).await.unwrap();
        std::fs::remove_file(path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["message"]["content"][0]["text"], "look");
        assert_eq!(value["message"]["content"][1]["source"]["data"], "AQID");
        assert_eq!(
            value["message"]["content"][1]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(
            input_line(&UserInput::text("plain")).await.unwrap(),
            user_message_line("plain")
        );
    }
}
