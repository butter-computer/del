//! del's own MCP server. See the crate README for the tool contracts.

pub mod host;
pub mod launch;
pub mod manager_host;
pub mod mock;
pub mod server;
pub mod stdio;
pub mod tools;

pub use host::*;
pub use mock::MockHost;
pub use server::{Credentials, McpServer, ServerOptions};
pub use manager_host::{ManagerHost, McpUi};
