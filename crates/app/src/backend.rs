//! Backend selection at startup.
//!
//! `DEL_BACKEND=mock` always uses the in-process mock. Otherwise the real
//! Codex / Claude backends are used; until those crates are merged into this
//! branch, asking for one reports that it is unavailable.

use async_trait::async_trait;
use del_agent::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

pub type Backends = HashMap<Provider, Arc<dyn AgentBackend>>;

pub fn use_mock() -> bool {
    std::env::var("DEL_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("mock"))
}

/// Build the backend for each provider that has one.
pub fn build() -> Backends {
    let mut map = Backends::new();
    if use_mock() {
        for p in Provider::ALL {
            map.insert(p, Arc::new(Mock::new(p)));
        }
    }
    map
}

/// Minimal scripted backend: a prompt streams a short reply, then completes.
/// Stand-in until `del-agent`'s own `MockBackend` is merged.
struct Mock {
    provider: Provider,
    sessions: Mutex<HashMap<SessionId, broadcast::Sender<AgentEvent>>>,
}

impl Mock {
    fn new(provider: Provider) -> Self {
        Self { provider, sessions: Mutex::default() }
    }

    fn sender(&self, id: &SessionId) -> Result<broadcast::Sender<AgentEvent>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    fn open(&self, id: SessionId) -> SessionHandle {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        self.sessions.lock().unwrap().insert(id.clone(), tx.clone());
        SessionHandle::new(id, self.provider, tx)
    }
}

#[async_trait]
impl AgentBackend for Mock {
    fn provider(&self) -> Provider {
        self.provider
    }

    async fn start_session(&self, _config: SessionConfig) -> Result<SessionHandle> {
        let id = SessionId::new(format!("mock-{}", uuid_like()));
        Ok(self.open(id))
    }

    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let tx = self.sender(session)?;
        tokio::spawn(async move {
            let turn = "turn".to_string();
            let _ = tx.send(AgentEvent::TurnStarted { turn_id: turn.clone() });
            tokio::time::sleep(Duration::from_millis(1200)).await;
            let _ = tx.send(AgentEvent::AssistantMessage {
                turn_id: turn.clone(),
                text: format!("(mock) you said: {}", input.text),
            });
            let _ = tx.send(AgentEvent::TurnCompleted { turn_id: turn, status: TurnStatus::Completed });
        });
        Ok(())
    }

    async fn steer(&self, _: &SessionId, _: String) -> Result<()> {
        Ok(())
    }

    async fn interrupt(&self, session: &SessionId) -> Result<()> {
        let tx = self.sender(session)?;
        let _ = tx.send(AgentEvent::TurnCompleted { turn_id: "turn".into(), status: TurnStatus::Interrupted });
        Ok(())
    }

    async fn resume(&self, _config: SessionConfig, session: &SessionId) -> Result<SessionHandle> {
        Ok(self.open(session.clone()))
    }

    async fn respond_approval(&self, _: &SessionId, _: &str, _: ApprovalResponse) -> Result<()> {
        Ok(())
    }

    async fn close(&self, session: &SessionId) -> Result<()> {
        self.sessions.lock().unwrap().remove(session);
        Ok(())
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{:x}-{}", crate::sessions::now_secs(), N.fetch_add(1, Ordering::Relaxed))
}
