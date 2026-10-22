//! The subset of the Codex app-server v2 protocol that del uses.
//!
//! Derived from `schema/codex_app_server_protocol.schemas.json` (see
//! `schema/VERSION`). Everything we do not interpret stays a `serde_json::Value`
//! and unknown enum variants fall back to `Unknown`, so a newer CLI adding
//! items or fields does not break decoding.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadRef {
    pub id: String,
    #[serde(default)]
    pub turns: Vec<Value>,
}

/// Result of `thread/start`, `thread/resume` and `thread/fork`.
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadResponse {
    pub thread: ThreadRef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnRef {
    pub id: String,
    #[serde(default)]
    pub status: Option<TurnStatus>,
    #[serde(default)]
    pub error: Option<TurnError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnResponse {
    pub turn: TurnRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
    InProgress,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnError {
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountResponse {
    pub account: Option<Account>,
    #[serde(default)]
    pub requires_openai_auth: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Account {
    ApiKey,
    Chatgpt {
        #[serde(default)]
        email: Option<String>,
        #[serde(default, rename = "planType")]
        plan_type: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
    Never,
}

/// One entry of `turn/start.input` / `turn/steer.input`.
pub fn text_input(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

pub fn local_image_input(path: &str) -> Value {
    json!({ "type": "localImage", "path": path })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileUpdateChange {
    pub path: String,
    pub kind: PatchChangeKind,
    #[serde(default)]
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PatchChangeKind {
    Add,
    Delete,
    Update,
    #[serde(other)]
    Unknown,
}

/// A thread item, as carried by `item/started` and `item/completed`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadItem {
    #[serde(rename_all = "camelCase")]
    AgentMessage {
        id: String,
        #[serde(default)]
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        id: String,
        command: String,
        status: ItemStatus,
        #[serde(default)]
        aggregated_output: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    #[serde(rename_all = "camelCase")]
    FileChange {
        id: String,
        #[serde(default)]
        changes: Vec<FileUpdateChange>,
        status: ItemStatus,
    },
    #[serde(rename_all = "camelCase")]
    McpToolCall {
        id: String,
        server: String,
        tool: String,
        status: ItemStatus,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<TurnError>,
    },
    #[serde(rename_all = "camelCase")]
    DynamicToolCall {
        id: String,
        tool: String,
        status: ItemStatus,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        success: Option<bool>,
        #[serde(default)]
        content_items: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    WebSearch {
        id: String,
        #[serde(default)]
        query: String,
    },
    #[serde(other)]
    Other,
}

/// Params shared by the thread-scoped delta notifications.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaParams {
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemParams {
    pub turn_id: String,
    pub item: ThreadItem,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnNotification {
    pub turn: TurnRef,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenBreakdown {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenUsageParams {
    #[serde(rename = "tokenUsage")]
    pub token_usage: ThreadTokenUsage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadTokenUsage {
    pub total: TokenBreakdown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorParams {
    pub error: TurnError,
    #[serde(default)]
    pub will_retry: bool,
}
