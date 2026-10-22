//! The seam between the MCP server and the app.
//!
//! The server owns the wire protocol, authentication and scoping; everything
//! that touches real state (sessions, the UI) goes through [`McpHost`]. The app
//! implements it over `del_agent::SessionManager` and its UI; tests use
//! [`MockHost`](crate::MockHost). No GPUI types appear here.

use async_trait::async_trait;
use del_agent::{Provider, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What an agent says it is doing, via `report_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Working,
    Idle,
    Blocked,
    Done,
    Error,
}

impl ReportedState {
    pub const ALL: [&'static str; 5] = ["working", "idle", "blocked", "done", "error"];
}

/// One row of `list_sessions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    Chat,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub provider: Option<Provider>,
    #[serde(default)]
    pub mode: SessionMode,
    #[serde(default)]
    pub command: Option<String>,
    pub cwd: PathBuf,
    pub title: Option<String>,
    /// del's own coarse status: `idle`, `running`, `waiting_approval`, `error`.
    pub status: String,
    /// Last state the session reported itself, if any.
    pub reported_state: Option<ReportedState>,
}

/// Result of `get_session_summary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub info: SessionInfo,
    /// Last `report_status` summary text, if any.
    pub reported_summary: Option<String>,
    /// Tail of the session's latest assistant output, if del has it.
    pub last_output: Option<String>,
}

/// How a user answered an `ask_user` prompt in the del UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserAnswer {
    /// Picked one of the offered options (`index` into the option list).
    Option { index: usize, text: String },
    /// Typed free text.
    Text { text: String },
    /// Dismissed without answering.
    Dismissed,
}

/// Arguments of `spawn_session`, already validated by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    pub provider: Provider,
    pub cwd: PathBuf,
    pub prompt: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("unknown session {0}")]
    UnknownSession(String),
    #[error("{0}")]
    Failed(String),
}

pub type HostResult<T> = Result<T, HostError>;

/// Implemented by the app. Every call carries `caller`, the session whose
/// bearer token authenticated the request; the server has already enforced
/// scoping, so hosts may trust it but must not widen it.
///
/// Methods may be dropped mid-await when the agent disconnects (notably
/// `ask_user`); implementations should treat drop as cancellation and remove
/// any prompt they showed.
#[async_trait]
pub trait McpHost: Send + Sync + 'static {
    /// Record what `caller` is doing so del can show it.
    async fn report_status(
        &self,
        caller: &SessionId,
        state: ReportedState,
        summary: &str,
    ) -> HostResult<()>;

    /// Show a question for `caller` in the UI and wait for the user.
    async fn ask_user(
        &self,
        caller: &SessionId,
        question: &str,
        options: &[String],
    ) -> HostResult<UserAnswer>;

    async fn list_sessions(&self, caller: &SessionId) -> HostResult<Vec<SessionInfo>>;

    async fn session_summary(
        &self,
        caller: &SessionId,
        target: &SessionId,
    ) -> HostResult<SessionSummary>;

    /// Start a sibling session and send it the prompt; returns its id.
    async fn spawn_session(
        &self,
        caller: &SessionId,
        request: SpawnRequest,
    ) -> HostResult<SessionInfo>;

    async fn send_to_session(
        &self,
        caller: &SessionId,
        target: &SessionId,
        text: &str,
    ) -> HostResult<()>;

    /// Raise a desktop/in-app notification attributed to `caller`.
    async fn notify(&self, caller: &SessionId, title: &str, body: &str) -> HostResult<()>;
}
