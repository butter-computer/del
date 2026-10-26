//! The chat page: a `ChatView` entity that owns one agent session, folds its
//! `AgentEvent` stream into a `ChatModel`, and renders the transcript plus the
//! composer.

pub(crate) mod attachments;
mod activity;
mod blocks;
mod loading;
pub mod model;
mod view;

pub use view::{Backend, COLUMN_MAX_WIDTH, ChatView, PromptSubmitted};

pub use blocks::pending_prompt;
pub use loading::conversation_loading;
