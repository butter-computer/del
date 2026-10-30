//! App-owned MCP service. GPUI consumes `BridgeEvent` on its own thread; all
//! waiting and backend I/O stay on the existing Tokio runtime.
//!
//! Retain `McpBridge` for the app lifetime and clone `BridgeHandle` into terminal
//! launch tasks. `register_terminal` returns an optional opaque TerminalLease;
//! copy its config args/env into the launch and retain the lease in the host.
//! Manual callers may use upsert_terminal/launch_config and revoke on exit.
//! Dropping the owner shuts down even when launch/UI handle clones survive.

use async_trait::async_trait;
use del_agent::{AgentEvent, Provider, SessionId, SessionManager, SessionStatus};
use del_mcp::{
    HostError, HostResult, McpHost, McpServer, ReportedState, ServerOptions, SessionInfo,
    SessionMode, SessionSummary, SpawnRequest, UserAnswer,
    launch::{self, LaunchConfig, Transport},
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot, watch};

/// Handle these on the GPUI thread. Keep reply senders with the visible prompt
/// or launch task; dropping a sender fails the tool instead of hanging it.
pub enum BridgeEvent {
    /// `allowed` lists the only sessions whose browsers this request may use.
    Browser {
        caller: SessionId,
        allowed: Vec<SessionId>,
        request: del_mcp::BrowserRequest,
        reply: oneshot::Sender<HostResult<serde_json::Value>>,
    },
    AskUser {
        request_id: u64,
        caller: SessionId,
        question: String,
        options: Vec<String>,
        reply: oneshot::Sender<UserAnswer>,
    },
    /// Remove the matching prompt (answered, timed out, revoked, or shutdown).
    QuestionClosed { request_id: u64 },
    Status {
        caller: SessionId,
        state: ReportedState,
        summary: String,
    },
    Notify {
        caller: SessionId,
        title: String,
        body: String,
    },
    /// Launch a real visible terminal session and reply only after it starts.
    SpawnTerminal {
        caller: SessionId,
        request: SpawnRequest,
        reply: oneshot::Sender<HostResult<SessionInfo>>,
    },
    /// Write text to the target terminal, including the Enter/submit action.
    SendTerminal {
        caller: SessionId,
        target: SessionId,
        text: String,
        reply: oneshot::Sender<HostResult<()>>,
    },
}

#[derive(Clone)]
struct TerminalSnapshot {
    info: SessionInfo,
    last_output: Option<String>,
}

struct AppHost {
    manager: SessionManager,
    events: UnboundedSender<BridgeEvent>,
    terminals: Mutex<HashMap<SessionId, TerminalSnapshot>>,
    reported: Mutex<HashMap<SessionId, (ReportedState, String)>>,
    revocations: Mutex<HashMap<SessionId, watch::Sender<bool>>>,
    stopped: watch::Receiver<bool>,
    next_question: AtomicU64,
}

struct Service {
    // Option makes shutdown explicit even while BridgeHandle clones survive.
    server: Mutex<Option<McpServer>>,
    host: Arc<AppHost>,
    config_dir: PathBuf,
    files: Mutex<HashMap<SessionId, Vec<PathBuf>>>,
    stopped: watch::Sender<bool>,
}

/// App lifetime owner; create using the app's existing runtime (never block_on
/// on the UI thread). No runtime, terminal, or agent process is created here.
pub struct McpBridge {
    handle: BridgeHandle,
}

#[derive(Clone)]
pub struct BridgeHandle {
    service: Arc<Service>,
}

/// Opaque process-lifetime lease: box this for TerminalHost. Dropping it ends
/// only this launch's credentials/config, leaving metadata for history.
/// Do not drop it after copying args/env; retain until process exit/close.
pub struct TerminalLease {
    handle: BridgeHandle,
    id: SessionId,
    config: LaunchConfig,
    cancel: watch::Sender<bool>,
}

impl TerminalLease {
    pub fn config(&self) -> &LaunchConfig {
        &self.config
    }
}

impl Drop for TerminalLease {
    fn drop(&mut self) {
        let server = self.handle.service.server.lock().unwrap();
        if let Some(server) = server.as_ref() {
            // A late exit from an old process must not revoke its replacement.
            let current = self
                .handle
                .service
                .host
                .revocations
                .lock()
                .unwrap()
                .get(&self.id)
                .is_some_and(|cancel| cancel.same_channel(&self.cancel));
            if current {
                self.handle.revoke_locked(server, &self.id);
            }
        }
    }
}

impl McpBridge {
    pub async fn start(
        manager: SessionManager,
        events: UnboundedSender<BridgeEvent>,
        config_dir: PathBuf,
    ) -> io::Result<Self> {
        Self::start_with_options(manager, events, config_dir, ServerOptions::default()).await
    }

    pub async fn start_with_options(
        manager: SessionManager,
        events: UnboundedSender<BridgeEvent>,
        config_dir: PathBuf,
        options: ServerOptions,
    ) -> io::Result<Self> {
        std::fs::create_dir_all(&config_dir)?;
        let (stopped, rx) = watch::channel(false);
        let host = Arc::new(AppHost {
            manager,
            events,
            terminals: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashMap::new()),
            revocations: Mutex::new(HashMap::new()),
            stopped: rx,
            next_question: AtomicU64::new(1),
        });
        let server = McpServer::start(host.clone(), options).await?;
        let worker = host.clone();
        let mut shutdown = worker.stopped.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            let mut jobs = tokio::task::JoinSet::new();
            let mut in_flight = std::collections::HashSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                        if let Ok(id) = result { in_flight.remove(&id); }
                    }
                    _ = interval.tick() => {
                        for id in worker.manager.registered_peer_sessions() {
                            // Terminal agents use their explicit status reports; never
                            // type into a PTY based on a chat provider's turn boundary.
                            let terminal = worker.terminals.lock().unwrap().contains_key(&id);
                            if (!terminal && worker.manager.session_record(&id).is_err()) || worker.manager.peer_work(&id).is_none() { continue; }
                            if !in_flight.insert(id.clone()) { continue; }
                            let host = worker.clone();
                            jobs.spawn(async move {
                                let result = if terminal { host.dispatch_terminal_peer(&id).await } else { host.manager.dispatch_peer(&id).await };
                                if let Err(error) = result {
                                    eprintln!("[del peers] {id}: {error}");
                                    let _ = host.manager.control_peer_work(&id, "pause");
                                }
                                id
                            });
                        }
                    }
                }
            }
            jobs.abort_all();
        });
        Ok(Self {
            handle: BridgeHandle {
                service: Arc::new(Service {
                    server: Mutex::new(Some(server)),
                    host,
                    config_dir,
                    files: Mutex::new(HashMap::new()),
                    stopped,
                }),
            },
        })
    }

    pub fn handle(&self) -> BridgeHandle {
        self.handle.clone()
    }

    pub fn shutdown(&self) {
        let mut server = self.handle.service.server.lock().unwrap();
        // Wake pending ask/spawn/send before graceful HTTP shutdown.
        self.handle.service.stopped.send_replace(true);
        if let Some(server) = server.take() {
            server.shutdown();
        }
        for files in self
            .handle
            .service
            .files
            .lock()
            .unwrap()
            .drain()
            .map(|(_, f)| f)
        {
            remove_files(files);
        }
        self.handle.service.host.revocations.lock().unwrap().clear();
    }
}

impl Drop for McpBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Credentials bound to each chat, with separate project-peer and browser scopes.
struct ChatBrowserCredentials {
    service: std::sync::Weak<Service>,
}

impl ChatBrowserCredentials {
    fn with_server<T>(&self, f: impl FnOnce(&Service, &McpServer) -> T) -> io::Result<T> {
        let service = self
            .service
            .upgrade()
            .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
        let server = service.server.lock().unwrap();
        let server = server
            .as_ref()
            .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
        Ok(f(&service, server))
    }
}

impl Service {
    /// Pending host replies of `id` are cancelled through this entry when its
    /// credentials are revoked; keep the existing one across extra tokens.
    fn track(&self, id: &SessionId) {
        self.host
            .revocations
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert_with(|| watch::channel(false).0);
    }
}

impl del_agent::backend::BrowserCredentials for ChatBrowserCredentials {
    fn mint(&self, session: &SessionId) -> io::Result<String> {
        self.with_server(|service, server| {
            let token = server.register_chat_client(session).token;
            service.track(session);
            token
        })
    }

    fn rebind(&self, from: &SessionId, to: &SessionId) {
        let _ = self.with_server(|service, server| {
            server.rebind_session(from, to);
            let moved = service.host.revocations.lock().unwrap().remove(from);
            if let Some(cancel) = moved {
                service
                    .host
                    .revocations
                    .lock()
                    .unwrap()
                    .entry(to.clone())
                    .or_insert(cancel);
            }
        });
    }

    fn revoke(&self, session: &SessionId) {
        let _ = self.with_server(|service, server| {
            server.revoke_session(session);
            if let Some(cancel) = service.host.revocations.lock().unwrap().remove(session) {
                cancel.send_replace(true);
            }
        });
    }

    fn mint_shared(&self, group: &SessionId) -> io::Result<String> {
        self.with_server(|service, server| {
            let token = server.register_shared_browser_client(group).token;
            service.track(group);
            token
        })
    }

    fn set_shared_member(&self, group: &SessionId, session: &SessionId, member: bool) {
        let _ =
            self.with_server(|_, server| server.set_shared_browser_member(group, session, member));
    }
}

fn remove_files(files: Vec<PathBuf>) {
    for file in files {
        let _ = std::fs::remove_file(file);
    }
}

fn tail(text: String) -> String {
    let skip = text.chars().count().saturating_sub(2_000);
    text.chars().skip(skip).collect()
}

impl BridgeHandle {
    pub fn chat_browser_tools(&self) -> io::Result<del_agent::backend::BrowserTools> {
        let url = self
            .endpoint()
            .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
        Ok(del_agent::backend::BrowserTools {
            url,
            credentials: Arc::new(ChatBrowserCredentials {
                service: Arc::downgrade(&self.service),
            }),
            question: Some({
                let service = Arc::downgrade(&self.service);
                Arc::new(move |caller, question, options| {
                    let service = service.clone();
                    Box::pin(async move {
                        let service = service.upgrade().ok_or_else(|| {
                            del_agent::AgentError::Backend("Question UI closed".into())
                        })?;
                        let host = &service.host;
                        let request_id = host.next_question.fetch_add(1, Ordering::Relaxed);
                        let (reply, rx) = oneshot::channel();
                        host.events
                            .send(BridgeEvent::AskUser {
                                request_id,
                                caller,
                                question,
                                options,
                                reply,
                            })
                            .map_err(|_| {
                                del_agent::AgentError::Backend("Question UI closed".into())
                            })?;
                        let _guard = QuestionGuard {
                            events: host.events.clone(),
                            id: request_id,
                        };
                        let mut stopped = host.stopped.clone();
                        let answer = tokio::select! { result = rx => result.ok(), _ = stopped.changed() => None };
                        Ok(match answer {
                            Some(UserAnswer::Option { index, .. }) => {
                                Some(del_agent::backend::QuestionAnswer::Option(index))
                            }
                            Some(UserAnswer::Text { text }) => {
                                Some(del_agent::backend::QuestionAnswer::Text(text))
                            }
                            _ => None,
                        })
                    })
                })
            }),
            claude_session_config: Some({
                let service = Arc::downgrade(&self.service);
                Arc::new(move |id| {
                    let service = service
                        .upgrade()
                        .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
                    let server = service.server.lock().unwrap();
                    let server = server
                        .as_ref()
                        .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
                    server.revoke_session(id);
                    let credentials = server.register_chat_client(id);
                    let config =
                        launch::claude(&credentials, &Transport::Http, &service.config_dir)?;
                    let file = config.files[0].clone();
                    service
                        .files
                        .lock()
                        .unwrap()
                        .insert(id.clone(), config.files);
                    let (cancel, _) = watch::channel(false);
                    service
                        .host
                        .revocations
                        .lock()
                        .unwrap()
                        .insert(id.clone(), cancel);
                    Ok(file)
                })
            }),
        })
    }

    pub fn endpoint(&self) -> Option<String> {
        self.service
            .server
            .lock()
            .unwrap()
            .as_ref()
            .map(McpServer::url)
    }

    /// Call on session creation/metadata changes; shell/custom terminals use
    /// provider None and never receive agent tokens.
    pub fn upsert_terminal(&self, info: SessionInfo, last_output: Option<String>) {
        if info.provider.is_some()
            && !self
                .service
                .host
                .manager
                .registered_peer_sessions()
                .contains(&info.id)
            && let Err(error) = self
                .service
                .host
                .manager
                .set_peer_project(&info.id, &info.cwd)
        {
            eprintln!("[del peers] Could not register terminal: {error}");
        }
        self.service.host.terminals.lock().unwrap().insert(
            info.id.clone(),
            TerminalSnapshot {
                info,
                last_output: last_output.map(tail),
            },
        );
    }

    pub fn update_terminal_output(&self, id: &SessionId, text: String) {
        if let Some(snapshot) = self.service.host.terminals.lock().unwrap().get_mut(id) {
            snapshot.last_output = Some(tail(text));
        }
    }

    pub fn remove_terminal(&self, id: &SessionId) {
        self.revoke(id);
        if let Err(error) = self.service.host.manager.forget_peer_session(id) {
            eprintln!("[del peers] Could not remove terminal mailbox: {error}");
        }
        if let Some(server) = self.service.server.lock().unwrap().as_ref() {
            server.forget_session(id);
        }
        self.service.host.terminals.lock().unwrap().remove(id);
        self.service.host.reported.lock().unwrap().remove(id);
    }

    /// Prepare metadata and a process-lifetime lease for the terminal launcher.
    /// Run on the runtime launch task; clone lease.config().args/env into the
    /// prepared spec and pass the lease boxed to the host. Shell/custom
    /// terminals are recorded but return None because they need no MCP token.
    pub fn register_terminal(&self, info: SessionInfo) -> io::Result<Option<TerminalLease>> {
        if info.mode != SessionMode::Terminal {
            return Err(io::Error::other(
                "terminal registration requires terminal mode",
            ));
        }
        if info.provider == Some(Provider::Acp) {
            return Err(io::Error::other(
                "ACP does not use terminal MCP launch config",
            ));
        }
        let (id, provider) = (info.id.clone(), info.provider);
        self.upsert_terminal(info, None);
        let Some(provider) = provider else {
            return Ok(None);
        };
        let (config, cancel) = self.prepare_launch(&id, provider)?;
        Ok(Some(TerminalLease {
            handle: self.clone(),
            id,
            config,
            cancel,
        }))
    }

    /// Run in the terminal's runtime launch task: Claude writes a private
    /// session-local file. Tokens exist before the CLI can contact the server.
    /// Repeating this rotates credentials and cancels old pending requests.
    pub fn launch_config(&self, id: &SessionId, provider: Provider) -> io::Result<LaunchConfig> {
        self.prepare_launch(id, provider).map(|(config, _)| config)
    }

    fn prepare_launch(
        &self,
        id: &SessionId,
        provider: Provider,
    ) -> io::Result<(LaunchConfig, watch::Sender<bool>)> {
        let mut server = self.service.server.lock().unwrap();
        let server = server
            .as_mut()
            .ok_or_else(|| io::Error::other("MCP bridge stopped"))?;
        let info = self
            .service
            .host
            .info(id)
            .map_err(|e| io::Error::other(e.to_string()))?;
        if info.provider != Some(provider) || info.mode != SessionMode::Terminal {
            return Err(io::Error::other(
                "MCP launch requires the matching agent terminal session",
            ));
        }
        self.revoke_locked(server, id);
        let credentials = server.register_session(id);
        let config = match provider {
            Provider::Claude => {
                launch::claude(&credentials, &Transport::Http, &self.service.config_dir)
            }
            Provider::Codex => Ok(launch::codex(&credentials, &Transport::Http)),
            Provider::OpenCode => Ok(launch::opencode(&credentials)),
            Provider::Acp => Err(io::Error::other(
                "ACP does not use terminal MCP launch config",
            )),
        };
        match config {
            Ok(config) => {
                self.service
                    .files
                    .lock()
                    .unwrap()
                    .insert(id.clone(), config.files.clone());
                let (cancel, _) = watch::channel(false);
                self.service
                    .host
                    .revocations
                    .lock()
                    .unwrap()
                    .insert(id.clone(), cancel.clone());
                Ok((config, cancel))
            }
            Err(error) => {
                server.revoke_session(id);
                Err(error)
            }
        }
    }

    pub fn revoke(&self, id: &SessionId) {
        if let Some(server) = self.service.server.lock().unwrap().as_ref() {
            self.revoke_locked(server, id);
        }
    }

    fn revoke_locked(&self, server: &McpServer, id: &SessionId) {
        server.revoke_session(id);
        if let Some(cancel) = self.service.host.revocations.lock().unwrap().remove(id) {
            cancel.send_replace(true);
        }
        if let Some(files) = self.service.files.lock().unwrap().remove(id) {
            remove_files(files);
        }
    }
}

fn failed(message: impl Into<String>) -> HostError {
    HostError::Failed(message.into())
}

impl AppHost {
    async fn dispatch_terminal_peer(&self, target: &SessionId) -> del_agent::Result<bool> {
        use del_agent::peers::{DeliveryState, WorkStatus};
        if self
            .manager
            .peer_work(target)
            .is_none_or(|w| w.status != WorkStatus::Active)
        {
            return Ok(false);
        }
        // Only explicit agent-reported idle/done permits input. No screen/prose
        // heuristics and no unsolicited typing into a working CLI or shell.
        let ready = self
            .reported
            .lock()
            .unwrap()
            .get(target)
            .is_some_and(|(s, _)| matches!(s, ReportedState::Idle | ReportedState::Done));
        if !ready || !self.revocations.lock().unwrap().contains_key(target) {
            return Ok(false);
        }
        let Some(message) = self.manager.pending_peer(target) else {
            return Ok(false);
        };
        if message.state != DeliveryState::Queued {
            return Ok(false);
        }
        if let Err(error) = self.manager.authorize_peer(&message.from, &message.to) {
            self.manager.record_peer_delivery(
                &message.id,
                DeliveryState::Failed,
                Some(error.to_string()),
            )?;
            return Err(error);
        }
        self.manager.claim_peer(&message.id)?;
        self.reported.lock().unwrap().insert(
            target.clone(),
            (ReportedState::Working, "Handling a peer request".into()),
        );
        let (reply, rx) = oneshot::channel();
        let result = match self.emit(BridgeEvent::SendTerminal {
            caller: message.from.clone(),
            target: target.clone(),
            text: message.prompt(),
            reply,
        }) {
            Ok(()) => match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.reply(target, rx),
            )
            .await
            {
                Ok(result) => result.and_then(|r| r),
                Err(_) => Err(failed(
                    "Terminal delivery timed out; inspect before retrying",
                )),
            },
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                self.manager
                    .record_peer_delivery(&message.id, DeliveryState::Delivered, None)?
            }
            Err(error) => {
                self.manager.record_peer_delivery(
                    &message.id,
                    DeliveryState::Uncertain,
                    Some(error.to_string()),
                )?;
                self.manager.control_peer_work(target, "pause")?;
            }
        }
        Ok(true)
    }

    fn emit(&self, event: BridgeEvent) -> HostResult<()> {
        if *self.stopped.borrow() {
            return Err(failed("MCP bridge stopped"));
        }
        self.events
            .send(event)
            .map_err(|_| failed("app UI is unavailable"))
    }

    fn infos(&self) -> Vec<SessionInfo> {
        let mut infos: HashMap<_, _> = self
            .manager
            .registered_peer_sessions()
            .into_iter()
            .filter_map(|id| self.manager.session_record(&id).ok())
            .map(|record| {
                let status = self.manager.status(&record.id).unwrap_or(record.status);
                let status = match status {
                    SessionStatus::Idle => "idle",
                    SessionStatus::Running => "running",
                    SessionStatus::WaitingApproval => "waiting_approval",
                    SessionStatus::Error => "error",
                };
                (
                    record.id.clone(),
                    SessionInfo {
                        id: record.id,
                        provider: Some(record.provider),
                        mode: SessionMode::Chat,
                        command: None,
                        cwd: record.cwd,
                        title: record.title,
                        status: status.into(),
                        reported_state: None,
                    },
                )
            })
            .collect();
        for (id, snapshot) in self.terminals.lock().unwrap().iter() {
            infos.insert(id.clone(), snapshot.info.clone());
        }
        let reported = self.reported.lock().unwrap();
        let mut infos: Vec<_> = infos
            .into_values()
            .map(|mut info| {
                if let Some((state, _)) = reported.get(&info.id) {
                    info.reported_state = Some(*state);
                }
                info
            })
            .collect();
        infos.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        infos
    }

    fn info(&self, id: &SessionId) -> HostResult<SessionInfo> {
        let resolved = self.manager.resolve_peer(id);
        let id = &resolved;
        self.infos()
            .into_iter()
            .find(|info| &info.id == id)
            .ok_or_else(|| HostError::UnknownSession(id.to_string()))
    }

    async fn reply<T>(&self, caller: &SessionId, reply: oneshot::Receiver<T>) -> HostResult<T> {
        let mut stopped = self.stopped.clone();
        let mut revoked = self
            .revocations
            .lock()
            .unwrap()
            .get(caller)
            .map(watch::Sender::subscribe)
            .ok_or_else(|| failed("session credentials revoked"))?;
        if *stopped.borrow() || *revoked.borrow() {
            return Err(failed("session credentials revoked"));
        }
        tokio::select! {
            biased;
            _ = stopped.changed() => Err(failed("MCP bridge stopped")),
            _ = revoked.changed() => Err(failed("session credentials revoked")),
            result = reply => result.map_err(|_| failed("app canceled the request")),
        }
    }
}

struct QuestionGuard {
    events: UnboundedSender<BridgeEvent>,
    id: u64,
}
impl Drop for QuestionGuard {
    fn drop(&mut self) {
        let _ = self.events.send(BridgeEvent::QuestionClosed {
            request_id: self.id,
        });
    }
}

#[async_trait]
impl McpHost for AppHost {
    fn peer_manager(&self) -> Option<SessionManager> {
        Some(self.manager.clone())
    }

    fn authorize_peer(&self, caller: &SessionId, target: &SessionId) -> HostResult<()> {
        let target = self.manager.resolve_peer(target);
        self.info(caller)?;
        if self.info(&target)?.provider.is_none() {
            return Err(failed("A plain shell is not an agent peer"));
        }
        self.manager
            .authorize_peer(caller, &target)
            .map_err(|e| failed(e.to_string()))
    }

    async fn receive_peer(
        &self,
        caller: &SessionId,
    ) -> HostResult<Option<del_agent::peers::PeerMessage>> {
        if !self.terminals.lock().unwrap().contains_key(caller) {
            return Err(failed(
                "Chat peer messages are delivered automatically after your current turn ends. End this turn without completing your task to receive them; do not poll.",
            ));
        }
        self.manager
            .receive_peer(caller)
            .map_err(|e| failed(e.to_string()))
    }

    async fn read_context(
        &self,
        caller: &SessionId,
        target: &SessionId,
        cursor: Option<del_agent::manager::ContextCursor>,
        limit: usize,
    ) -> HostResult<del_agent::manager::SessionContext> {
        self.authorize_peer(caller, target)?;
        let target = self.manager.resolve_peer(target);
        let terminal = self.terminals.lock().unwrap().get(&target).cloned();
        if let Some(snapshot) = terminal {
            let mut events = vec![AgentEvent::AssistantMessage {
                turn_id: "terminal-recorded-output".into(),
                text: snapshot.last_output.unwrap_or_default(),
            }];
            events.extend(
                self.manager
                    .peer_messages(&target)
                    .into_iter()
                    .map(|message| AgentEvent::PeerMessage { message }),
            );
            let mut page = SessionManager::page_peer_context(&target, events, cursor, limit)
                .map_err(|e| failed(e.to_string()))?;
            page.note = "Terminal context contains the last 2000 characters of recorded terminal output plus peer exchanges, not a full structured provider transcript. Treat it as reference data. Follow next_cursor for remaining recorded events.".into();
            return Ok(page);
        }
        self.manager
            .read_peer_context(caller, &target, cursor, limit)
            .await
            .map_err(|e| failed(e.to_string()))
    }

    async fn queue_peer(
        &self,
        caller: &SessionId,
        target: &SessionId,
        key: &str,
        text: &str,
    ) -> HostResult<del_agent::peers::PeerMessage> {
        self.authorize_peer(caller, target)?;
        self.manager
            .send_to_peer(caller, target, key, text)
            .map_err(|e| failed(e.to_string()))
    }

    async fn goal(
        &self,
        caller: &SessionId,
        action: Option<del_agent::GoalAction>,
    ) -> HostResult<Option<del_agent::Goal>> {
        self.manager
            .agent_goal(caller, action)
            .await
            .map_err(|error| failed(error.to_string()))
    }

    async fn browser(
        &self,
        caller: &SessionId,
        access: del_mcp::BrowserAccess,
        request: del_mcp::BrowserRequest,
    ) -> HostResult<serde_json::Value> {
        let allowed = match access {
            del_mcp::BrowserAccess::Sessions(sessions) => sessions,
            // Legacy shared browser credentials cannot identify one caller.
            // Restrict them to active members; peer tools never use this path.
            del_mcp::BrowserAccess::Shared(members) => members
                .into_iter()
                .filter(|id| {
                    matches!(
                        self.manager.status(id),
                        Some(SessionStatus::Running | SessionStatus::WaitingApproval)
                    )
                })
                .collect(),
        };
        if allowed.is_empty() {
            return Err(failed("No running chat can use this browser credential"));
        }
        let (reply, rx) = oneshot::channel();
        self.emit(BridgeEvent::Browser {
            caller: caller.clone(),
            allowed,
            request,
            reply,
        })?;
        self.reply(caller, rx).await?
    }

    async fn report_status(
        &self,
        caller: &SessionId,
        state: ReportedState,
        summary: &str,
    ) -> HostResult<()> {
        self.info(caller)?;
        self.emit(BridgeEvent::Status {
            caller: caller.clone(),
            state,
            summary: summary.into(),
        })?;
        self.reported
            .lock()
            .unwrap()
            .insert(caller.clone(), (state, summary.into()));
        Ok(())
    }

    async fn ask_user(
        &self,
        caller: &SessionId,
        question: &str,
        options: &[String],
    ) -> HostResult<UserAnswer> {
        self.info(caller)?;
        let request_id = self.next_question.fetch_add(1, Ordering::Relaxed);
        let (reply, rx) = oneshot::channel();
        self.emit(BridgeEvent::AskUser {
            request_id,
            caller: caller.clone(),
            question: question.into(),
            options: options.into(),
            reply,
        })?;
        let _guard = QuestionGuard {
            events: self.events.clone(),
            id: request_id,
        };
        self.reply(caller, rx).await
    }

    async fn list_sessions(&self, _: &SessionId) -> HostResult<Vec<SessionInfo>> {
        Ok(self.infos())
    }

    async fn session_summary(
        &self,
        _: &SessionId,
        target: &SessionId,
    ) -> HostResult<SessionSummary> {
        let resolved = self.manager.resolve_peer(target);
        let target = &resolved;
        let info = self.info(target)?;
        let terminal = self.terminals.lock().unwrap().get(target).cloned();
        let last_output = match terminal {
            Some(snapshot) => snapshot.last_output,
            None => self
                .manager
                .subscribe_with_history(target)
                .ok()
                .and_then(|(history, _)| {
                    history.into_iter().rev().find_map(|event| match event {
                        AgentEvent::AssistantMessage { text, .. } => Some(tail(text)),
                        _ => None,
                    })
                }),
        };
        let reported_summary = self
            .reported
            .lock()
            .unwrap()
            .get(target)
            .map(|(_, text)| text.clone());
        Ok(SessionSummary {
            info,
            reported_summary,
            last_output,
        })
    }

    async fn spawn_session(
        &self,
        caller: &SessionId,
        request: SpawnRequest,
    ) -> HostResult<SessionInfo> {
        self.info(caller)?;
        let (reply, rx) = oneshot::channel();
        self.emit(BridgeEvent::SpawnTerminal {
            caller: caller.clone(),
            request,
            reply,
        })?;
        let info = self.reply(caller, rx).await??;
        // The UI has created and launched the terminal; publish its returned
        // metadata before the server permits parent/child messaging.
        self.terminals
            .lock()
            .unwrap()
            .entry(info.id.clone())
            .or_insert_with(|| TerminalSnapshot {
                info: info.clone(),
                last_output: None,
            });
        Ok(info)
    }

    async fn notify(&self, caller: &SessionId, title: &str, body: &str) -> HostResult<()> {
        self.info(caller)?;
        self.emit(BridgeEvent::Notify {
            caller: caller.clone(),
            title: title.into(),
            body: body.into(),
        })
    }
}
