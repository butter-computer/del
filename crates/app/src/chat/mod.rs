//! The chat page: a `ChatView` entity that owns one agent session, folds its
//! `AgentEvent` stream into a `ChatModel`, and renders the transcript plus the
//! composer.

mod activity;
pub(crate) mod attachments;
mod blocks;
mod citations;
pub(crate) mod links;
mod loading;
pub mod model;
mod reply_times;
mod view;

pub use view::{
    Backend, COLUMN_MAX_WIDTH, ChatView, OpenLink, OpenPeer, PeerActivity, PromptSubmitted,
    ReplyCompleted,
};

pub use blocks::pending_prompt;
pub use loading::conversation_loading;
