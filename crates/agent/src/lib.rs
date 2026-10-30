//! Harness-neutral agent layer: normalized events, the `AgentBackend` contract,
//! a scriptable mock, a session supervisor and CLI detection.

pub mod backend;
pub mod commands;
pub use commands::{CommandKind, ComposerCommand, Goal, GoalAction, GoalStatus, split_command};
pub mod detect;
pub mod environment;
pub mod index;
pub mod manager;
pub mod mock;
pub mod peers;
pub mod procgroup;
pub mod types;

pub use backend::*;
pub use detect::{LoginState, ProviderStatus, detect, detect_all};
pub use index::{SessionRecord, data_dir, default_index_path};
pub use manager::{
    ForkContext, ForkLifetime, ForkResult, ForkSnapshot, ForkSource, ManagerEvent, SessionManager,
};
pub use mock::MockBackend;
pub use types::*;

mod models;
pub use models::{EffortOption, ModelInfo};

pub mod titles;
