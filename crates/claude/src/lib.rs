//! Claude Code backend: drives the user's installed `claude` CLI as a
//! subprocess in `--print --input-format stream-json --output-format stream-json`
//! mode and maps its events to [`del_agent::AgentEvent`]. See `TERMS.md`.

mod backend;
mod protocol;

pub use backend::{AuthStatus, ClaudeBackend, ClaudeOptions, auth_status};
pub use protocol::Translator;
