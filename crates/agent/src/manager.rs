//! Supervisor owning many sessions across providers.

use crate::backend::*;
use crate::index::*;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Everything the manager broadcasts to `subscribe_all` listeners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManagerEvent {
    Event { session: SessionId, event: AgentEvent },
    StatusChanged { session: SessionId, status: SessionStatus },
}

struct Live {
    handle: SessionHandle,
    status: SessionStatus,
    pump: JoinHandle<()>,
}

struct Shared {
    backends: HashMap<Provider, Arc<dyn AgentBackend>>,
    index: Mutex<SessionIndex>,
    live: Mutex<HashMap<SessionId, Live>>,
    all_tx: broadcast::Sender<ManagerEvent>,
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
        AgentEvent::Error { recoverable: false, .. } => SessionStatus::Error,
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
        let _ = self.all_tx.send(ManagerEvent::StatusChanged { session: id.clone(), status });
    }
}

impl SessionManager {
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
                all_tx,
            }),
        })
    }

    fn backend(&self, p: Provider) -> Result<Arc<dyn AgentBackend>> {
        self.shared.backends.get(&p).cloned().ok_or(AgentError::NoBackend(p))
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

    fn attach(&self, handle: SessionHandle) {
        let id = handle.id.clone();
        let mut rx = handle.subscribe();
        let shared = self.shared.clone();
        let pump_id = id.clone();
        let pump = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let current = shared.live.lock().unwrap().get(&pump_id).map(|l| l.status);
                        if let Some(cur) = current {
                            let next = next_status(cur, &event);
                            // Status first so a listener seeing the event sees a consistent status.
                            shared.set_status(&pump_id, next);
                        }
                        let _ = shared.all_tx.send(ManagerEvent::Event { session: pump_id.clone(), event });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let old = self
            .shared
            .live
            .lock()
            .unwrap()
            .insert(id, Live { handle, status: SessionStatus::Idle, pump });
        if let Some(old) = old {
            old.pump.abort();
        }
    }

    pub async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let handle = self.backend(config.provider)?.start_session(config.clone()).await?;
        let now = now_ms();
        self.shared.index.lock().unwrap().upsert(SessionRecord {
            id: handle.id.clone(),
            provider: config.provider,
            cwd: config.cwd,
            model: config.model,
            permission_mode: config.permission_mode,
            title: None,
            created_at_ms: now,
            updated_at_ms: now,
            status: SessionStatus::Idle,
        })?;
        self.attach(handle.clone());
        Ok(handle)
    }

    /// Reattach a session from the index (or return the live one).
    pub async fn resume(&self, id: &SessionId) -> Result<SessionHandle> {
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
        let handle = self.backend(record.provider)?.resume(record.config(), id).await?;
        self.attach(handle.clone());
        self.shared.set_status(id, SessionStatus::Idle);
        Ok(handle)
    }

    pub async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        let p = self.live_provider(id)?;
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

    pub async fn steer(&self, id: &SessionId, text: String) -> Result<()> {
        self.backend(self.live_provider(id)?)?.steer(id, text).await
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
        self.backend(self.live_provider(id)?)?.respond_approval(id, approval_id, response).await?;
        self.shared.set_status(id, SessionStatus::Running);
        Ok(())
    }

    /// End the live session. Its index record stays so it can be resumed.
    pub async fn close(&self, id: &SessionId) -> Result<()> {
        let p = self.live_provider(id)?;
        let res = self.backend(p)?.close(id).await;
        if let Some(l) = self.shared.live.lock().unwrap().remove(id) {
            l.pump.abort();
        }
        let _ = self.shared.index.lock().unwrap().update(id, |r| r.status = SessionStatus::Idle);
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

    /// Events of one live session.
    pub fn subscribe(&self, id: &SessionId) -> Result<broadcast::Receiver<AgentEvent>> {
        self.shared
            .live
            .lock()
            .unwrap()
            .get(id)
            .map(|l| l.handle.subscribe())
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    /// Events and status changes of every session.
    pub fn subscribe_all(&self) -> broadcast::Receiver<ManagerEvent> {
        self.shared.all_tx.subscribe()
    }

    /// Close every session and kill all backend child processes. Idempotent.
    pub async fn shutdown(&self) {
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
        SessionConfig { cwd: "/tmp".into(), model: None, provider: p, permission_mode: PermissionMode::Ask }
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

    async fn wait_status(rx: &mut broadcast::Receiver<ManagerEvent>, id: &SessionId, want: SessionStatus) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ManagerEvent::StatusChanged { session, status } = rx.recv().await.unwrap() {
                    if &session == id && status == want {
                        return;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {want:?}"));
    }

    #[tokio::test]
    async fn tracks_status_through_a_turn_and_an_approval() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&dir.path().join("i.jsonl"));
        let mut all = m.subscribe_all();
        let h = m.start_session(cfg(Provider::Codex)).await.unwrap();
        assert_eq!(m.status(&h.id), Some(SessionStatus::Idle));

        m.send_prompt(&h.id, UserInput::text("please approve this")).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        let approval = loop {
            if let ManagerEvent::Event { event: AgentEvent::ApprovalRequested { id, .. }, .. } = all.recv().await.unwrap() {
                break id;
            }
        };
        m.respond_approval(&h.id, &approval, ApprovalResponse::Approve).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Running).await;
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        assert_eq!(m.list_sessions()[0].title.as_deref(), Some("please approve this"));
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
            if let ManagerEvent::Event { session, event: AgentEvent::TurnCompleted { .. } } = all.recv().await.unwrap() {
                done.insert(session);
            }
        }
        assert!(matches!(ra.recv().await.unwrap(), AgentEvent::TurnStarted { .. }));
        assert_eq!(m.list_sessions().len(), 2);
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
        assert!(m.send_prompt(&id, UserInput::text("x")).await.is_err(), "must resume first");

        let h = m.resume(&id).await.unwrap();
        let mut rx = h.subscribe();
        m.send_prompt(&id, UserInput::text("hello again")).await.unwrap();
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
        m.send_prompt(&h.id, UserInput::text("approve")).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::WaitingApproval).await;
        m.interrupt(&h.id).await.unwrap();
        wait_status(&mut all, &h.id, SessionStatus::Idle).await;
        m.shutdown().await;
        m.shutdown().await;
    }

    #[test]
    fn status_machine_keeps_error_until_next_prompt() {
        let done = AgentEvent::TurnCompleted { turn_id: "t".into(), status: TurnStatus::Failed };
        let fatal = AgentEvent::Error { message: "x".into(), recoverable: false };
        let soft = AgentEvent::Error { message: "x".into(), recoverable: true };
        assert_eq!(next_status(SessionStatus::Running, &soft), SessionStatus::Running);
        let e = next_status(SessionStatus::Running, &fatal);
        assert_eq!(e, SessionStatus::Error);
        assert_eq!(next_status(e, &done), SessionStatus::Error);
        assert_eq!(next_status(SessionStatus::Running, &done), SessionStatus::Idle);
    }
}
