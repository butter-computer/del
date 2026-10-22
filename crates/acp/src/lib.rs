//! ACP v1 over stdio, as an additional del chat backend.
mod backend;
pub mod registry;
mod rpc;
mod translate;

pub use backend::AcpBackend;
pub use registry::AcpAgentConfig;

mod models;
