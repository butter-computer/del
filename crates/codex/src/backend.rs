//! [`AgentBackend`] over a single shared `codex app-server` process. Each del
//! session is one Codex thread; a router task demultiplexes notifications and
//! server requests by `threadId`.

use crate::auth::{self, AuthStatus, CodexError};
use crate::mapping::{ApprovalMethod, Mapper, RequestAction};
use crate::protocol::*;
use crate::rpc::{ClientError, CodexClient, Incoming, RequestId, SpawnOptions};
use async_trait::async_trait;
use del_agent::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

const CLIENT_NAME: &str = "del";

#[derive(Debug, Clone)]
pub struct CodexOptions {
    pub spawn: SpawnOptions,
    /// API key used only when no Codex login exists. Defaults to the
    /// `CODEX_API_KEY` / `OPENAI_API_KEY` environment variables.
    pub api_key: Option<String>,
    pub client_version: String,
    /// How long `shutdown` waits for the child to exit before killing it.
    pub shutdown_grace: Duration,
}

impl Default for CodexOptions {
    fn default() -> Self {
        Self {
            spawn: SpawnOptions::default(),
            api_key: auth::api_key_from_env(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            shutdown_grace: Duration::from_secs(3),
        }
    }
}

struct PendingApproval {
    rpc_id: RequestId,
    method: ApprovalMethod,
}

#[derive(Default)]
struct SessionState {
    mapper: Mapper,
    approvals: HashMap<String, PendingApproval>,
    /// A `turn/start` is in flight (guards double-submit).
    starting: bool,
    turn_config: Option<SessionConfig>,
}

struct Session {
    events: broadcast::Sender<AgentEvent>,
    state: Mutex<SessionState>,
}

impl Session {
    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event); // no subscribers is fine
    }
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;

struct Connection {
    client: CodexClient,
}

pub struct CodexBackend {
    browser_tools: Mutex<Option<del_agent::backend::BrowserTools>>,
    opts: CodexOptions,
    conn: tokio::sync::Mutex<Option<Arc<Connection>>>,
    sessions: Sessions,
}

fn be(e: impl std::fmt::Display) -> AgentError {
    AgentError::Backend(e.to_string())
}

/// Map del's permission mode onto Codex's approval policy + sandbox.
pub fn policy_for(mode: PermissionMode) -> (ApprovalPolicy, SandboxMode) {
    match mode {
        PermissionMode::Ask => (ApprovalPolicy::Untrusted, SandboxMode::ReadOnly),
        PermissionMode::AutoEdit => (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
        PermissionMode::Plan => (ApprovalPolicy::Never, SandboxMode::ReadOnly),
        PermissionMode::FullAuto => (ApprovalPolicy::Never, SandboxMode::DangerFullAccess),
    }
}

fn thread_params(config: &SessionConfig) -> serde_json::Map<String, Value> {
    let (approval, sandbox) = policy_for(config.permission_mode);
    let mut p = serde_json::Map::new();
    p.insert("cwd".into(), json!(config.cwd));
    p.insert("approvalPolicy".into(), json!(approval));
    p.insert("sandbox".into(), json!(sandbox));
    p.insert("serviceName".into(), json!(CLIENT_NAME));
    if let Some(m) = &config.model {
        p.insert("model".into(), json!(m));
    }
    if let Some(effort) = &config.reasoning_effort {
        p.insert("config".into(), json!({"model_reasoning_effort": effort}));
    }
    if let Some(tier) = &config.service_tier {
        p.insert("serviceTier".into(), json!(tier));
    }
    p
}

fn turn_input(input: &UserInput) -> Vec<Value> {
    let mut items = vec![text_input(&input.text)];
    for a in &input.attachments {
        let path = a.path.display().to_string();
        let is_image = a.mime.as_deref().is_some_and(|m| m.starts_with("image/"));
        items.push(if is_image {
            local_image_input(&path)
        } else {
            text_input(&format!("[attached file: {path}]"))
        });
    }
    for skill in &input.skills {
        if let Some(path) = &skill.target {
            items.push(json!({"type":"skill","name":skill.name,"path":path}));
        }
    }
    items
}

impl CodexBackend {
    pub fn new(opts: CodexOptions) -> Self {
        Self {
            opts,
            browser_tools: Mutex::new(None),
            conn: tokio::sync::Mutex::new(None),
            sessions: Default::default(),
        }
    }

    async fn start_turn(&self, session: &SessionId, items: Vec<Value>) -> Result<()> {
        let s = self.session(session)?;
        {
            let mut st = s.state.lock().unwrap();
            if st.starting || st.mapper.turn_id().is_some() {
                return Err(AgentError::Busy("a turn is already running".into()));
            }
            st.starting = true;
        }
        let conn = self.connection().await?;
        let mut params = json!({ "threadId": session.as_str(), "input": items });
        if let Some(config) = &s.state.lock().unwrap().turn_config {
            let (approval, sandbox) = policy_for(config.permission_mode);
            params["approvalPolicy"] = json!(approval);
            params["sandboxPolicy"] = json!({"type": match sandbox {
                SandboxMode::ReadOnly => "readOnly",
                SandboxMode::WorkspaceWrite => "workspaceWrite",
                SandboxMode::DangerFullAccess => "dangerFullAccess",
            }});
            params["model"] = json!(config.model);
            params["effort"] = json!(config.reasoning_effort);
            params["serviceTier"] = json!(config.service_tier);
            // A null persistent override can inherit the prior tier. Explicitly
            // select standard speed for this turn when Fast is switched off.
            params["serviceTierForTurn"] =
                json!(config.service_tier.as_deref().unwrap_or("default"));
        }
        let result: std::result::Result<TurnResponse, ClientError> =
            conn.client.call("turn/start", params).await;
        let mut st = s.state.lock().unwrap();
        st.starting = false;
        let resp = result.map_err(be)?;
        st.mapper.note_turn(&resp.turn.id);
        Ok(())
    }

    /// Check that `codex` can be started and is authenticated, without
    /// keeping a session. Useful for a settings screen.
    pub async fn check_auth(&self) -> std::result::Result<AuthStatus, CodexError> {
        let conn = self.connection().await?;
        auth::ensure_authenticated(&conn.client, self.opts.api_key.as_deref()).await
    }

    /// Attach to an already-connected client (tests, replay). Performs the
    /// initialize handshake and the auth check.
    pub async fn attach(
        opts: CodexOptions,
        client: CodexClient,
        incoming: mpsc::UnboundedReceiver<Incoming>,
    ) -> std::result::Result<Self, CodexError> {
        let backend = Self::new(opts);
        let conn = backend.handshake(client, incoming).await?;
        *backend.conn.lock().await = Some(conn);
        Ok(backend)
    }

    async fn handshake(
        &self,
        client: CodexClient,
        incoming: mpsc::UnboundedReceiver<Incoming>,
    ) -> std::result::Result<Arc<Connection>, CodexError> {
        let setup = async {
            client
                .initialize(CLIENT_NAME, &self.opts.client_version)
                .await?;
            auth::ensure_authenticated(&client, self.opts.api_key.as_deref()).await
        };
        if let Err(e) = setup.await {
            client.shutdown(Duration::from_millis(500)).await;
            return Err(e);
        }
        tokio::spawn(route(client.clone(), incoming, self.sessions.clone()));
        Ok(Arc::new(Connection { client }))
    }

    async fn connection(&self) -> std::result::Result<Arc<Connection>, CodexError> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref()
            && !c.client.is_closed()
        {
            return Ok(c.clone());
        }
        let mut spawn = self.opts.spawn.clone();
        spawn
            .global_args
            .extend(["-c".into(), "features.goals=true".into()]);
        if spawn.program == std::path::Path::new("codex") {
            spawn.program = del_agent::detect::find_working_binary("codex")
                .await
                .map(|(path, _)| path)
                .ok_or(CodexError::NotInstalled)?;
        }
        let (client, rx) = CodexClient::spawn(&spawn)?;
        let conn = self.handshake(client, rx).await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    fn session(&self, id: &SessionId) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    fn register(&self, thread_id: &str) -> Arc<Session> {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let session = Arc::new(Session {
            events: tx,
            state: Mutex::new(SessionState::default()),
        });
        self.sessions
            .lock()
            .unwrap()
            .insert(thread_id.to_owned(), session.clone());
        session
    }

    async fn open(
        &self,
        method: &str,
        mut params: serde_json::Map<String, Value>,
        thread: Option<&SessionId>,
        provider: Provider,
    ) -> Result<SessionHandle> {
        let conn = self.connection().await?;
        if let Some(tools) = self.browser_tools.lock().unwrap().as_ref() {
            apply_browser_tools(&mut params, tools);
        }
        if let Some(t) = thread {
            params.insert("threadId".into(), json!(t.as_str()));
        }
        let resp: ThreadResponse = conn
            .client
            .call(method, Value::Object(params))
            .await
            .map_err(be)?;
        let id = SessionId::new(resp.thread.id);
        let existing = self.sessions.lock().unwrap().get(id.as_str()).cloned();
        let session = existing.unwrap_or_else(|| self.register(id.as_str()));
        let mut history = if thread.is_some() {
            session
                .state
                .lock()
                .unwrap()
                .mapper
                .history(&resp.thread.turns)
        } else {
            Vec::new()
        };
        if thread.is_some()
            && let Ok(response) = conn
                .client
                .request("thread/goal/get", json!({"threadId": id.as_str()}))
                .await
        {
            history.push(AgentEvent::GoalUpdated {
                goal: crate::commands::goal(&response["goal"]),
            });
        }
        Ok(SessionHandle::new(id, provider, session.events.clone()).with_history(history))
    }
}

#[async_trait]
impl AgentBackend for CodexBackend {
    fn set_browser_tools(&self, tools: del_agent::backend::BrowserTools) {
        *self.browser_tools.lock().unwrap() = Some(tools);
    }

    fn provider(&self) -> Provider {
        Provider::Codex
    }

    async fn list_commands(&self, cwd: &std::path::Path) -> Result<Vec<ComposerCommand>> {
        let conn = self.connection().await.map_err(be)?;
        let response = conn
            .client
            .request("skills/list", json!({"cwds":[cwd],"forceReload":true}))
            .await
            .map_err(be)?;
        Ok(crate::commands::catalog(&response))
    }

    async fn invoke_command(
        &self,
        id: &SessionId,
        command: ComposerCommand,
        input: UserInput,
    ) -> Result<()> {
        let arguments = split_command(&input.text)
            .map(|(_, args)| args)
            .unwrap_or_default();
        match command.kind {
            CommandKind::Skill => {
                let path = command
                    .target
                    .ok_or_else(|| be("Skill is missing its provider path"))?;
                // Preserve the typed command in the provider's saved transcript.
                let mut items = turn_input(&input);
                if !input
                    .skills
                    .iter()
                    .any(|skill| skill.name == command.name && skill.target.as_ref() == Some(&path))
                {
                    items.push(json!({"type":"skill","name":command.name,"path":path}));
                }
                self.start_turn(id, items).await
            }
            CommandKind::Goal => {
                if !input.attachments.is_empty() {
                    return Err(be("Send attachments in a message before setting a goal."));
                }
                if input
                    .command
                    .as_ref()
                    .is_some_and(|c| c.kind == CommandKind::Goal)
                    && !input.text.starts_with("/goal")
                {
                    self.control_goal(id, del_agent::GoalAction::Set(input.goal_objective()))
                        .await?;
                    let session = self.session(id)?;
                    if session.state.lock().unwrap().mapper.turn_id().is_none() {
                        session.emit(AgentEvent::TurnCompleted {
                            turn_id: "goal-control".into(),
                            status: del_agent::TurnStatus::Completed,
                        });
                    }
                    return Ok(());
                }
                let conn = self.connection().await.map_err(be)?;
                let (method, params) = crate::commands::goal_request(id.as_str(), arguments);
                let response = conn.client.request(method, params).await.map_err(be)?;
                let session = self.session(id)?;
                session.emit(AgentEvent::GoalUpdated {
                    goal: crate::commands::goal(&response["goal"]),
                });
                let state = session.state.lock().unwrap();
                let turn_id = format!(
                    "goal-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                session.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    text: crate::commands::goal_message(method, &response),
                });
                // Activating a goal can immediately start a native turn. Do not
                // finish that turn when reporting the command's local result.
                if state.mapper.turn_id().is_none() {
                    session.emit(AgentEvent::TurnCompleted {
                        turn_id,
                        status: del_agent::TurnStatus::Completed,
                    });
                }
                Ok(())
            }
            _ => Err(be("Change Fast mode using the composer control.")),
        }
    }

    async fn control_goal(&self, id: &SessionId, action: del_agent::GoalAction) -> Result<()> {
        use del_agent::GoalAction;
        let conn = self.connection().await.map_err(be)?;
        let (method, params) = match action {
            GoalAction::Set(objective) => (
                "thread/goal/set",
                json!({"threadId":id.as_str(), "objective":objective, "status":"active"}),
            ),
            GoalAction::Pause => crate::commands::goal_request(id.as_str(), "pause"),
            GoalAction::Resume => crate::commands::goal_request(id.as_str(), "resume"),
            GoalAction::Clear => crate::commands::goal_request(id.as_str(), "clear"),
        };
        let response = conn.client.request(method, params).await.map_err(be)?;
        self.session(id)?.emit(AgentEvent::GoalUpdated {
            goal: crate::commands::goal(&response["goal"]),
        });
        Ok(())
    }

    async fn generate_title(&self, context: &str) -> Result<Option<String>> {
        let conn = self.connection().await?;
        let response: Value = conn.client.request("thread/start", json!({
            "model":"gpt-5.6-luna", "ephemeral":true, "cwd":std::env::temp_dir(),
            "approvalPolicy":"never", "sandbox":"read-only", "serviceName":"del-title",
            "baseInstructions":del_agent::titles::INSTRUCTIONS,
            "config":{"model_reasoning_effort":"none", "features.shell_tool":false, "features.multi_agent":false, "web_search":"disabled"}
        })).await.map_err(be)?;
        let id = response["thread"]["id"]
            .as_str()
            .ok_or_else(|| be("Title thread has no id"))?
            .to_owned();
        let (events, mut rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        self.sessions.lock().unwrap().insert(
            id.clone(),
            Arc::new(Session {
                events,
                state: Mutex::new(SessionState::default()),
            }),
        );
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            conn.client
                .request(
                    "turn/start",
                    json!({"threadId":id,"input":[text_input(context)],"effort":"none"}),
                )
                .await
                .map_err(be)?;
            let mut title = String::new();
            loop {
                match rx.recv().await.map_err(be)? {
                    AgentEvent::AssistantMessage { text, .. } => title = text,
                    AgentEvent::TurnCompleted {
                        status: del_agent::TurnStatus::Completed,
                        ..
                    } => return Ok(del_agent::titles::clean(&title)),
                    AgentEvent::Error { message, .. } => return Err(be(message)),
                    AgentEvent::TurnCompleted { .. }
                    | AgentEvent::ToolCallStarted { .. }
                    | AgentEvent::ApprovalRequested { .. } => {
                        return Err(be("Title request did not produce a plain reply"));
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| be("Title generation timed out"))
        .and_then(|result| result);
        let _ = self.interrupt(&SessionId::new(id.clone())).await;
        let _ = self.close(&SessionId::new(id)).await;
        result
    }

    async fn list_models(&self, _cwd: &std::path::Path) -> Result<Vec<ModelInfo>> {
        let conn = self.connection().await?;
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page: Value = conn
                .client
                .call("model/list", json!({"cursor": cursor}))
                .await
                .map_err(be)?;
            models.extend(crate::models::parse_models(&page)?);
            let next = page["nextCursor"].as_str().map(str::to_owned);
            if next.is_none() {
                break;
            }
            if next == cursor {
                return Err(be("model/list repeated its cursor"));
            }
            cursor = next;
        }
        Ok(models)
    }

    async fn session_title(
        &self,
        session: &SessionId,
        _cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        let conn = self.connection().await?;
        let response: Value = conn
            .client
            .call(
                "thread/read",
                json!({
                    "threadId": session.as_str(), "includeTurns": false,
                }),
            )
            .await
            .map_err(be)?;
        Ok(["name", "title"].into_iter().find_map(|key| {
            response["thread"][key]
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
        }))
    }

    async fn session_display_title(
        &self,
        session: &SessionId,
        _cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        let conn = self.connection().await?;
        let response: Value = conn
            .client
            .call(
                "thread/read",
                json!({
                    "threadId": session.as_str(), "includeTurns": false,
                }),
            )
            .await
            .map_err(be)?;
        Ok(["name", "title", "preview"].into_iter().find_map(|key| {
            response["thread"][key]
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(|title| {
                    title
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(120)
                        .collect()
                })
        }))
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.open(
            "thread/start",
            thread_params(&config),
            None,
            config.provider,
        )
        .await
    }

    async fn resume(&self, config: SessionConfig, session: &SessionId) -> Result<SessionHandle> {
        self.open(
            "thread/resume",
            thread_params(&config),
            Some(session),
            config.provider,
        )
        .await
    }

    async fn reconfigure(&self, config: SessionConfig, id: &SessionId) -> Result<SessionHandle> {
        let session = self.session(id)?;
        session.state.lock().unwrap().turn_config = Some(config);
        Ok(SessionHandle::new(
            id.clone(),
            Provider::Codex,
            session.events.clone(),
        ))
    }

    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()> {
        self.start_turn(session, turn_input(&input)).await
    }

    async fn steer(&self, session: &SessionId, text: String) -> Result<()> {
        self.steer_input(session, UserInput::text(text)).await
    }

    async fn steer_input(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let s = self.session(session)?;
        let turn = s.state.lock().unwrap().mapper.turn_id().map(str::to_owned);
        let Some(turn) = turn else {
            return Err(AgentError::Backend("no turn in progress to steer".into()));
        };
        let conn = self.connection().await?;
        conn.client
            .request(
                "turn/steer",
                json!({ "threadId": session.as_str(), "expectedTurnId": turn, "input": turn_input(&input) }),
            )
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn interrupt(&self, session: &SessionId) -> Result<()> {
        let s = self.session(session)?;
        let turn = s.state.lock().unwrap().mapper.turn_id().map(str::to_owned);
        let Some(turn) = turn else { return Ok(()) }; // nothing running
        let conn = self.connection().await?;
        conn.client
            .request(
                "turn/interrupt",
                json!({ "threadId": session.as_str(), "turnId": turn }),
            )
            .await
            .map_err(be)?;
        Ok(())
    }

    async fn respond_approval(
        &self,
        session: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        let s = self.session(session)?;
        let pending = s
            .state
            .lock()
            .unwrap()
            .approvals
            .remove(approval_id)
            .ok_or_else(|| AgentError::Backend(format!("no pending approval {approval_id}")))?;
        let conn = self.connection().await?;
        conn.client
            .respond(&pending.rpc_id, pending.method.result(response))
            .map_err(be)
    }

    async fn close(&self, session: &SessionId) -> Result<()> {
        let (removed, last) = {
            let mut sessions = self.sessions.lock().unwrap();
            let removed = sessions.remove(session.as_str());
            (removed, sessions.is_empty())
        };
        if removed.is_none() {
            return Err(AgentError::UnknownSession(session.clone()));
        }
        if last {
            // No session left: stop the app-server rather than leave it idle.
            // The next session spawns a fresh one.
            if let Some(conn) = self.conn.lock().await.take() {
                conn.client.shutdown(self.opts.shutdown_grace).await;
            }
            return Ok(());
        }
        if let Some(conn) = self.conn.lock().await.clone()
            && !conn.client.is_closed()
        {
            let _ = conn
                .client
                .request(
                    "thread/unsubscribe",
                    json!({ "threadId": session.as_str() }),
                )
                .await;
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        if let Some(conn) = self.conn.lock().await.take() {
            conn.client.shutdown(self.opts.shutdown_grace).await;
        }
        self.sessions.lock().unwrap().clear();
        Ok(())
    }
}

/// Demultiplex everything the server sends to the owning session.
async fn route(client: CodexClient, mut rx: mpsc::UnboundedReceiver<Incoming>, sessions: Sessions) {
    let find = |params: &Value| -> Option<Arc<Session>> {
        let tid = params["threadId"].as_str()?;
        sessions.lock().unwrap().get(tid).cloned()
    };
    while let Some(msg) = rx.recv().await {
        match msg {
            Incoming::Notification { method, params } => {
                let Some(s) = find(&params) else { continue };
                if method == "serverRequest/resolved" {
                    // Resolved server-side (e.g. interrupt): the answer is moot.
                    if let Ok(id) = serde_json::from_value::<RequestId>(params["requestId"].clone())
                    {
                        s.state.lock().unwrap().approvals.remove(&id.to_string());
                    }
                    continue;
                }
                let events = s
                    .state
                    .lock()
                    .unwrap()
                    .mapper
                    .notification(&method, &params);
                for e in events {
                    s.emit(e);
                }
            }
            Incoming::Request { id, method, params } => {
                let Some(s) = find(&params) else {
                    let _ = client.respond_error(&id, -32602, "unknown thread");
                    continue;
                };
                let action = s
                    .state
                    .lock()
                    .unwrap()
                    .mapper
                    .server_request(&id, &method, &params);
                match action {
                    RequestAction::Approval { event, method } => {
                        s.state
                            .lock()
                            .unwrap()
                            .approvals
                            .insert(id.to_string(), PendingApproval { rpc_id: id, method });
                        s.emit(event);
                    }
                    RequestAction::Reject { message, event } => {
                        let _ = client.respond_error(&id, -32601, &message);
                        if let Some(e) = event {
                            s.emit(e);
                        }
                    }
                }
            }
            Incoming::Closed { reason } => {
                let mut msg = format!("Codex app-server stopped: {reason}");
                if let Some(last) = client.stderr_tail().last() {
                    msg.push_str(&format!(" (last stderr: {last})"));
                }
                let all: Vec<_> = sessions.lock().unwrap().values().cloned().collect();
                for s in all {
                    {
                        let mut st = s.state.lock().unwrap();
                        st.approvals.clear();
                        st.starting = false;
                        st.mapper = Mapper::default();
                    }
                    s.emit(AgentEvent::Error {
                        message: msg.clone(),
                        recoverable: false,
                    });
                }
                break;
            }
        }
    }
}

fn apply_browser_tools(
    params: &mut serde_json::Map<String, Value>,
    tools: &del_agent::backend::BrowserTools,
) {
    let config = params
        .entry("config".to_owned())
        .or_insert_with(|| json!({}));
    config["mcp_servers.del_browser"] =
        json!({"url":tools.url,"http_headers":{"Authorization":format!("Bearer {}",tools.token)}});
    let existing = params
        .get("developerInstructions")
        .and_then(Value::as_str)
        .unwrap_or("");
    let policy = del_agent::backend::BROWSER_INSTRUCTIONS;
    let instructions = if existing.is_empty() {
        policy.to_owned()
    } else {
        format!("{existing}\n\n{policy}")
    };
    params.insert("developerInstructions".into(), instructions.into());
}

#[cfg(test)]
mod model_config_tests {
    #[test]
    fn browser_routing_is_included_without_losing_thread_settings() {
        let mut params = serde_json::json!({"config":{"model_reasoning_effort":"high"},"developerInstructions":"Existing instructions"}).as_object().unwrap().clone();
        super::apply_browser_tools(
            &mut params,
            &del_agent::backend::BrowserTools {
                url: "http://127.0.0.1:1234/mcp".into(),
                token: "fixture".into(),
                claude_config: "/tmp/browser.json".into(),
                claude_session_config: None,
            },
        );
        assert_eq!(params["config"]["model_reasoning_effort"], "high");
        assert_eq!(
            params["config"]["mcp_servers.del_browser"]["url"],
            "http://127.0.0.1:1234/mcp"
        );
        let instructions = params["developerInstructions"].as_str().unwrap();
        assert!(instructions.starts_with("Existing instructions\n\n"));
        assert!(instructions.contains(del_agent::backend::BROWSER_INSTRUCTIONS));
    }
    #[test]
    fn mixed_attachments_keep_documents_as_files_and_images_as_images() {
        let input = del_agent::UserInput {
            command: None,
            skills: Vec::new(),
            text: "inspect".into(),
            attachments: [
                ("report.pdf", "application/pdf"),
                ("clip.mp4", "video/mp4"),
                ("image.png", "image/png"),
            ]
            .into_iter()
            .map(|(name, mime)| del_agent::Attachment {
                path: format!("/attachments/{name}").into(),
                mime: Some(mime.into()),
            })
            .collect(),
        };
        let items = super::turn_input(&input);
        assert_eq!(items[1]["type"], "text");
        assert!(items[1]["text"].as_str().unwrap().contains("report.pdf"));
        assert_eq!(items[2]["type"], "text");
        assert!(items[2]["text"].as_str().unwrap().contains("clip.mp4"));
        assert_eq!(items[3]["type"], "localImage");
    }
    use super::*;
    #[test]
    fn full_access_disables_approval_and_sandbox_for_chat_threads() {
        let config = SessionConfig {
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Codex,
            permission_mode: PermissionMode::FullAuto,
        };
        let params = thread_params(&config);
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["sandbox"], "danger-full-access");
    }

    #[test]
    fn effort_is_forwarded_only_when_selected() {
        let mut config = SessionConfig {
            cwd: "/tmp".into(),
            model: Some("available-model".into()),
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Codex,
            permission_mode: PermissionMode::Ask,
        };
        assert!(!thread_params(&config).contains_key("config"));
        assert!(!thread_params(&config).contains_key("serviceTier"));
        config.reasoning_effort = Some("high".into());
        config.service_tier = Some("priority".into());
        let params = thread_params(&config);
        assert_eq!(params["model"], "available-model");
        assert_eq!(params["config"]["model_reasoning_effort"], "high");
        assert_eq!(params["serviceTier"], "priority");
    }
}
