//! Provider-neutral data types shared by every backend and UI consumer.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Claude,
    /// Any agent speaking the Agent Client Protocol. `SessionConfig::model`
    /// names the registry entry (`"<agent>"` or `"<agent>/<model>"`).
    Acp,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Codex, Provider::Claude, Provider::Acp];

    /// Executable name of the provider's CLI.
    pub fn binary(self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Claude => "claude",
            // ACP agents are launched from the registry, not a fixed binary.
            Provider::Acp => "",
        }
    }
}

/// Opaque, backend-assigned session identifier (the provider's thread/session id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How much the agent may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Ask before running commands or editing files.
    #[default]
    Ask,
    /// Auto-approve file edits, ask for everything else.
    AutoEdit,
    /// Read-only planning; no edits or commands.
    Plan,
    /// Never ask. Use with care.
    FullAuto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub cwd: PathBuf,
    /// Provider model id; `None` means the provider's default.
    pub model: Option<String>,
    /// Provider reasoning effort; absent preserves the provider default.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    pub provider: Provider,
    pub permission_mode: PermissionMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserInput {
    pub text: String,
    /// Preserve an explicit menu selection through queues and draft startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<crate::ComposerCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<crate::ComposerCommand>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl UserInput {
    /// Native goal APIs accept an objective, not structured skill input items.
    /// Preserve explicit skill identities as requirements of the persistent goal.
    pub fn goal_objective(&self) -> String {
        if self.skills.is_empty() {
            return self.text.clone();
        }
        let mut objective = self.text.trim();
        for skill in &self.skills {
            if let Some(rest) = objective.strip_prefix(&format!("/{}", skill.name))
                && (rest.is_empty() || rest.starts_with(char::is_whitespace))
            {
                objective = rest.trim_start();
            }
        }
        let requirements = self
            .skills
            .iter()
            .map(|skill| match &skill.target {
                Some(path) => format!(
                    "- Use /{} by reading and following the skill at {}.",
                    skill.name,
                    serde_json::to_string(path).expect("path serializes")
                ),
                None => format!(
                    "- Invoke the /{} skill using the native Skill tool.",
                    skill.name
                ),
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{objective}\n\nRequired skills for this goal:\n{requirements}")
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            command: None,
            skills: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Completed,
    Failed,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Command,
    FileChange,
    Other,
}

/// The user's answer to an `ApprovalRequested` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResponse {
    Approve,
    Deny,
}

/// Normalized event stream; identical shape for every provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    GoalUpdated {
        goal: Option<crate::Goal>,
    },
    /// A user message restored from the provider's saved transcript.
    UserMessage {
        text: String,
    },
    SessionStarted {
        session_id: SessionId,
    },
    TurnStarted {
        turn_id: String,
    },
    AssistantTextDelta {
        turn_id: String,
        text: String,
    },
    /// Final text of one assistant message (after its deltas, if any).
    AssistantMessage {
        turn_id: String,
        text: String,
    },
    ReasoningDelta {
        turn_id: String,
        text: String,
    },
    ToolCallStarted {
        id: String,
        name: String,
        input_summary: String,
    },
    ToolCallUpdated {
        id: String,
        output_summary: String,
    },
    ToolCallFinished {
        id: String,
        status: ToolStatus,
        output_summary: String,
    },
    FileChange {
        path: PathBuf,
        kind: FileChangeKind,
        diff: Option<String>,
    },
    ApprovalRequested {
        id: String,
        kind: ApprovalKind,
        detail: String,
    },
    TokenUsage {
        input: u64,
        output: u64,
        cached: u64,
    },
    TurnCompleted {
        turn_id: String,
        status: TurnStatus,
    },
    Error {
        message: String,
        recoverable: bool,
    },
}

/// Coarse per-session state tracked by the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    WaitingApproval,
    Error,
}
