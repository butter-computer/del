//! Supervisor owning many sessions across providers.

use crate::backend::*;
use crate::index::*;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

mod forks;
mod peers;
pub use forks::{ForkContext, ForkLifetime, ForkRecord, ForkResult, ForkSnapshot, ForkSource};
pub use peers::{ContextChunk, ContextCursor, SessionContext};

/// Everything the manager broadcasts to `subscribe_all` listeners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManagerEvent {
    PromptSent {
        session: SessionId,
    },
    TitleLoading {
        session: SessionId,
        loading: bool,
    },
    Event {
        session: SessionId,
        event: AgentEvent,
    },
    StatusChanged {
        session: SessionId,
        status: SessionStatus,
    },
}

/// Most events kept per session for late subscribers.
const HISTORY_LIMIT: usize = 50_000;

/// Drop the oldest tenth of an over-long history, keeping `SessionStarted`
/// and the first user message so the history still opens the conversation.
fn trim_history(history: &mut Vec<AgentEvent>) {
    if history.len() < HISTORY_LIMIT {
        return;
    }
    let start = usize::from(matches!(
        history.first(),
        Some(AgentEvent::SessionStarted { .. })
    ));
    let end = (start + HISTORY_LIMIT / 10).min(history.len());
    match history[start..end]
        .iter()
        .position(|e| matches!(e, AgentEvent::UserMessage { .. }))
    {
        Some(first_user) => {
            let pinned = start + first_user;
            history.drain(pinned + 1..end);
            history.drain(start..pinned);
        }
        None => {
            history.drain(start..end);
        }
    }
}

struct Live {
    handle: SessionHandle,
    config: SessionConfig,
    status: SessionStatus,
    pump: JoinHandle<()>,
    /// Re-broadcast of the backend's events, fed under the `live` lock together
    /// with `history`, so a subscriber that takes the lock sees every event
    /// exactly once: first in the history, then on the receiver.
    fanout: broadcast::Sender<AgentEvent>,
    history: Vec<AgentEvent>,
    /// Approval requests still waiting for an answer, so settling one of
    /// several does not report the session Running too early.
    pending_approvals: HashSet<String>,
}

const STANDBY_LIFETIME: Duration = Duration::from_secs(30 * 60);

struct Standby {
    config: SessionConfig,
    handle: SessionHandle,
    created: Instant,
}

struct Shared {
    peers: Mutex<crate::peers::PeerStore>,
    backends: HashMap<Provider, Arc<dyn AgentBackend>>,
    account_backends: Mutex<HashMap<(Provider, String), Arc<dyn AgentBackend>>>,
    browser_tools: Mutex<Option<BrowserTools>>,
    index: Mutex<SessionIndex>,
    live: Mutex<HashMap<SessionId, Live>>,
    resume_locks: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
    all_tx: broadcast::Sender<ManagerEvent>,
    standby: Mutex<Option<Standby>>,
    preparation: tokio::sync::Mutex<()>,
    title_generation: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
    /// Activity marker (live history length, or `usize::MAX` when not live) at
    /// the last provider title read, so unchanged sessions are not re-polled.
    title_polls: Mutex<HashMap<SessionId, usize>>,
}

/// Cheap to clone; all clones share state.
#[derive(Clone)]
pub struct SessionManager {
    shared: Arc<Shared>,
}

/// `approvals_pending`: whether any approval is still unanswered after `ev`.
fn next_status(current: SessionStatus, ev: &AgentEvent, approvals_pending: bool) -> SessionStatus {
    match ev {
        AgentEvent::TurnStarted { .. } => SessionStatus::Running,
        AgentEvent::ApprovalRequested { .. } => SessionStatus::WaitingApproval,
        AgentEvent::ApprovalResolved { .. }
            if current == SessionStatus::WaitingApproval && !approvals_pending =>
        {
            SessionStatus::Running
        }
        AgentEvent::Error {
            recoverable: false, ..
        } => SessionStatus::Error,
        AgentEvent::TurnCompleted { .. } if current != SessionStatus::Error => SessionStatus::Idle,
        _ => current,
    }
}

impl Shared {
    fn set_status(&self, id: &SessionId, status: SessionStatus) {
        self.transition(id, |_| Some(status));
    }

    /// Compare-and-set the live status under the `live` lock, so a decision
    /// based on the current status can never write back a stale one. Returns
    /// whether the status changed.
    fn transition(
        &self,
        id: &SessionId,
        decide: impl FnOnce(SessionStatus) -> Option<SessionStatus>,
    ) -> bool {
        let mut live = self.live.lock().unwrap();
        let Some(l) = live.get_mut(id) else {
            return false;
        };
        match decide(l.status) {
            Some(next) if next != l.status => {
                l.status = next;
                self.publish_status(id, next);
                true
            }
            _ => false,
        }
    }

    /// Persist and broadcast a status. Callers hold the `live` lock so
    /// concurrent changes are published in the order they were applied.
    fn publish_status(&self, id: &SessionId, status: SessionStatus) {
        let _ = self.index.lock().unwrap().update(id, |r| {
            r.status = status;
            r.updated_at_ms = now_ms();
        });
        let _ = self.all_tx.send(ManagerEvent::StatusChanged {
            session: id.clone(),
            status,
        });
    }
}

impl SessionManager {
    pub async fn list_commands(
        &self,
        provider: Provider,
        cwd: &Path,
    ) -> Result<Vec<crate::ComposerCommand>> {
        self.backend(provider)?.list_commands(cwd).await
    }
    pub async fn list_models(
        &self,
        provider: Provider,
        cwd: &std::path::Path,
    ) -> Result<Vec<crate::ModelInfo>> {
        self.shared
            .backends
            .get(&provider)
            .ok_or(AgentError::NoBackend(provider))?
            .list_models(cwd)
            .await
    }

    /// Read a provider conversation name, including terminal-owned conversations.
    pub async fn session_display_title(
        &self,
        provider: Provider,
        id: &SessionId,
        cwd: &Path,
    ) -> Result<Option<String>> {
        self.backend(provider)?.session_display_title(id, cwd).await
    }

    pub async fn account_session_display_title(
        &self,
        provider: Provider,
        account: Option<&str>,
        id: &SessionId,
        cwd: &Path,
    ) -> Result<Option<String>> {
        self.account_backend(provider, account)?
            .session_display_title(id, cwd)
            .await
    }

    /// Refresh provider-owned names without changing session order or live state.
    ///
    /// Never starts a provider just to read a name: only live sessions, and
    /// saved ones sharing a live session's provider, account and folder (whose
    /// provider process is already running), are asked. A session is asked
    /// again only after new activity, so idle or nameless sessions are not
    /// polled forever.
    pub async fn refresh_titles(&self) -> Result<bool> {
        let records = self.list_sessions();
        let activity: HashMap<SessionId, usize> = self
            .shared
            .live
            .lock()
            .unwrap()
            .iter()
            .map(|(id, l)| (id.clone(), l.history.len()))
            .collect();
        let warm: std::collections::HashSet<_> = records
            .iter()
            .filter(|r| activity.contains_key(&r.id))
            .map(|r| (r.provider, r.account_id.clone(), r.cwd.clone()))
            .collect();
        let mut changed = false;
        for record in records {
            if record.title_turns > 0 {
                continue;
            }
            let marker = match activity.get(&record.id) {
                Some(&len) => len,
                None if warm.contains(&(
                    record.provider,
                    record.account_id.clone(),
                    record.cwd.clone(),
                )) =>
                {
                    usize::MAX
                }
                None => continue,
            };
            if self
                .shared
                .title_polls
                .lock()
                .unwrap()
                .insert(record.id.clone(), marker)
                == Some(marker)
            {
                continue;
            }
            let Ok(backend) = self.account_backend(record.provider, record.account_id.as_deref())
            else {
                continue;
            };
            let Ok(Ok(Some(title))) = tokio::time::timeout(
                Duration::from_secs(3),
                backend.session_title(&record.id, &record.cwd),
            )
            .await
            else {
                continue;
            };
            let Some(title) = crate::titles::clean_provider(&title) else {
                continue;
            };
            let mut index = self.shared.index.lock().unwrap();
            if index
                .get(&record.id)
                .is_some_and(|r| r.title_turns == 0 && r.title.as_deref() != Some(title.as_str()))
            {
                index.update(&record.id, |r| r.title = Some(title))?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// One lightweight request on the first sent prompt, then every six user turns.
    /// Serialized per session, separately from foreground sends; failed generation leaves the
    /// fallback title intact and can retry on a later prompt.
    pub async fn generate_title(&self, id: &SessionId) -> Result<bool> {
        if self.is_temporary(id) {
            return Ok(false);
        }
        let lock = self
            .shared
            .title_generation
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _generation = lock.lock().await;
        let record = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        let (turns, context) = {
            let live = self.shared.live.lock().unwrap();
            let Some(session) = live.get(id) else {
                return Ok(false);
            };
            crate::titles::context(&session.history)
        };
        if !crate::titles::due(record.title_turns, turns) {
            return Ok(false);
        }
        let backend = self.account_backend(record.provider, record.account_id.as_deref())?;
        let _ = self.shared.all_tx.send(ManagerEvent::TitleLoading {
            session: id.clone(),
            loading: true,
        });
        let result = async {
            let title = backend.generate_session_title(id, &context).await?;
            let Some(title) = title.and_then(|s| crate::titles::clean(&s)) else {
                return Ok(false);
            };
            let mut index = self.shared.index.lock().unwrap();
            if index.get(id).is_none() {
                return Ok(false);
            }
            index.update(id, |record| {
                record.title = Some(title);
                record.title_turns = turns;
            })?;
            Ok(true)
        }
        .await;
        let _ = self.shared.all_tx.send(ManagerEvent::TitleLoading {
            session: id.clone(),
            loading: false,
        });
        result
    }

    /// `index_path` is typically [`default_index_path`].
    pub fn new(
        index_path: impl AsRef<Path>,
        backends: impl IntoIterator<Item = Arc<dyn AgentBackend>>,
    ) -> Result<Self> {
        let (all_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let index = SessionIndex::open(index_path.as_ref())?;
        let mut peers =
            crate::peers::PeerStore::open(index_path.as_ref().with_extension("peers.json"))?;
        for record in index.list() {
            if peers.project(&record.id).is_none() && record.cwd.is_absolute() {
                peers.register(&record.id, &record.cwd)?;
            }
        }
        Ok(Self {
            shared: Arc::new(Shared {
                backends: backends.into_iter().map(|b| (b.provider(), b)).collect(),
                account_backends: Mutex::new(HashMap::new()),
                browser_tools: Mutex::new(None),
                index: Mutex::new(index),
                peers: Mutex::new(peers),
                live: Mutex::new(HashMap::new()),
                resume_locks: Mutex::new(HashMap::new()),
                all_tx,
                standby: Mutex::new(None),
                preparation: tokio::sync::Mutex::new(()),
                title_generation: Mutex::new(HashMap::new()),
                title_polls: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn register_account(&self, account: String, backend: Arc<dyn AgentBackend>) {
        if let Some(tools) = self.shared.browser_tools.lock().unwrap().clone() {
            backend.set_browser_tools(tools);
        }
        self.shared
            .account_backends
            .lock()
            .unwrap()
            .insert((backend.provider(), account), backend);
    }

    pub fn unregister_account(
        &self,
        provider: Provider,
        account: &str,
    ) -> Option<Arc<dyn AgentBackend>> {
        self.shared
            .account_backends
            .lock()
            .unwrap()
            .remove(&(provider, account.to_owned()))
    }

    fn account_backend(
        &self,
        provider: Provider,
        account: Option<&str>,
    ) -> Result<Arc<dyn AgentBackend>> {
        match account {
            None => self.backend(provider),
            Some(account) => self
                .shared
                .account_backends
                .lock()
                .unwrap()
                .get(&(provider, account.to_owned()))
                .cloned()
                .ok_or_else(|| {
                    AgentError::Backend(
                        "This account is unavailable. Reconnect it in Settings → Providers.".into(),
                    )
                }),
        }
    }

    fn session_backend(&self, id: &SessionId) -> Result<Arc<dyn AgentBackend>> {
        let record = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        self.account_backend(record.provider, record.account_id.as_deref())
    }

    pub async fn account_models(
        &self,
        provider: Provider,
        account: Option<&str>,
        cwd: &Path,
    ) -> Result<Vec<crate::ModelInfo>> {
        self.account_backend(provider, account)?
            .list_models(cwd)
            .await
    }

    pub async fn account_commands(
        &self,
        provider: Provider,
        account: Option<&str>,
        cwd: &Path,
    ) -> Result<Vec<crate::ComposerCommand>> {
        self.account_backend(provider, account)?
            .list_commands(cwd)
            .await
    }

    fn backend(&self, p: Provider) -> Result<Arc<dyn AgentBackend>> {
        self.shared
            .backends
            .get(&p)
            .cloned()
            .ok_or(AgentError::NoBackend(p))
    }

    fn live_provider(&self, id: &SessionId) -> Result<Provider> {
        self.shared
            .live
            .lock()
            .unwrap()
            .get(id)
            .map(|l| l.handle.provider)
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    /// Start pumping a backend's events. Backends cannot emit `SessionStarted`
    /// to anyone (no subscriber exists yet), so the manager emits the canonical
    /// one here, first in the session's history; any the backend emits itself
    /// is dropped. Returns the handle without its restored snapshot, which now
    /// lives only in the session history.
    fn attach(&self, mut handle: SessionHandle, config: SessionConfig) -> SessionHandle {
        let id = handle.id.clone();
        let mut rx = handle.subscribe();
        let mut restored = handle.take_history().to_vec();
        let messages = self.peer_messages(&id);
        restored.retain(|event| !matches!(event, AgentEvent::UserMessage { text } if text == peers::CONTINUE_WORK));
        for event in &mut restored {
            // Snapshots may predate native-ID rebinding or delivery updates.
            // The durable mailbox owns the canonical endpoints and receipt.
            if let AgentEvent::PeerMessage { message } = event
                && let Some(current) = messages.iter().find(|m| m.id == message.id)
            {
                *message = current.clone();
            }
            if let AgentEvent::UserMessage { text } = event
                && let Some(message) = messages.iter().find(|m| {
                    m.to == id && m.matches_prompt(text, |endpoint| self.resolve_peer(endpoint))
                })
            {
                *event = AgentEvent::PeerMessage {
                    message: message.clone(),
                };
            }
        }
        for message in messages {
            if !restored.iter().any(
                |e| matches!(e, AgentEvent::PeerMessage { message: old } if old.id == message.id),
            ) {
                restored.push(AgentEvent::PeerMessage { message });
            }
        }
        // Publish the attachment before the pump can consume its first event.
        let mut live = self.shared.live.lock().unwrap();
        let old = live.remove(&id);
        let started = AgentEvent::SessionStarted {
            session_id: id.clone(),
        };
        let (fanout, history) = if let Some(old) = old {
            old.pump.abort();
            (old.fanout, old.history)
        } else {
            let (fanout, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
            let mut history = vec![started.clone()];
            history.extend_from_slice(&restored);
            let _ = self.shared.all_tx.send(ManagerEvent::Event {
                session: id.clone(),
                event: started,
            });
            (fanout, history)
        };
        drop(restored);
        let shared = self.shared.clone();
        let pump_id = id.clone();
        let pump = tokio::spawn(async move {
            // Set after events were lost: the turn state is unknown until the
            // stream shows whether a turn is still running.
            let mut resyncing = false;
            loop {
                let (event, status) = match rx.recv().await {
                    Ok(AgentEvent::SessionStarted { .. }) => continue,
                    Ok(event) => {
                        let mut status = None;
                        if resyncing {
                            match event {
                                AgentEvent::TurnStarted { .. }
                                | AgentEvent::TurnCompleted { .. } => resyncing = false,
                                AgentEvent::AssistantTextDelta { .. }
                                | AgentEvent::ReasoningDelta { .. }
                                | AgentEvent::ToolCallStarted { .. }
                                | AgentEvent::ToolCallUpdated { .. } => {
                                    status = Some(SessionStatus::Running);
                                }
                                _ => {}
                            }
                        }
                        (event, status)
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        // A lost `TurnCompleted` would leave the session stuck
                        // Running. Assume the turn ended; streaming output that
                        // follows marks it Running again.
                        resyncing = true;
                        (
                            AgentEvent::Error {
                                message: format!(
                                    "Del fell behind and skipped {skipped} updates from this session. Some output may be missing."
                                ),
                                recoverable: true,
                            },
                            Some(SessionStatus::Idle),
                        )
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                {
                    let mut live = shared.live.lock().unwrap();
                    let Some(l) = live.get_mut(&pump_id) else {
                        continue;
                    };
                    trim_history(&mut l.history);
                    match &event {
                        AgentEvent::ApprovalRequested { id, .. } => {
                            l.pending_approvals.insert(id.clone());
                        }
                        AgentEvent::ApprovalResolved { id } => {
                            l.pending_approvals.remove(id);
                        }
                        AgentEvent::TurnCompleted { .. } => l.pending_approvals.clear(),
                        _ => {}
                    }
                    l.history.push(event.clone());
                    let _ = l.fanout.send(event.clone());
                    let next = match status {
                        Some(SessionStatus::Idle)
                            if !matches!(
                                l.status,
                                SessionStatus::Running | SessionStatus::WaitingApproval
                            ) =>
                        {
                            l.status
                        }
                        Some(SessionStatus::Running) if l.status != SessionStatus::Idle => l.status,
                        Some(status) => status,
                        None => next_status(l.status, &event, !l.pending_approvals.is_empty()),
                    };
                    // Status first so a listener seeing the event sees a consistent status.
                    if next != l.status {
                        l.status = next;
                        shared.publish_status(&pump_id, next);
                    }
                }
                let _ = shared.all_tx.send(ManagerEvent::Event {
                    session: pump_id.clone(),
                    event,
                });
            }
        });
        live.insert(
            id,
            Live {
                handle: handle.clone(),
                config,
                status: SessionStatus::Idle,
                pump,
                fanout,
                history,
                pending_approvals: HashSet::new(),
            },
        );
        handle
    }

    /// Keep one unused session ready, without publishing a chat or sending a prompt.
    /// Preparation never holds the lock used by the send path.
    pub async fn prepare_session(&self, config: SessionConfig) -> Result<()> {
        let _preparing = self.shared.preparation.lock().await;
        if self
            .shared
            .standby
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.config == config && s.created.elapsed() < STANDBY_LIFETIME)
        {
            return Ok(());
        }
        let started = Instant::now();
        let handle = self
            .backend(config.provider)?
            .start_session(config.clone())
            .await?;
        let previous = self.shared.standby.lock().unwrap().replace(Standby {
            config,
            handle,
            created: Instant::now(),
        });
        if let Some(previous) = previous {
            let _ = self
                .backend(previous.config.provider)?
                .close(&previous.handle.id)
                .await;
        }
        eprintln!(
            "[del startup] standby prepared in {}ms",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    pub async fn clear_prepared_session(&self) {
        let _preparing = self.shared.preparation.lock().await;
        let previous = self.shared.standby.lock().unwrap().take();
        if let Some(previous) = previous
            && let Ok(backend) = self.backend(previous.config.provider)
        {
            let _ = backend.close(&previous.handle.id).await;
        }
    }

    pub async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.start_account_session(config, None).await
    }

    pub async fn start_account_session(
        &self,
        config: SessionConfig,
        account_id: Option<String>,
    ) -> Result<SessionHandle> {
        let applied_config = config.clone();
        let (prepared, expired) = {
            let mut slot = self.shared.standby.lock().unwrap();
            if account_id.is_none()
                && slot
                    .as_ref()
                    .is_some_and(|s| s.config == config && s.created.elapsed() < STANDBY_LIFETIME)
            {
                (slot.take().map(|s| s.handle), None)
            } else if slot
                .as_ref()
                .is_some_and(|s| s.created.elapsed() >= STANDBY_LIFETIME)
            {
                (None, slot.take())
            } else {
                (None, None)
            }
        };
        // Release an expired standby's process without delaying this start.
        if let Some(expired) = expired
            && let Ok(backend) = self.backend(expired.config.provider)
        {
            tokio::spawn(async move {
                let _ = backend.close(&expired.handle.id).await;
            });
        }
        let started = Instant::now();
        let warm = prepared.is_some();
        let backend = self.account_backend(config.provider, account_id.as_deref())?;
        let handle = match prepared {
            Some(handle) => handle,
            None => backend.start_session(config.clone()).await?,
        };
        eprintln!(
            "[del startup] session ready warm={warm} elapsed={}ms",
            started.elapsed().as_millis()
        );
        let now = now_ms();
        let recorded = self.shared.index.lock().unwrap().upsert(SessionRecord {
            fork: None,
            id: handle.id.clone(),
            provider: config.provider,
            account_id,
            cwd: config.cwd,
            model: config.model,
            reasoning_effort: config.reasoning_effort,
            service_tier: config.service_tier,
            permission_mode: config.permission_mode,
            title: None,
            title_turns: 0,
            created_at_ms: now,
            updated_at_ms: now,
            status: SessionStatus::Idle,
        });
        if let Err(error) = recorded {
            // Never leave a provider process running for a session nobody can reach.
            let _ = backend.close(&handle.id).await;
            return Err(error);
        }
        if applied_config.cwd.is_absolute()
            && let Err(error) = self.set_peer_project(&handle.id, &applied_config.cwd)
        {
            let _ = backend.close(&handle.id).await;
            return Err(error);
        }
        Ok(self.attach(handle, applied_config))
    }

    /// Reattach a session from the index (or return the live one).
    pub async fn resume(&self, id: &SessionId) -> Result<SessionHandle> {
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _resuming = lock.lock().await;
        self.resume_locked(id).await
    }

    /// Caller holds the session's resume/send lock. Failed sessions keep their
    /// history and subscribers, but their provider process must be reattached.
    async fn resume_locked(&self, id: &SessionId) -> Result<SessionHandle> {
        if let Some(l) = self.shared.live.lock().unwrap().get(id)
            && l.status != SessionStatus::Error
        {
            return Ok(l.handle.clone());
        }
        if self.is_temporary(id) {
            return Err(AgentError::Backend("This temporary chat lost its provider connection. Open a new side chat to fork the main conversation again.".into()));
        }
        let record = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        let handle = self
            .account_backend(record.provider, record.account_id.as_deref())?
            .resume(record.config(), id)
            .await?;
        let handle = self.attach(handle, record.config());
        // `attach` starts live sessions Idle; clear a persisted Error too.
        if record.status != SessionStatus::Idle {
            let _live = self.shared.live.lock().unwrap();
            self.shared.publish_status(id, SessionStatus::Idle);
        }
        Ok(handle)
    }

    pub async fn control_goal(&self, id: &SessionId, action: crate::GoalAction) -> Result<()> {
        self.live_provider(id)?;
        self.session_backend(id)?.control_goal(id, action).await
    }

    pub async fn agent_goal(
        &self,
        id: &SessionId,
        action: Option<crate::GoalAction>,
    ) -> Result<Option<crate::Goal>> {
        self.session_backend(id)?.agent_goal(id, action).await
    }

    async fn resolve_command(
        &self,
        backend: &dyn AgentBackend,
        cwd: &Path,
        input: &UserInput,
    ) -> Result<Option<crate::ComposerCommand>> {
        let catalog = if !input.skills.is_empty()
            || input.command.is_some()
            || crate::split_command(&input.text).is_some()
        {
            backend.list_commands(cwd).await?
        } else {
            Vec::new()
        };
        for skill in &input.skills {
            if skill.kind != crate::CommandKind::Skill
                || !catalog.iter().any(|available| {
                    available.kind == crate::CommandKind::Skill
                        && available.name == skill.name
                        && available.target == skill.target
                })
            {
                return Err(AgentError::Backend(format!(
                    "Selected skill /{} is no longer available. Select it again from the / menu.",
                    skill.name
                )));
            }
        }
        let selected_goal = input
            .command
            .as_ref()
            .filter(|c| c.kind == crate::CommandKind::Goal);
        let parsed = crate::split_command(&input.text);
        let command =
            if let Some((name, _)) = selected_goal.map(|c| (c.name.as_str(), "")).or(parsed) {
                catalog.into_iter().find(|command| {
                    command.name == name
                        && input
                            .command
                            .as_ref()
                            .is_none_or(|selected| selected.target == command.target)
                })
            } else {
                None
            };
        if input.command.is_some() && command.is_none() {
            return Err(AgentError::Backend(
                "The selected command is no longer available. Select it again from the / menu."
                    .into(),
            ));
        }
        Ok(command)
    }

    pub async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        self.send_prompt_kind(id, input, peers::SendOrigin::User)
            .await
    }

    async fn send_prompt_kind(
        &self,
        id: &SessionId,
        input: UserInput,
        origin: peers::SendOrigin<'_>,
    ) -> Result<()> {
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _sending = lock.lock().await;
        if !matches!(origin, peers::SendOrigin::User)
            && matches!(
                self.status(id),
                Some(SessionStatus::Running | SessionStatus::WaitingApproval)
            )
        {
            return Err(AgentError::Busy("a turn is already running".into()));
        }
        if matches!(origin, peers::SendOrigin::User) && self.peer_blocks_user_queue(id) {
            return Err(AgentError::Busy(
                "current work and peer requests must finish before queued user messages".into(),
            ));
        }
        if self.status(id) == Some(SessionStatus::Error) {
            self.resume_locked(id).await?;
        }
        self.live_provider(id)?;
        let config = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?
            .config();
        let changed = self
            .shared
            .live
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|live| live.config != config);
        if changed {
            if self.status(id) == Some(SessionStatus::Running)
                || self.status(id) == Some(SessionStatus::WaitingApproval)
            {
                return Err(AgentError::Busy(
                    "settings apply after the current turn finishes".into(),
                ));
            }
            match self
                .session_backend(id)?
                .reconfigure(config.clone(), id)
                .await
            {
                Ok(handle) => {
                    self.attach(handle, config.clone());
                }
                Err(error) => {
                    // The old process may already be closed. Keep the history,
                    // mark the session failed, and retry on the next send (a
                    // default reconfigure tolerates the missing session).
                    if !matches!(error, AgentError::Busy(_)) {
                        self.shared.set_status(id, SessionStatus::Error);
                    }
                    return Err(error);
                }
            }
        }
        let title: String = input.text.trim().chars().take(60).collect();
        let backend = self.session_backend(id)?;
        let command = self.resolve_command(&*backend, &config.cwd, &input).await?;
        // Local commands can finish before the call returns. Never overwrite
        // their completion (or a very fast provider result) with Running.
        let previous = self.status(id).unwrap_or(SessionStatus::Idle);
        let title_input = match origin {
            peers::SendOrigin::Peer(message_id) => {
                self.shared.peers.lock().unwrap().claim(message_id)?;
                None
            }
            peers::SendOrigin::Continuation => {
                let peers = self.shared.peers.lock().unwrap();
                if peers.pending(id).is_some()
                    || peers
                        .work(id)
                        .is_none_or(|w| w.status != crate::peers::WorkStatus::Active)
                {
                    return Err(AgentError::Busy(
                        "Peer work changed before continuation".into(),
                    ));
                }
                None
            }
            peers::SendOrigin::User => {
                // Reconfiguration/command discovery may have awaited while a
                // peer arrived. Arbitrate again immediately before submission.
                if self.peer_blocks_user_queue(id) {
                    return Err(AgentError::Busy("Current work must finish first".into()));
                }
                Some(AgentEvent::UserMessage {
                    text: input.text.clone(),
                })
            }
        };
        let title_position = {
            let mut live = self.shared.live.lock().unwrap();
            let session = live
                .get_mut(id)
                .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
            let position = session.history.len();
            if let Some(event) = &title_input {
                session.history.push(event.clone());
            }
            position
        };
        self.shared.set_status(id, SessionStatus::Running);
        let ordinary_prompt = command.is_none();
        let context = self.pending_fork_context(id);
        let result = if let Some(command) = command {
            backend.invoke_command(id, command, input).await
        } else {
            backend
                .send_prompt(id, forks::with_context(input, context.as_deref()))
                .await
        };
        if let Err(error) = result {
            if let Some(session) = self.shared.live.lock().unwrap().get_mut(id)
                && title_input
                    .as_ref()
                    .is_some_and(|event| session.history.get(title_position) == Some(event))
            {
                session.history.remove(title_position);
            }
            // Only undo our own Running; events may already have moved on.
            self.shared.transition(id, |current| {
                (current == SessionStatus::Running).then_some(previous)
            });
            if let peers::SendOrigin::Peer(message_id) = origin {
                let state = if matches!(error, AgentError::Busy(_)) {
                    crate::peers::DeliveryState::Queued
                } else {
                    crate::peers::DeliveryState::Uncertain
                };
                let message = self.shared.peers.lock().unwrap().set_delivery(
                    message_id,
                    state,
                    Some(error.to_string()),
                )?;
                self.publish_peer(&message);
            }
            return Err(error);
        }
        if ordinary_prompt {
            self.consume_fork_context(id);
        }
        let _ = self.shared.index.lock().unwrap().update(id, |r| {
            if matches!(origin, peers::SendOrigin::User) && r.title.is_none() && !title.is_empty() {
                r.title = Some(title);
            }
        });
        let _ = self.shared.all_tx.send(ManagerEvent::PromptSent {
            session: id.clone(),
        });
        Ok(())
    }

    /// Move an idle conversation to another account without changing its identity.
    /// The transfer runs after closing the old writer, under the same lock as sends.
    pub async fn switch_account<F, Fut>(
        &self,
        id: &SessionId,
        account: Option<String>,
        transfer: F,
    ) -> Result<()>
    where
        F: FnOnce(SessionRecord) -> Fut + Send,
        Fut: std::future::Future<Output = Result<Option<std::path::PathBuf>>> + Send,
    {
        self.resume(id).await?;
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _switching = lock.lock().await;
        if matches!(
            self.status(id),
            Some(SessionStatus::Running | SessionStatus::WaitingApproval)
        ) {
            return Err(AgentError::Busy(
                "Wait for the response to finish before switching accounts.".into(),
            ));
        }
        let record = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        if record.account_id == account {
            return Ok(());
        }
        let old = self.session_backend(id)?;
        let target = self.account_backend(record.provider, account.as_deref())?;
        old.close(id).await?;
        let result = async {
            let path = transfer(record.clone()).await?;
            let handle = target
                .resume_transferred(record.config(), id, path.as_deref())
                .await?;
            if handle.id != *id {
                let _ = target.close(&handle.id).await;
                return Err(AgentError::Backend(
                    "The provider changed this conversation's identity during account transfer."
                        .into(),
                ));
            }
            self.shared
                .index
                .lock()
                .unwrap()
                .update(id, |r| r.account_id = account)?;
            Ok(handle)
        }
        .await;
        match result {
            Ok(handle) => {
                self.attach(handle, record.config());
                Ok(())
            }
            Err(error) => {
                let _ = target.close(id).await;
                match old.resume(record.config(), id).await {
                    Ok(handle) => {
                        self.attach(handle, record.config());
                    }
                    Err(restore) => {
                        return Err(AgentError::Backend(format!(
                            "{error}; could not reconnect the original account: {restore}"
                        )));
                    }
                }
                Err(error)
            }
        }
    }

    /// Persist next-turn settings without creating or selecting another session.
    pub fn set_model_settings(
        &self,
        id: &SessionId,
        model: Option<String>,
        effort: Option<String>,
        tier: Option<String>,
    ) -> Result<()> {
        self.shared.index.lock().unwrap().update(id, |record| {
            record.model = model;
            record.reasoning_effort = effort;
            record.service_tier = tier;
        })
    }

    /// Persist permissions for the next launch or idle-turn reconfiguration.
    pub fn set_browser_tools(&self, tools: crate::backend::BrowserTools) {
        *self.shared.browser_tools.lock().unwrap() = Some(tools.clone());
        for backend in self.shared.account_backends.lock().unwrap().values() {
            backend.set_browser_tools(tools.clone());
        }
        for backend in self.shared.backends.values() {
            backend.set_browser_tools(tools.clone());
        }
    }

    pub fn set_permission_mode(&self, id: &SessionId, mode: PermissionMode) -> Result<()> {
        let mut index = self.shared.index.lock().unwrap();
        let record = index
            .get(id)
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        if record.permission_mode == mode {
            return Ok(());
        }
        index.update(id, |record| record.permission_mode = mode)
    }

    pub async fn steer(&self, id: &SessionId, text: String) -> Result<()> {
        self.steer_input(id, UserInput::text(text)).await
    }

    pub async fn steer_input(&self, id: &SessionId, input: UserInput) -> Result<()> {
        if self.pending_fork_context(id).is_some() {
            return Err(AgentError::Busy(
                "updated fork context is waiting; queue a normal message instead of steering"
                    .into(),
            ));
        }
        let backend = self.session_backend(id)?;
        if crate::split_command(&input.text).is_some() || !input.skills.is_empty() {
            let cwd = self
                .shared
                .index
                .lock()
                .unwrap()
                .get(id)
                .ok_or_else(|| AgentError::UnknownSession(id.clone()))?
                .cwd
                .clone();
            if let Some(command) = self.resolve_command(&*backend, &cwd, &input).await? {
                // Backends either execute a native control immediately or return
                // Busy, leaving the command queued for the end of this turn.
                return backend.invoke_command(id, command, input).await;
            }
        }
        let text = input.text.clone();
        backend.steer_input(id, input).await?;
        if let Some(session) = self.shared.live.lock().unwrap().get_mut(id) {
            session.history.push(AgentEvent::UserMessage { text });
        }
        let _ = self.shared.all_tx.send(ManagerEvent::PromptSent {
            session: id.clone(),
        });
        Ok(())
    }

    pub async fn interrupt(&self, id: &SessionId) -> Result<()> {
        self.shared.peers.lock().unwrap().pause(id)?;
        self.session_backend(id)?.interrupt(id).await
    }

    pub async fn respond_approval(
        &self,
        id: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        self.session_backend(id)?
            .respond_approval(id, approval_id, response)
            .await?;
        // The turn may already have finished (or failed) while we awaited, and
        // other requests may still be waiting.
        let mut live = self.shared.live.lock().unwrap();
        if let Some(l) = live.get_mut(id) {
            l.pending_approvals.remove(approval_id);
            if l.status == SessionStatus::WaitingApproval && l.pending_approvals.is_empty() {
                l.status = SessionStatus::Running;
                self.shared.publish_status(id, SessionStatus::Running);
            }
        }
        Ok(())
    }

    /// End the live session. Its index record stays so it can be resumed.
    pub async fn close(&self, id: &SessionId) -> Result<()> {
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _closing = lock.lock().await;
        // Detach first so a failing backend lookup or close cannot leak the pump.
        let Some(live) = self.shared.live.lock().unwrap().remove(id) else {
            return Ok(());
        };
        live.pump.abort();
        let provider = live.handle.provider;
        drop(live);
        // A live session must always be closable, even when its index record
        // is gone: fall back to the provider the live handle was started on.
        let backend = match self.session_backend(id) {
            Err(AgentError::UnknownSession(_)) => self.backend(provider),
            other => other,
        };
        let res = match backend {
            Ok(backend) => match backend.close(id).await {
                // Already gone on the provider side: closing is done.
                Err(AgentError::UnknownSession(_)) => Ok(()),
                other => other,
            },
            Err(e) => Err(e),
        };
        let changed = {
            let mut index = self.shared.index.lock().unwrap();
            let changed = index
                .get(id)
                .is_some_and(|r| r.status != SessionStatus::Idle);
            if changed {
                let _ = index.update(id, |r| r.status = SessionStatus::Idle);
            }
            changed
        };
        if changed {
            let _ = self.shared.all_tx.send(ManagerEvent::StatusChanged {
                session: id.clone(),
                status: SessionStatus::Idle,
            });
        }
        res
    }

    /// Close if live, then forget the session entirely.
    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        if self.is_temporary(id) && self.is_live(id) {
            let _ = self.interrupt(id).await;
        }
        if self.is_live(id) {
            let _ = self.close(id).await;
        }
        self.shared.resume_locks.lock().unwrap().remove(id);
        self.shared.title_generation.lock().unwrap().remove(id);
        self.shared.title_polls.lock().unwrap().remove(id);
        self.forget_peer_session(id)?;
        self.shared.index.lock().unwrap().remove(id)
    }

    pub async fn session_process(&self, id: &SessionId) -> Option<(u32, bool)> {
        self.live_provider(id).ok()?;
        self.session_backend(id).ok()?.session_process(id).await
    }

    pub fn is_live(&self, id: &SessionId) -> bool {
        self.shared.live.lock().unwrap().contains_key(id)
    }

    /// Live status; `None` when the session isn't attached.
    pub fn status(&self, id: &SessionId) -> Option<SessionStatus> {
        self.shared.live.lock().unwrap().get(id).map(|l| l.status)
    }

    /// All known sessions, newest first (live and persisted).
    pub fn list_sessions(&self) -> Vec<SessionRecord> {
        self.shared.index.lock().unwrap().list()
    }

    /// Events of one live session from now on.
    pub fn subscribe(&self, id: &SessionId) -> Result<broadcast::Receiver<AgentEvent>> {
        self.subscribe_with_history(id).map(|(_, rx)| rx)
    }

    /// Everything the session has emitted since it was attached (starting with
    /// `SessionStarted`) plus a receiver for what comes next, with no gap or
    /// overlap between the two. Use this when the subscriber may arrive after
    /// the session already produced events.
    pub fn subscribe_with_history(
        &self,
        id: &SessionId,
    ) -> Result<(Vec<AgentEvent>, broadcast::Receiver<AgentEvent>)> {
        self.shared
            .live
            .lock()
            .unwrap()
            .get(id)
            .map(|l| (l.history.clone(), l.fanout.subscribe()))
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    /// Events and status changes of every session.
    pub fn subscribe_all(&self) -> broadcast::Receiver<ManagerEvent> {
        self.shared.all_tx.subscribe()
    }

    /// Close every session and kill all backend child processes. Idempotent.
    /// Sessions close concurrently: each may wait out a provider's grace period.
    pub async fn shutdown(&self) {
        // Save first: the caller may stop waiting on a slow provider.
        self.flush_index().await;
        self.clear_prepared_session().await;
        let ids: Vec<SessionId> = self.shared.live.lock().unwrap().keys().cloned().collect();
        let mut closing = tokio::task::JoinSet::new();
        for id in ids {
            let manager = self.clone();
            closing.spawn(async move {
                let _ = manager.close(&id).await;
            });
        }
        while closing.join_next().await.is_some() {}
        let accounts: Vec<_> = self
            .shared
            .account_backends
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        for b in accounts
            .into_iter()
            .chain(self.shared.backends.values().cloned())
        {
            let _ = b.shutdown().await;
        }
        self.flush_index().await;
    }

    /// Wait until every index change made so far has reached disk.
    pub async fn flush_index(&self) {
        let flush = self.shared.index.lock().unwrap().flusher();
        let _ = tokio::task::spawn_blocking(move || flush.wait()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBackend;
    use std::time::Duration;

    fn cfg(p: Provider) -> SessionConfig {
        SessionConfig {
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: p,
            permission_mode: PermissionMode::Ask,
        }
    }

    fn manager(path: &Path) -> SessionManager {
        SessionManager::new(
            path,
            [
                Arc::new(MockBackend::new(Provider::Codex)) as Arc<dyn AgentBackend>,
                Arc::new(MockBackend::new(Provider::Claude)),
            ],
        )
        .unwrap()
    }

    async fn wait_status(
        rx: &mut broadcast::Receiver<ManagerEvent>,
        id: &SessionId,
        want: SessionStatus,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ManagerEvent::StatusChanged { session, status } = rx.recv().await.unwrap()
                    && &session == id
                    && status == want
                {
                    return;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {want:?}"));
    }

    #[tokio::test]
    async fn account_switch_preserves_identity_history_and_rolls_back_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let source: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        let target: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        let manager = SessionManager::new(&path, [source.clone()]).unwrap();
        manager.register_account("second".into(), target.clone());
        let handle = manager.start_session(cfg(Provider::Codex)).await.unwrap();
        let mut events = manager.subscribe_all();
        manager
            .send_prompt(&handle.id, UserInput::text("remember this conversation"))
            .await
            .unwrap();
        assert!(
            manager
                .switch_account(&handle.id, Some("second".into()), |_| async {
                    panic!("must not transfer a running turn")
                })
                .await
                .is_err()
        );
        wait_status(&mut events, &handle.id, SessionStatus::Idle).await;
        let history = manager.subscribe_with_history(&handle.id).unwrap().0;
        manager
            .switch_account(&handle.id, Some("second".into()), |record| async move {
                assert_eq!(record.account_id, None);
                Ok(None)
            })
            .await
            .unwrap();
        assert_eq!(manager.list_sessions().len(), 1);
        assert_eq!(manager.list_sessions()[0].id, handle.id);
        assert_eq!(
            manager.subscribe_with_history(&handle.id).unwrap().0,
            history
        );
        assert!(Arc::ptr_eq(
            &manager.session_backend(&handle.id).unwrap(),
            &target
        ));
        let failed = manager
            .switch_account(&handle.id, None, |_| async {
                Err(AgentError::Backend("transfer failed".into()))
            })
            .await;
        assert!(failed.is_err());
        assert!(Arc::ptr_eq(
            &manager.session_backend(&handle.id).unwrap(),
            &target
        ));
        manager
            .send_prompt(&handle.id, UserInput::text("continue after rollback"))
            .await
            .unwrap();
        wait_status(&mut events, &handle.id, SessionStatus::Idle).await;
        manager
            .switch_account(&handle.id, None, |_| async { Ok(None) })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &manager.session_backend(&handle.id).unwrap(),
            &source
        ));
        manager.shutdown().await;
        let restored = SessionManager::new(path, [source.clone()]).unwrap();
        restored.resume(&handle.id).await.unwrap();
        assert!(Arc::ptr_eq(
            &restored.session_backend(&handle.id).unwrap(),
            &source
        ));
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn accounts_route_independently_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.jsonl");
        let default: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        let first: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        let second: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        let manager = SessionManager::new(&path, [default.clone()]).unwrap();
        manager.register_account("first".into(), first.clone());
        manager.register_account("second".into(), second.clone());
        let a = manager
            .start_account_session(cfg(Provider::Codex), Some("first".into()))
            .await
            .unwrap();
        let b = manager
            .start_account_session(cfg(Provider::Codex), Some("second".into()))
            .await
            .unwrap();
        let system = manager.start_session(cfg(Provider::Codex)).await.unwrap();
        assert!(Arc::ptr_eq(
            &manager.session_backend(&a.id).unwrap(),
            &first
        ));
        assert!(Arc::ptr_eq(
            &manager.session_backend(&b.id).unwrap(),
            &second
        ));
        assert!(Arc::ptr_eq(
            &manager.session_backend(&system.id).unwrap(),
            &default
        ));
        manager
            .send_prompt(&a.id, UserInput::text("hello a"))
            .await
            .unwrap();
        manager
            .send_prompt(&b.id, UserInput::text("hello b"))
            .await
            .unwrap();
        manager.interrupt(&a.id).await.unwrap();
        manager.interrupt(&b.id).await.unwrap();
        manager.shutdown().await;
        let restored = SessionManager::new(&path, [default]).unwrap();
        // Missing profiles must never silently use the system account.
        assert!(restored.resume(&a.id).await.is_err());
        restored.register_account("first".into(), first.clone());
        restored.resume(&a.id).await.unwrap();
        assert!(Arc::ptr_eq(
            &restored.session_backend(&a.id).unwrap(),
            &first
        ));
        assert!(restored.resume(&b.id).await.is_err());
        assert_eq!(
            restored
                .list_sessions()
                .iter()
                .find(|r| r.id == b.id)
                .unwrap()
                .account_id
                .as_deref(),
            Some("second")
        );
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn prompt_sent_can_generate_title_before_the_reply_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let mut backend = MockBackend::new(Provider::Codex).with_delay(Duration::from_secs(1));
        backend.title_response = Some("Chess board".into());
        let m = SessionManager::new(
            dir.path().join("i.jsonl"),
            [Arc::new(backend) as Arc<dyn AgentBackend>],
        )
        .unwrap();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        let mut events = m.subscribe_all();
        m.send_prompt(&h.id, UserInput::text("Build a chess board"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::PromptSent { session } => {
                        assert_eq!(session, h.id);
                        break;
                    }
                    ManagerEvent::Event {
                        event: AgentEvent::TurnCompleted { .. },
                        ..
                    } => panic!("Title trigger waited for reply"),
                    _ => {}
                }
            }
            assert!(m.generate_title(&h.id).await.unwrap());
        })
        .await
        .unwrap();
        assert_eq!(m.status(&h.id), Some(SessionStatus::Running));
        assert_eq!(m.list_sessions()[0].title.as_deref(), Some("Chess board"));
        // Rejected sends must not schedule another title request.
        while events.try_recv().is_ok() {}
        assert!(
            m.send_prompt(&h.id, UserInput::text("second prompt"))
                .await
                .is_err()
        );
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, ManagerEvent::PromptSent { .. }));
        }
        m.shutdown().await;
    }

    #[tokio::test]
    async fn generated_titles_persist_without_reordering_and_retry_invalid_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.jsonl");
        let mut backend = MockBackend::new(Provider::Codex);
        backend.title_response = Some("Chess Board Design".into());
        let m = SessionManager::new(&path, [Arc::new(backend) as Arc<dyn AgentBackend>]).unwrap();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        assert!(!m.generate_title(&h.id).await.unwrap());
        let mut events = m.subscribe_all();
        m.send_prompt(&h.id, UserInput::text("Build a chess board"))
            .await
            .unwrap();
        wait_status(&mut events, &h.id, SessionStatus::Idle).await;
        let before = m.list_sessions()[0].updated_at_ms;
        assert!(m.generate_title(&h.id).await.unwrap());
        assert!(!m.generate_title(&h.id).await.unwrap());
        let mut loading = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let ManagerEvent::TitleLoading { loading: value, .. } = event {
                loading.push(value);
            }
        }
        assert_eq!(loading, [true, false]);
        m.flush_index().await;
        let saved = SessionIndex::open(&path)
            .unwrap()
            .get(&h.id)
            .unwrap()
            .clone();
        assert_eq!(saved.title.as_deref(), Some("Chess Board Design"));
        assert_eq!(saved.title_turns, 1);
        assert_eq!(saved.updated_at_ms, before);
        for _ in 0..6 {
            m.send_prompt(&h.id, UserInput::text("Add a feature"))
                .await
                .unwrap();
            wait_status(&mut events, &h.id, SessionStatus::Idle).await;
        }
        assert!(m.generate_title(&h.id).await.unwrap());
        assert_eq!(m.list_sessions()[0].title_turns, 7);
        m.shutdown().await;

        let mut backend = MockBackend::new(Provider::Codex);
        backend.title_response = Some("Title\nInvalid explanation".into());
        let m = SessionManager::new(
            dir.path().join("invalid.jsonl"),
            [Arc::new(backend) as Arc<dyn AgentBackend>],
        )
        .unwrap();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        let mut events = m.subscribe_all();
        m.send_prompt(&h.id, UserInput::text("Fallback title"))
            .await
            .unwrap();
        wait_status(&mut events, &h.id, SessionStatus::Idle).await;
        assert!(!m.generate_title(&h.id).await.unwrap());
        assert_eq!(
            m.list_sessions()[0].title.as_deref(),
            Some("Fallback title")
        );
        assert_eq!(m.list_sessions()[0].title_turns, 0);
        let mut loading = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let ManagerEvent::TitleLoading { loading: value, .. } = event {
                loading.push(value);
            }
        }
        assert_eq!(loading, [true, false]);
        m.shutdown().await;
    }

    #[tokio::test]
    async fn standby_is_hidden_consumed_once_and_replenished() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let config = cfg(Provider::Codex);
        let mut events = m.subscribe_all();
        m.prepare_session(config.clone()).await.unwrap();
        let ready_id = m
            .shared
            .standby
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .id
            .clone();
        assert!(m.list_sessions().is_empty());
        assert!(events.try_recv().is_err());
        assert!(!m.is_live(&ready_id));
        m.prepare_session(config.clone()).await.unwrap();
        assert_eq!(
            m.shared.standby.lock().unwrap().as_ref().unwrap().handle.id,
            ready_id
        );
        let (first, second) = tokio::join!(
            m.start_session(config.clone()),
            m.start_session(config.clone())
        );
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_ne!(first.id, second.id);
        assert!(first.id == ready_id || second.id == ready_id);
        assert_eq!(m.list_sessions().len(), 2);
        m.prepare_session(config).await.unwrap();
        let replacement = m
            .shared
            .standby
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .id
            .clone();
        assert_ne!(replacement, ready_id);
        assert_eq!(m.list_sessions().len(), 2);
        m.shutdown().await;
        assert!(m.shared.standby.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn standby_never_crosses_configuration_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let config = cfg(Provider::Codex);
        m.prepare_session(config.clone()).await.unwrap();
        let ready_id = m
            .shared
            .standby
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .handle
            .id
            .clone();
        let mut variants = vec![config.clone(); 6];
        variants[0].cwd = "/different".into();
        variants[1].model = Some("different-model".into());
        variants[2].reasoning_effort = Some("high".into());
        variants[3].service_tier = Some("priority".into());
        variants[4].provider = Provider::Claude;
        variants[5].permission_mode = PermissionMode::FullAuto;
        for different in variants {
            let handle = m.start_session(different).await.unwrap();
            assert_ne!(handle.id, ready_id);
        }
        assert_eq!(m.start_session(config).await.unwrap().id, ready_id);
        m.shutdown().await;
    }

    #[tokio::test]
    async fn expired_standby_is_replaced_without_touching_active_chats() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let config = cfg(Provider::Codex);
        let active = m.start_session(config.clone()).await.unwrap();
        m.prepare_session(config.clone()).await.unwrap();
        let expired = {
            let mut slot = m.shared.standby.lock().unwrap();
            let standby = slot.as_mut().unwrap();
            standby.created = Instant::now() - STANDBY_LIFETIME;
            standby.handle.id.clone()
        };
        let fresh = m.start_session(config.clone()).await.unwrap();
        assert_ne!(fresh.id, expired);
        m.prepare_session(config).await.unwrap();
        assert!(m.is_live(&active.id));
        assert!(m.is_live(&fresh.id));
        // The expired standby is closed in the background.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !matches!(
                m.backend(Provider::Codex)
                    .unwrap()
                    .interrupt(&expired)
                    .await,
                Err(AgentError::UnknownSession(_))
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expired standby must be closed");
        m.shutdown().await;
    }

    #[tokio::test]
    async fn send_does_not_wait_for_background_preparation_lock() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let _preparing = m.shared.preparation.lock().await;
        let started = tokio::time::timeout(
            Duration::from_secs(1),
            m.start_session(cfg(Provider::Codex)),
        )
        .await;
        assert!(started.unwrap().is_ok());
        drop(_preparing);
        m.shutdown().await;
    }

    #[tokio::test]
    async fn reconfiguration_preserves_history_and_existing_subscribers() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let h = m.start_session(cfg(Provider::Claude)).await.unwrap();
        let (_, mut rx) = m.subscribe_with_history(&h.id).unwrap();
        let mut all = m.subscribe_all();
        m.send_prompt(&h.id, UserInput::text("first"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        while rx.try_recv().is_ok() {}
        let (before, _) = m.subscribe_with_history(&h.id).unwrap();
        m.set_model_settings(&h.id, Some("new-model".into()), Some("high".into()), None)
            .unwrap();
        m.set_permission_mode(&h.id, PermissionMode::FullAuto)
            .unwrap();
        m.send_prompt(&h.id, UserInput::text("second"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        let (after, _) = m.subscribe_with_history(&h.id).unwrap();
        assert_eq!(&after[..before.len()], before.as_slice());
        assert_eq!(
            after
                .iter()
                .filter(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
                .count(),
            2
        );
        let mut completed = false;
        while let Ok(event) = rx.try_recv() {
            completed |= matches!(event, AgentEvent::TurnCompleted { .. });
        }
        assert!(
            completed,
            "the original chat subscriber must receive the next reply"
        );
        assert_eq!(
            m.shared.live.lock().unwrap()[&h.id].config.model.as_deref(),
            Some("new-model")
        );
        assert_eq!(
            m.shared.live.lock().unwrap()[&h.id].config.permission_mode,
            PermissionMode::FullAuto
        );
        assert_eq!(
            m.list_sessions()[0].permission_mode,
            PermissionMode::FullAuto
        );
        assert_eq!(m.list_sessions().len(), 1);
    }

    #[tokio::test]
    async fn tracks_status_through_a_turn_and_an_approval() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let mut all = m.subscribe_all();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        assert_eq!(m.status(&h.id), Some(SessionStatus::Idle));

        m.send_prompt(&h.id, UserInput::text("please approve this"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        let approval = loop {
            if let ManagerEvent::Event {
                event: AgentEvent::ApprovalRequested { id, .. },
                ..
            } = all.recv().await.unwrap()
            {
                break id;
            }
        };
        m.respond_approval(&h.id, &approval, ApprovalResponse::Approve)
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Running).await;
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        assert_eq!(
            m.list_sessions()[0].title.as_deref(),
            Some("please approve this")
        );
    }

    #[tokio::test]
    async fn late_subscriber_gets_session_started_and_earlier_events() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let h = m.start_session(cfg(Provider::Claude)).await.unwrap();
        // The first prompt completes before anyone subscribes.
        let mut all = m.subscribe_all();
        m.send_prompt(&h.id, UserInput::text("hi")).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        let (history, mut rx) = m.subscribe_with_history(&h.id).unwrap();
        assert!(matches!(
            history.first(),
            Some(AgentEvent::SessionStarted { session_id }) if *session_id == h.id
        ));
        assert_eq!(
            history
                .iter()
                .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
                .count(),
            1
        );
        assert!(
            history
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
        );
        // No overlap: the receiver only sees what comes after the snapshot.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn fans_out_to_per_session_and_global_subscribers() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let a = m.start_session(cfg(Provider::Codex)).await.unwrap();
        let b = m.start_session(cfg(Provider::Claude)).await.unwrap();
        let mut ra = m.subscribe(&a.id).unwrap();
        let mut all = m.subscribe_all();
        m.send_prompt(&a.id, UserInput::text("hi")).await.unwrap();
        m.send_prompt(&b.id, UserInput::text("hi")).await.unwrap();
        let mut done = std::collections::HashSet::new();
        while done.len() < 2 {
            if let ManagerEvent::Event {
                session,
                event: AgentEvent::TurnCompleted { .. },
            } = all.recv().await.unwrap()
            {
                done.insert(session);
            }
        }
        assert!(matches!(
            ra.recv().await.unwrap(),
            AgentEvent::TurnStarted { .. }
        ));
        assert_eq!(m.list_sessions().len(), 2);
    }

    #[tokio::test]
    async fn unloading_releases_live_session_but_keeps_resumable_record() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let h = m.start_session(cfg(Provider::Claude)).await.unwrap();
        let mut rx = m.subscribe(&h.id).unwrap();
        let before = m.list_sessions()[0].clone();
        m.close(&h.id).await.unwrap();
        m.close(&h.id).await.unwrap();
        assert!(!m.is_live(&h.id));
        assert!(matches!(
            rx.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
        let after = &m.list_sessions()[0];
        assert_eq!(after.id, before.id);
        assert_eq!(after.config(), before.config());
        assert_eq!(after.title, before.title);
        assert_eq!(m.resume(&h.id).await.unwrap().id, h.id);
        assert!(m.is_live(&h.id));
        m.shutdown().await;
    }

    #[tokio::test]
    async fn sessions_survive_restart_and_resume() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.jsonl");
        let id = {
            let m = manager(&path);
            let h = m.start_session(cfg(Provider::Claude)).await.unwrap();
            m.shutdown().await;
            assert!(!m.is_live(&h.id));
            h.id
        };
        let m = manager(&path);
        let list = m.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert!(!m.is_live(&id));
        assert!(
            m.send_prompt(&id, UserInput::text("x")).await.is_err(),
            "must resume first"
        );

        let h = m.resume(&id).await.unwrap();
        let mut rx = h.subscribe();
        m.send_prompt(&id, UserInput::text("hello again"))
            .await
            .unwrap();
        loop {
            if matches!(rx.recv().await.unwrap(), AgentEvent::TurnCompleted { .. }) {
                break;
            }
        }
        m.delete(&id).await.unwrap();
        m.flush_index().await;
        assert!(manager(&path).list_sessions().is_empty());
    }

    #[tokio::test]
    async fn error_and_interrupt_statuses_and_missing_backend() {
        let dir = tempfile::tempdir().unwrap();
        let m = SessionManager::new(
            dir.path().join("i.jsonl"),
            [Arc::new(MockBackend::new(Provider::Codex)) as Arc<dyn AgentBackend>],
        )
        .unwrap();
        assert!(matches!(
            m.start_session(cfg(Provider::Claude)).await,
            Err(AgentError::NoBackend(Provider::Claude))
        ));
        let mut all = m.subscribe_all();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        m.send_prompt(&h.id, UserInput::text("approve"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        m.interrupt(&h.id).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        m.shutdown().await;
        m.shutdown().await;
    }

    #[test]
    fn trimming_keeps_session_start_and_first_prompt() {
        let delta = |i: usize| AgentEvent::AssistantTextDelta {
            turn_id: "t".into(),
            text: i.to_string(),
        };
        let mut history = vec![
            AgentEvent::SessionStarted {
                session_id: SessionId::new("s"),
            },
            delta(0),
            AgentEvent::UserMessage {
                text: "first".into(),
            },
        ];
        history.extend((1..HISTORY_LIMIT).map(delta));
        for _ in 0..3 {
            let len = history.len();
            trim_history(&mut history);
            assert!(history.len() < len);
            assert!(matches!(history[0], AgentEvent::SessionStarted { .. }));
            assert!(matches!(&history[1], AgentEvent::UserMessage { text } if text == "first"));
            history.extend((0..HISTORY_LIMIT).map(delta));
        }
        assert_eq!(history.last(), Some(&delta(HISTORY_LIMIT - 1)));
    }

    #[tokio::test]
    async fn provider_resolved_approvals_resume_running_once_none_pend() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let mut all = m.subscribe_all();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        m.send_prompt(&h.id, UserInput::text("withdraw"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        let mut resolved = 0;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match all.recv().await.unwrap() {
                    ManagerEvent::Event {
                        event: AgentEvent::ApprovalResolved { .. },
                        ..
                    } => resolved += 1,
                    // The status is published just before the event that caused it.
                    ManagerEvent::StatusChanged {
                        status: SessionStatus::Running,
                        ..
                    } => {
                        assert_eq!(resolved, 1, "Running only once both are settled");
                        return;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        m.shutdown().await;
    }

    #[tokio::test]
    async fn approval_reply_never_overwrites_a_finished_turn() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        assert!(!m.shared.transition(&h.id, |current| {
            (current == SessionStatus::WaitingApproval).then_some(SessionStatus::Running)
        }));
        assert_eq!(m.status(&h.id), Some(SessionStatus::Idle));
        m.shutdown().await;
    }

    #[tokio::test]
    async fn close_broadcasts_idle_and_detaches_even_without_a_backend() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let mut all = m.subscribe_all();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        m.send_prompt(&h.id, UserInput::text("approve"))
            .await
            .unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        m.close(&h.id).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        assert_eq!(m.list_sessions()[0].status, SessionStatus::Idle);

        let account: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(Provider::Codex));
        m.register_account("gone".into(), account);
        let a = m
            .start_account_session(cfg(Provider::Codex), Some("gone".into()))
            .await
            .unwrap();
        m.unregister_account(Provider::Codex, "gone");
        assert!(m.close(&a.id).await.is_err());
        assert!(!m.is_live(&a.id));
        m.delete(&a.id).await.unwrap();
        assert!(!m.shared.resume_locks.lock().unwrap().contains_key(&a.id));
        m.shutdown().await;
    }

    #[tokio::test]
    async fn live_session_closes_even_without_an_index_record() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        m.shared.index.lock().unwrap().remove(&h.id).unwrap();
        m.close(&h.id).await.unwrap();
        assert!(!m.is_live(&h.id));
        assert!(
            m.backend(Provider::Codex)
                .unwrap()
                .send_prompt(&h.id, UserInput::text("x"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn resume_clears_a_persisted_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.jsonl");
        let id = {
            let m = manager(&path);
            let h = m.start_session(cfg(Provider::Claude)).await.unwrap();
            m.shutdown().await;
            h.id
        };
        {
            let mut index = SessionIndex::open(&path).unwrap();
            index
                .update(&id, |r| r.status = SessionStatus::Error)
                .unwrap();
        }
        let m = manager(&path);
        let mut all = m.subscribe_all();
        m.resume(&id).await.unwrap();
        wait_status(&mut all, &id, SessionStatus::Idle).await;
        assert_eq!(m.list_sessions()[0].status, SessionStatus::Idle);
        m.shutdown().await;
    }

    #[tokio::test]
    async fn provider_titles_are_read_only_from_running_providers_and_only_after_activity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("i.jsonl");
        let mut backend = MockBackend::new(Provider::Codex);
        backend.session_title = Some("Named\u{202E} by provider".into());
        let reads = backend.session_title_reads.clone();
        let m = SessionManager::new(&path, [Arc::new(backend) as Arc<dyn AgentBackend>]).unwrap();
        let saved = m.start_session(cfg(Provider::Codex)).await.unwrap();
        m.close(&saved.id).await.unwrap();
        let mut elsewhere = cfg(Provider::Codex);
        elsewhere.cwd = "/elsewhere".into();
        let other = m.start_session(elsewhere).await.unwrap();
        m.close(&other.id).await.unwrap();
        // Nothing is live: no provider may be started to read names.
        assert!(!m.refresh_titles().await.unwrap());
        assert!(reads.lock().unwrap().is_empty());

        let live = m.start_session(cfg(Provider::Codex)).await.unwrap();
        assert!(m.refresh_titles().await.unwrap());
        let mut read = reads.lock().unwrap().clone();
        read.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let mut expected = vec![live.id.clone(), saved.id.clone()];
        expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(read, expected, "same provider and folder only");
        assert!(
            m.list_sessions()
                .iter()
                .filter(|r| r.id != other.id)
                .all(|r| r.title.as_deref() == Some("Named by provider"))
        );
        // No new activity: not asked again.
        assert!(!m.refresh_titles().await.unwrap());
        assert_eq!(reads.lock().unwrap().len(), 2);
        m.shutdown().await;
    }

    #[test]
    fn status_machine_keeps_error_until_next_prompt() {
        let done = AgentEvent::TurnCompleted {
            turn_id: "t".into(),
            status: TurnStatus::Failed,
        };
        let fatal = AgentEvent::Error {
            message: "x".into(),
            recoverable: false,
        };
        let soft = AgentEvent::Error {
            message: "x".into(),
            recoverable: true,
        };
        assert_eq!(
            next_status(SessionStatus::Running, &soft, false),
            SessionStatus::Running
        );
        let e = next_status(SessionStatus::Running, &fatal, false);
        assert_eq!(e, SessionStatus::Error);
        assert_eq!(next_status(e, &done, false), SessionStatus::Error);
        assert_eq!(
            next_status(SessionStatus::Running, &done, false),
            SessionStatus::Idle
        );
        let resolved = AgentEvent::ApprovalResolved { id: "a".into() };
        let waiting = SessionStatus::WaitingApproval;
        assert_eq!(
            next_status(waiting, &resolved, false),
            SessionStatus::Running
        );
        assert_eq!(next_status(waiting, &resolved, true), waiting);
        assert_eq!(
            next_status(SessionStatus::Idle, &resolved, false),
            SessionStatus::Idle
        );
    }
}
