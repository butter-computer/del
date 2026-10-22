//! Harness-neutral agent layer: normalized events, the `AgentBackend` contract,
//! a scriptable mock, a session supervisor and CLI detection.

pub mod backend;
pub mod index;
pub mod manager;
pub mod mock;
pub mod types;

pub use backend::*;
pub use index::{SessionRecord, default_index_path};
pub use manager::{ManagerEvent, SessionManager};
pub use mock::MockBackend;
pub use types::*;
