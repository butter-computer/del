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
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

use crate::protocol::{Translator, interrupt_line, user_message_line};
use base64::Engine as _;

#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    /// Path or name of the `claude` executable.
    pub binary: PathBuf,
    pub config_dir: Option<PathBuf>,
    /// Value for `--setting-sources` (e.g. `"project"`); `None` loads all of the user's settings.
    pub setting_sources: Option<String>,
    /// Extra raw CLI arguments appended after del's own.
    pub extra_args: Vec<String>,
}

impl Default for ClaudeOptions {
    fn default() -> Self {
        Self {
            binary: PathBuf::from(Provider::Claude.binary()),
            config_dir: None,
            setting_sources: None,
            extra_args: vec![],
        }
    }
}

impl ClaudeOptions {
    pub(crate) fn prepare(&self, command: &mut Command) {
        if let Some(root) = &self.config_dir {
            command.env("CLAUDE_CONFIG_DIR", root).env("DEL_ACCOUNT_PROFILE", "1");
        }
    }
}

/// How long `claude` gets to exit after SIGTERM before its group is killed.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
/// How long `claude auth status` may take.
const AUTH_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Goal transcript polling interval, and the slowest retry while it is missing.
const GOAL_POLL: std::time::Duration = std::time::Duration::from_millis(500);
const GOAL_SEARCH_MAX: std::time::Duration = std::time::Duration::from_secs(5);
/// Longest stdout line accepted; a larger one is skipped rather than buffered.
const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Read one `\n`-terminated line of at most `max` bytes. `Ok(None)` is EOF;
/// `Err(len)` inside reports a skipped oversized line. Invalid UTF-8 is
/// replaced rather than treated as a read error.
async fn read_line_capped<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Option<std::result::Result<String, usize>>> {
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

struct Live {
    events: broadcast::Sender<AgentEvent>,
    goal: Mutex<Option<del_agent::Goal>>,
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
            if let AgentEvent::GoalUpdated { goal } = &e {
                *self.goal.lock().unwrap() = goal.clone();
            }
            let _ = self.events.send(e);
        }
    }

    /// Terminate the process group (SIGTERM, then SIGKILL after the grace).
    async fn terminate(&self) {
        self.closing.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut child = self.child.lock().await;
        match self.pid {
            Some(pid) => del_agent::procgroup::terminate_group(pid, &mut child, CLOSE_GRACE).await,
            None => {
                let _ = child.start_kill();
            }
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
    browser_tools: Mutex<Option<del_agent::backend::BrowserTools>>,
    options: ClaudeOptions,
    sessions: Mutex<HashMap<SessionId, Arc<Live>>>,
    titles: Arc<Mutex<crate::titles::Titles>>,
}

impl ClaudeBackend {
    pub fn new(options: ClaudeOptions) -> Self {
        Self {
            options,
            browser_tools: Mutex::new(None),
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
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            mode,
        ]
        .map(String::from)
        .to_vec();
        if config.permission_mode == PermissionMode::FullAuto {
            a.push("--dangerously-skip-permissions".into());
        }
        a.extend(session_flag.map(String::from));
        if let Some(m) = &config.model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(effort) = &config.reasoning_effort {
            a.extend(["--effort".into(), effort.clone()]);
        }
        a.extend([
            "--settings".into(),
            serde_json::json!({"fastMode": config.service_tier.as_deref() == Some("fast")})
                .to_string(),
        ]);
        if let Some(s) = &self.options.setting_sources {
            a.extend(["--setting-sources".into(), s.clone()]);
        }
        // The per-chat `--mcp-config` is added at spawn, once the id is known.
        if self.browser_tools.lock().unwrap().is_some() {
            a.extend([
                "--append-system-prompt".into(),
                del_agent::backend::BROWSER_INSTRUCTIONS.into(),
            ]);
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
        // Held until the new process is registered: two concurrent resumes of
        // one id must share a single `claude`, never start two.
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(live) = sessions.get(&id) {
            if !live.closing.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(SessionHandle::new(
                    id,
                    Provider::Claude,
                    live.events.clone(),
                ));
            }
            // That process exited; start a fresh one for the session.
            sessions.remove(&id);
        }
        let mut command = Command::new(&self.options.binary);
        self.options.prepare(&mut command);
        let mut args = self.args(config, session_flag);
        if let Some(factory) = self
            .browser_tools
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|tools| tools.claude_session_config.clone())
        {
            let path = factory(&id)?;
            args.extend(["--mcp-config".into(), path.to_string_lossy().into_owned()]);
            if let Some(index) = args.iter().position(|arg| arg == "--append-system-prompt") {
                args[index + 1].push_str("\n\n");
                args[index + 1].push_str(del_agent::backend::GOAL_INSTRUCTIONS);
            }
        }
        command
            .args(args)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Own process group, so closing takes down the MCP servers and tools too.
            .process_group(0);
        del_agent::environment::prepare_provider_command(command.as_std_mut());
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
            goal: Mutex::new(None),
            stdin: AsyncMutex::new(stdin),
            translator: Mutex::new(Translator::new()),
            child: AsyncMutex::new(child),
            pid,
            closing: Default::default(),
        });

        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr);
            while let Ok(Some(l)) = read_line_capped(&mut stderr, 64 * 1024).await {
                let Ok(l) = l else { continue };
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
            let mut stdout = BufReader::new(stdout);
            while let Ok(Some(line)) = read_line_capped(&mut stdout, MAX_LINE_BYTES).await {
                match line {
                    Ok(line) => {
                        let evs = reader.translator.lock().unwrap().handle_line(&line);
                        reader.emit(evs);
                    }
                    Err(len) => {
                        eprintln!("claude: skipped a {len}-byte stdout line over the size limit")
                    }
                }
            }
            // stdout closed (or failed): the process is exiting or unusable.
            // Poll rather than hold the child lock across `wait`, so close()
            // can still take it; kill a process that lingers without stdout.
            let started = std::time::Instant::now();
            let mut killed = false;
            let code = loop {
                match reader.child.lock().await.try_wait() {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                if !killed && started.elapsed() > CLOSE_GRACE {
                    killed = true;
                    match reader.pid {
                        Some(pid) => {
                            del_agent::procgroup::signal_group(pid, true);
                        }
                        None => {
                            let _ = reader.child.lock().await.start_kill();
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            };
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
            reader
                .closing
                .store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let watcher = live.clone();
        let goal_cwd = config.cwd.clone();
        let goal_root = self.options.config_dir.clone();
        let goal_id = id.as_str().to_owned();
        tokio::spawn(async move {
            // The first read starts at offset 0 on purpose: it restores the
            // newest goal state from every session record, which the history
            // (latest message branch only) does not guarantee to include.
            let mut state = Some((crate::goals::Reader::new(), None::<PathBuf>));
            let mut last = None;
            let mut delay = GOAL_POLL;
            loop {
                tokio::time::sleep(delay).await;
                if watcher.closing.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let Some((mut reader, mut path)) = state.take() else {
                    break;
                };
                let (root, cwd, id) = (goal_root.clone(), goal_cwd.clone(), goal_id.clone());
                // File IO stays off the async workers.
                let Ok((reader, path, events)) = tokio::task::spawn_blocking(move || {
                    if path.is_none() {
                        path = match &root {
                            Some(root) => crate::titles::transcript_path(root, &cwd, &id),
                            None => crate::history::path(&cwd, &id),
                        };
                    }
                    let events = path
                        .as_ref()
                        .and_then(|path| reader.read(path, &id).ok())
                        .unwrap_or_default();
                    (reader, path, events)
                })
                .await
                else {
                    break;
                };
                // Until Claude writes the transcript, searching for it scans
                // every project directory; back off instead of doing that twice a second.
                delay = if path.is_some() {
                    GOAL_POLL
                } else {
                    (delay * 2).min(GOAL_SEARCH_MAX)
                };
                state = Some((reader, path));
                // Only the newest state matters on initial restore.
                if let Some(event) = events.last()
                    && last.as_ref() != Some(event)
                {
                    watcher.emit(vec![event.clone()]);
                    last = Some(event.clone());
                }
            }
        });
        sessions.insert(id.clone(), live);
        Ok(SessionHandle::new(id, Provider::Claude, tx))
    }

    async fn submit(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let live = self.live(session)?;
        let line = input_line(&input).await?;
        let turn_id = uuid::Uuid::new_v4().to_string();
        let evs = live.translator.lock().unwrap().enqueue_turn(turn_id);
        live.emit(evs);
        if let Err(error) = live.write_line(&line).await {
            let events = live.translator.lock().unwrap().abort();
            live.emit(events);
            return Err(error);
        }
        Ok(())
    }
}

async fn input_line(input: &UserInput) -> Result<String> {
    if input.skills.len() > 6 {
        return Err(AgentError::Backend(
            "Claude Code supports up to six skills per message.".into(),
        ));
    }
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
    async fn session_process(&self, id: &SessionId) -> Option<(u32, bool)> {
        self.live(id).ok()?.pid.map(|pid| (pid, false))
    }
    fn set_browser_tools(&self, tools: del_agent::backend::BrowserTools) {
        *self.browser_tools.lock().unwrap() = Some(tools);
    }

    fn provider(&self) -> Provider {
        Provider::Claude
    }

    async fn generate_title(&self, context: &str) -> Result<Option<String>> {
        let mut command = Command::new(&self.options.binary);
        self.options.prepare(&mut command);
        command
            .args([
                "-p",
                "--model",
                "claude-haiku-5-5",
                "--effort",
                "low",
                "--thinking",
                "disabled",
                "--output-format",
                "json",
                "--no-session-persistence",
                "--max-turns",
                "1",
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--setting-sources",
                "",
                "--settings",
                "{\"disableAllHooks\":true,\"alwaysThinkingEnabled\":false}",
                "--system-prompt",
                del_agent::titles::INSTRUCTIONS,
            ])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        del_agent::environment::prepare_provider_command(command.as_std_mut());
        let mut child = command.spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin.write_all(context.as_bytes()).await?;
        drop(stdin);
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
                .await
                .map_err(|_| AgentError::Backend("Title generation timed out".into()))??;
        if !output.status.success() {
            return Err(AgentError::Backend("Claude title generation failed".into()));
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| AgentError::Backend(e.to_string()))?;
        if value["is_error"] == true {
            return Err(AgentError::Backend(
                "Claude could not generate a title".into(),
            ));
        }
        Ok(value["result"].as_str().and_then(del_agent::titles::clean))
    }

    async fn list_models(&self, cwd: &std::path::Path) -> Result<Vec<del_agent::ModelInfo>> {
        crate::models::discover(&self.options, cwd).await
    }

    async fn list_commands(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Vec<del_agent::ComposerCommand>> {
        let catalog = crate::models::discover_catalog(&self.options, cwd).await?;
        Ok(crate::models::parse_commands(&catalog))
    }

    async fn invoke_command(
        &self,
        session: &SessionId,
        command: del_agent::ComposerCommand,
        mut input: UserInput,
    ) -> Result<()> {
        if command.kind == del_agent::CommandKind::Goal && !input.text.starts_with("/goal") {
            input.text = format!("/goal {}", input.goal_objective());
        }
        // stream-json executes native slash commands, including goals and
        // skills; their local output and continuation arrive on the same stream.
        self.send_prompt(session, input).await
    }

    async fn control_goal(&self, session: &SessionId, action: del_agent::GoalAction) -> Result<()> {
        let text = match action {
            del_agent::GoalAction::Set(objective) => format!("/goal {objective}"),
            del_agent::GoalAction::Clear => {
                self.interrupt(session).await?;
                "/goal clear".into()
            }
            _ => return Err(AgentError::Backend("Claude supports setting and clearing goals, but has no native pause/resume command.".into())),
        };
        self.submit(session, UserInput::text(text)).await
    }

    async fn agent_goal(
        &self,
        session: &SessionId,
        action: Option<del_agent::GoalAction>,
    ) -> Result<Option<del_agent::Goal>> {
        let goal = self.live(session)?.goal.lock().unwrap().clone();
        let text = match action {
            None => return Ok(goal),
            Some(del_agent::GoalAction::Set(objective)) => format!("/goal {objective}"),
            Some(del_agent::GoalAction::Resume) => format!(
                "/goal {}",
                goal.as_ref()
                    .ok_or_else(|| AgentError::Backend(
                        "No goal to resume. Set an objective first.".into()
                    ))?
                    .objective
            ),
            Some(del_agent::GoalAction::Clear) => "/goal clear".into(),
            Some(del_agent::GoalAction::Pause) => {
                return Err(AgentError::Backend(
                    "Claude Code does not expose native goal pause; use clear to stop it.".into(),
                ));
            }
        };
        // Queue the native slash control without interrupting its calling tool.
        self.submit(session, UserInput::text(text)).await?;
        Ok(goal)
    }

    async fn session_title(
        &self,
        session: &SessionId,
        cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        let Some(root) = self.options.config_dir.clone().or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
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

    /// The CLI may consume steering within the current response or queue it
    /// for another response. Replayed input acknowledgements let the translator
    /// distinguish those cases. When idle this behaves like `send_prompt`.
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
        let evs = live.translator.lock().unwrap().mark_interrupting();
        live.emit(evs);
        let written = live
            .write_line(&interrupt_line(&uuid::Uuid::new_v4().to_string()))
            .await;
        if written.is_err() {
            // The CLI never saw the interrupt; the next result is not ours.
            live.translator.lock().unwrap().unmark_interrupting();
        }
        written
    }

    async fn resume(&self, config: SessionConfig, session: &SessionId) -> Result<SessionHandle> {
        let cwd = config.cwd.clone();
        let id = session.as_str().to_owned();
        let root = self.options.config_dir.clone();
        let history = tokio::task::spawn_blocking(move || match root {
            Some(root) => crate::history::load_from(&root, &cwd, &id),
            None => crate::history::load(&cwd, &id),
        })
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
        live.terminate().await;
        Ok(())
    }

    /// Stop every session at once: SIGTERM all groups first, then wait for
    /// them in parallel, so quitting costs one grace period, not one each.
    async fn shutdown(&self) -> Result<()> {
        let lives: Vec<_> = self.sessions.lock().unwrap().drain().map(|(_, l)| l).collect();
        let mut waits = tokio::task::JoinSet::new();
        for live in lives {
            if !live.closing.swap(true, std::sync::atomic::Ordering::SeqCst)
                && let Some(pid) = live.pid
            {
                del_agent::procgroup::signal_group(pid, false);
            }
            waits.spawn(async move { live.terminate().await });
        }
        while waits.join_next().await.is_some() {}
        Ok(())
    }
}

impl Drop for ClaudeBackend {
    /// Last resort when the app exits without `shutdown`: `kill_on_drop`
    /// only reaches the leader, so take down each group's tools and MCP servers.
    fn drop(&mut self) {
        for live in self.sessions.get_mut().unwrap().values() {
            // `closing` is set once the process was reaped; its pid may be reused.
            if !live.closing.load(std::sync::atomic::Ordering::SeqCst)
                && let Some(pid) = live.pid
            {
                del_agent::procgroup::signal_group(pid, true);
            }
        }
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
    let mut command = Command::new(&options.binary);
    options.prepare(&mut command);
    del_agent::environment::prepare_provider_command(command.as_std_mut());
    let out = tokio::time::timeout(
        AUTH_STATUS_TIMEOUT,
        command
            .args(["auth", "status"])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| AgentError::Backend("`claude auth status` timed out".into()))?
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
    fn fast_mode_is_session_scoped_for_new_and_resumed_chats() {
        let backend = ClaudeBackend::new(ClaudeOptions::default());
        let mut config = SessionConfig {
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: Some("fast".into()),
            provider: Provider::Claude,
            permission_mode: PermissionMode::Ask,
        };
        for flag in ["--session-id", "--resume"] {
            let args = backend.args(&config, [flag, "test"]);
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--settings", "{\"fastMode\":true}"])
            );
        }
        config.service_tier = None;
        assert!(
            backend
                .args(&config, ["--resume", "test"])
                .windows(2)
                .any(|pair| pair == ["--settings", "{\"fastMode\":false}"])
        );
    }
    struct Unused;
    impl del_agent::backend::BrowserCredentials for Unused {
        fn mint(&self, _: &SessionId) -> std::io::Result<String> {
            unreachable!("Claude chats use their per-chat config file")
        }
        fn rebind(&self, _: &SessionId, _: &SessionId) {}
        fn revoke(&self, _: &SessionId) {}
        fn mint_shared(&self, _: &SessionId) -> std::io::Result<String> {
            unreachable!("Claude chats use their per-chat config file")
        }
        fn set_shared_member(&self, _: &SessionId, _: &SessionId, _: bool) {}
    }

    #[test]
    fn browser_routing_is_appended_for_new_and_resumed_chats() {
        let backend = ClaudeBackend::new(ClaudeOptions::default());
        backend.set_browser_tools(del_agent::backend::BrowserTools {
            url: "http://127.0.0.1:1234/mcp".into(),
            credentials: std::sync::Arc::new(Unused),
            claude_session_config: None,
            question: None,
        });
        let config = SessionConfig {
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Claude,
            permission_mode: PermissionMode::FullAuto,
        };
        for flag in ["--session-id", "--resume"] {
            let args = backend.args(&config, [flag, "test"]);
            assert!(args.windows(2).any(|pair| pair
                == [
                    "--append-system-prompt",
                    del_agent::backend::BROWSER_INSTRUCTIONS
                ]));
            // Claude chats get their own MCP config at spawn, never a shared one.
            assert!(!args.contains(&"--mcp-config".into()));
            assert!(!args.contains(&"--system-prompt".into()));
        }
    }

    #[test]
    fn full_access_bypasses_permissions_for_new_and_resumed_chats() {
        let backend = ClaudeBackend::new(ClaudeOptions::default());
        let mut config = SessionConfig {
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Claude,
            permission_mode: PermissionMode::FullAuto,
        };
        for flag in ["--session-id", "--resume"] {
            let args = backend.args(&config, [flag, "test"]);
            assert!(args.contains(&"--dangerously-skip-permissions".into()));
            assert!(
                args.windows(2)
                    .any(|w| w == ["--permission-mode", "bypassPermissions"])
            );
            assert!(args.windows(2).any(|w| w == [flag, "test"]));
        }
        config.permission_mode = PermissionMode::Ask;
        assert!(
            !backend
                .args(&config, ["--session-id", "test"])
                .contains(&"--dangerously-skip-permissions".into())
        );
    }

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
    async fn multiple_skills_are_one_native_chained_message() {
        let mut input = UserInput::text("/review /plugin:check inspect this");
        input.skills = ["review", "plugin:check"]
            .into_iter()
            .map(|name| del_agent::ComposerCommand {
                name: name.into(),
                description: String::new(),
                argument_hint: String::new(),
                kind: del_agent::CommandKind::Skill,
                source: "Claude Code".into(),
                target: None,
            })
            .collect();
        let value: serde_json::Value =
            serde_json::from_str(&input_line(&input).await.unwrap()).unwrap();
        assert_eq!(value["message"]["content"], input.text);
        input.skills = vec![input.skills[0].clone(); 7];
        assert!(
            input_line(&input)
                .await
                .unwrap_err()
                .to_string()
                .contains("six skills")
        );
    }

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
                command: None,
                skills: Vec::new(),
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
            command: None,
            skills: Vec::new(),
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
