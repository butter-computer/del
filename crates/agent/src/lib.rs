//! Harness-neutral agent layer: normalized events, the `AgentBackend` contract,
//! a scriptable mock, a session supervisor and CLI detection.

pub mod backend;
pub mod types;

pub use backend::*;
pub use types::*;
