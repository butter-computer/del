//! The chat page: a `ChatView` entity that owns one agent session, folds its
//! `AgentEvent` stream into a `ChatModel`, and renders the transcript plus the
//! composer.

pub(crate) mod attachments;
mod activity;
pub(crate) mod links;
mod blocks;
mod loading;
pub mod model;
mod view;

pub use view::{Backend, COLUMN_MAX_WIDTH, ChatView, PromptSubmitted, OpenLink};

pub use blocks::pending_prompt;
pub use loading::conversation_loading;
