//! Codex integration for del: a JSON-RPC client for `codex app-server` and an
//! [`del_agent::AgentBackend`] built on it.

pub mod auth;
pub mod backend;
pub mod mapping;
pub mod protocol;
pub mod rpc;

pub use auth::{AuthStatus, CodexError};
pub use backend::{CodexBackend, CodexOptions};
pub use rpc::{ClientError, CodexClient, Incoming, RequestId, SpawnOptions};
