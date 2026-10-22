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
    items
}

impl CodexBackend {
    pub fn new(opts: CodexOptions) -> Self {
        Self {
            opts,
            conn: tokio::sync::Mutex::new(None),
            sessions: Default::default(),
        }
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
        let (client, rx) = CodexClient::spawn(&self.opts.spawn)?;
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
        session.emit(AgentEvent::SessionStarted {
            session_id: id.clone(),
        });
        Ok(SessionHandle::new(id, provider, session.events.clone()))
    }
}

#[async_trait]
impl AgentBackend for CodexBackend {
    fn provider(&self) -> Provider {
        Provider::Codex
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

    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let s = self.session(session)?;
        {
            let mut st = s.state.lock().unwrap();
            if st.starting || st.mapper.turn_id().is_some() {
                return Err(AgentError::Busy("a turn is already running".into()));
            }
            st.starting = true;
        }
        let conn = self.connection().await?;
        let result: std::result::Result<TurnResponse, ClientError> = conn
            .client
            .call(
                "turn/start",
                json!({ "threadId": session.as_str(), "input": turn_input(&input) }),
            )
            .await;
        let mut st = s.state.lock().unwrap();
        st.starting = false;
        let resp = result.map_err(be)?;
        st.mapper.note_turn(&resp.turn.id);
        Ok(())
    }

    async fn steer(&self, session: &SessionId, text: String) -> Result<()> {
        let s = self.session(session)?;
        let turn = s.state.lock().unwrap().mapper.turn_id().map(str::to_owned);
        let Some(turn) = turn else {
            return Err(AgentError::Backend("no turn in progress to steer".into()));
        };
        let conn = self.connection().await?;
        conn.client
            .request(
                "turn/steer",
                json!({ "threadId": session.as_str(), "expectedTurnId": turn, "input": [text_input(&text)] }),
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
        let removed = self.sessions.lock().unwrap().remove(session.as_str());
        if removed.is_none() {
            return Err(AgentError::UnknownSession(session.clone()));
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
