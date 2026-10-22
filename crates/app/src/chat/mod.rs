//! The chat page: a `ChatView` entity that owns one agent session, folds its
//! `AgentEvent` stream into a `ChatModel`, and renders the transcript plus the
//! composer.

mod blocks;
pub mod demo;
pub mod model;
mod view;

pub use view::{Backend, ChatView};
