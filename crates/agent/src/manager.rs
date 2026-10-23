//! Supervisor owning many sessions across providers.

use crate::backend::*;
use crate::index::*;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Everything the manager broadcasts to `subscribe_all` listeners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManagerEvent {
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
}

const STANDBY_LIFETIME: Duration = Duration::from_secs(30 * 60);

struct Standby {
    config: SessionConfig,
    handle: SessionHandle,
    created: Instant,
}

struct Shared {
    backends: HashMap<Provider, Arc<dyn AgentBackend>>,
    index: Mutex<SessionIndex>,
    live: Mutex<HashMap<SessionId, Live>>,
    resume_locks: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
    all_tx: broadcast::Sender<ManagerEvent>,
    standby: Mutex<Option<Standby>>,
    preparation: tokio::sync::Mutex<()>,
}

/// Cheap to clone; all clones share state.
#[derive(Clone)]
pub struct SessionManager {
    shared: Arc<Shared>,
}

fn next_status(current: SessionStatus, ev: &AgentEvent) -> SessionStatus {
    match ev {
        AgentEvent::TurnStarted { .. } => SessionStatus::Running,
        AgentEvent::ApprovalRequested { .. } => SessionStatus::WaitingApproval,
        AgentEvent::Error {
            recoverable: false, ..
        } => SessionStatus::Error,
        AgentEvent::TurnCompleted { .. } if current != SessionStatus::Error => SessionStatus::Idle,
        _ => current,
    }
}

impl Shared {
    fn set_status(&self, id: &SessionId, status: SessionStatus) {
        {
            let mut live = self.live.lock().unwrap();
            match live.get_mut(id) {
                Some(l) if l.status != status => l.status = status,
                _ => return,
            }
        }
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

    /// Refresh provider-owned names without changing session order or live state.
    pub async fn refresh_titles(&self) -> Result<bool> {
        let mut changed = false;
        for record in self.list_sessions() {
            let Ok(backend) = self.backend(record.provider) else {
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
            let title = title.trim();
            if title.is_empty() {
                continue;
            }
            let mut index = self.shared.index.lock().unwrap();
            if index
                .get(&record.id)
                .is_some_and(|r| r.title.as_deref() != Some(title))
            {
                index.update(&record.id, |r| r.title = Some(title.to_owned()))?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// `index_path` is typically [`default_index_path`].
    pub fn new(
        index_path: impl AsRef<Path>,
        backends: impl IntoIterator<Item = Arc<dyn AgentBackend>>,
    ) -> Result<Self> {
        let (all_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Ok(Self {
            shared: Arc::new(Shared {
                backends: backends.into_iter().map(|b| (b.provider(), b)).collect(),
                index: Mutex::new(SessionIndex::open(index_path.as_ref())?),
                live: Mutex::new(HashMap::new()),
                resume_locks: Mutex::new(HashMap::new()),
                all_tx,
                standby: Mutex::new(None),
                preparation: tokio::sync::Mutex::new(()),
            }),
        })
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
    /// is dropped.
    fn attach(&self, handle: SessionHandle, config: SessionConfig) {
        let id = handle.id.clone();
        let mut rx = handle.subscribe();
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
            history.extend_from_slice(handle.history());
            let _ = self.shared.all_tx.send(ManagerEvent::Event {
                session: id.clone(),
                event: started,
            });
            (fanout, history)
        };
        let shared = self.shared.clone();
        let pump_id = id.clone();
        let pump = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(AgentEvent::SessionStarted { .. }) => {}
                    Ok(event) => {
                        let mut live = shared.live.lock().unwrap();
                        let Some(l) = live.get_mut(&pump_id) else {
                            continue;
                        };
                        if l.history.len() >= HISTORY_LIMIT {
                            l.history.drain(..HISTORY_LIMIT / 10);
                        }
                        l.history.push(event.clone());
                        let _ = l.fanout.send(event.clone());
                        let next = next_status(l.status, &event);
                        drop(live);
                        // Status first so a listener seeing the event sees a consistent status.
                        shared.set_status(&pump_id, next);
                        let _ = shared.all_tx.send(ManagerEvent::Event {
                            session: pump_id.clone(),
                            event,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        live.insert(
            id,
            Live {
                handle,
                config,
                status: SessionStatus::Idle,
                pump,
                fanout,
                history,
            },
        );
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
        let applied_config = config.clone();
        let prepared = {
            let mut slot = self.shared.standby.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|s| s.config == config && s.created.elapsed() < STANDBY_LIFETIME)
            {
                slot.take().map(|s| s.handle)
            } else {
                None
            }
        };
        let started = Instant::now();
        let warm = prepared.is_some();
        let handle = match prepared {
            Some(handle) => handle,
            None => {
                self.backend(config.provider)?
                    .start_session(config.clone())
                    .await?
            }
        };
        eprintln!(
            "[del startup] session ready warm={warm} elapsed={}ms",
            started.elapsed().as_millis()
        );
        let now = now_ms();
        self.shared.index.lock().unwrap().upsert(SessionRecord {
            id: handle.id.clone(),
            provider: config.provider,
            cwd: config.cwd,
            model: config.model,
            reasoning_effort: config.reasoning_effort,
            service_tier: config.service_tier,
            permission_mode: config.permission_mode,
            title: None,
            created_at_ms: now,
            updated_at_ms: now,
            status: SessionStatus::Idle,
        })?;
        self.attach(handle.clone(), applied_config);
        Ok(handle)
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
        if let Some(l) = self.shared.live.lock().unwrap().get(id) {
            return Ok(l.handle.clone());
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
            .backend(record.provider)?
            .resume(record.config(), id)
            .await?;
        self.attach(handle.clone(), record.config());
        self.shared.set_status(id, SessionStatus::Idle);
        Ok(handle)
    }

    pub async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _sending = lock.lock().await;
        let p = self.live_provider(id)?;
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
            let handle = self.backend(p)?.reconfigure(config.clone(), id).await?;
            self.attach(handle, config);
        }
        let title: String = input.text.trim().chars().take(60).collect();
        self.backend(p)?.send_prompt(id, input).await?;
        let _ = self.shared.index.lock().unwrap().update(id, |r| {
            if r.title.is_none() && !title.is_empty() {
                r.title = Some(title);
            }
        });
        self.shared.set_status(id, SessionStatus::Running);
        Ok(())
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
        self.backend(self.live_provider(id)?)?.steer(id, text).await
    }

    pub async fn steer_input(&self, id: &SessionId, input: UserInput) -> Result<()> {
        self.backend(self.live_provider(id)?)?
            .steer_input(id, input)
            .await
    }

    pub async fn interrupt(&self, id: &SessionId) -> Result<()> {
        self.backend(self.live_provider(id)?)?.interrupt(id).await
    }

    pub async fn respond_approval(
        &self,
        id: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        self.backend(self.live_provider(id)?)?
            .respond_approval(id, approval_id, response)
            .await?;
        self.shared.set_status(id, SessionStatus::Running);
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
        if !self.is_live(id) {
            return Ok(());
        }
        let p = self.live_provider(id)?;
        let res = self.backend(p)?.close(id).await;
        if let Some(l) = self.shared.live.lock().unwrap().remove(id) {
            l.pump.abort();
        }
        let _ = self
            .shared
            .index
            .lock()
            .unwrap()
            .update(id, |r| r.status = SessionStatus::Idle);
        res
    }

    /// Close if live, then forget the session entirely.
    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        if self.is_live(id) {
            let _ = self.close(id).await;
        }
        self.shared.index.lock().unwrap().remove(id)
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
    pub async fn shutdown(&self) {
        self.clear_prepared_session().await;
        let ids: Vec<SessionId> = self.shared.live.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.close(&id).await;
        }
        for b in self.shared.backends.values() {
            let _ = b.shutdown().await;
        }
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
        assert!(
            m.backend(Provider::Codex)
                .unwrap()
                .send_prompt(&expired, UserInput::text("unused"))
                .await
                .is_err()
        );
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
            next_status(SessionStatus::Running, &soft),
            SessionStatus::Running
        );
        let e = next_status(SessionStatus::Running, &fatal);
        assert_eq!(e, SessionStatus::Error);
        assert_eq!(next_status(e, &done), SessionStatus::Error);
        assert_eq!(
            next_status(SessionStatus::Running, &done),
            SessionStatus::Idle
        );
    }
}
