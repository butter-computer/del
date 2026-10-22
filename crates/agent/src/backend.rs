//! The backend contract implemented by Codex, Claude and the mock.

use crate::types::*;
use async_trait::async_trait;
use tokio::sync::broadcast;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("unknown session {0}")]
    UnknownSession(SessionId),
    #[error("no backend registered for {0:?}")]
    NoBackend(Provider),
    #[error("session is busy: {0}")]
    Busy(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AgentError>;

/// Cheap, cloneable handle to a live session and its event stream.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub provider: Provider,
    events: broadcast::Sender<AgentEvent>,
}

impl SessionHandle {
    pub fn new(id: SessionId, provider: Provider, events: broadcast::Sender<AgentEvent>) -> Self {
        Self {
            id,
            provider,
            events,
        }
    }
    /// Subscribe to events emitted from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    pub fn sender(&self) -> &broadcast::Sender<AgentEvent> {
        &self.events
    }
}

/// Capacity used for per-session broadcast channels.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;

#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn provider(&self) -> Provider;

    /// Start a fresh session. Emits `SessionStarted` on the stream.
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle>;
    /// Begin a new turn. Returns once the prompt is accepted; progress arrives as events.
    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()>;
    /// Inject guidance into the turn currently in flight.
    async fn steer(&self, session: &SessionId, text: String) -> Result<()>;
    /// Stop the current turn (ends with `TurnCompleted{Interrupted}`).
    async fn interrupt(&self, session: &SessionId) -> Result<()>;
    /// Reattach to a previously started session (e.g. after app restart).
    async fn resume(&self, config: SessionConfig, session: &SessionId) -> Result<SessionHandle>;
    /// Answer a pending `ApprovalRequested`.
    async fn respond_approval(
        &self,
        session: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()>;
    /// End the session and release its resources (child processes).
    async fn close(&self, session: &SessionId) -> Result<()>;
    /// Kill every child process this backend owns.
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
