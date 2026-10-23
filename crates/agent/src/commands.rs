//! Harness-advertised composer commands. The UI never expands skill files or
//! implements an agent loop; execution belongs to the owning backend.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandKind {
    Skill,
    Action,
    Goal,
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub kind: CommandKind,
    pub source: String,
    /// Opaque provider data, for example Codex's fully qualified skill path.
    pub target: Option<String>,
}

/// Only a command at the beginning of the input is eligible for dispatch.
pub fn split_command(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start().strip_prefix('/')?;
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let name = &text[..end];
    (!name.is_empty()).then(|| (name, text[end..].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn goal_objective_retains_all_selected_skill_requirements() {
        let mut input = crate::UserInput::text("/docs /morning Write a report");
        input.skills = vec![
            ComposerCommand {
                name: "docs".into(),
                description: String::new(),
                argument_hint: String::new(),
                kind: CommandKind::Skill,
                source: "project".into(),
                target: Some("/repo/docs/SKILL.md".into()),
            },
            ComposerCommand {
                name: "morning".into(),
                description: String::new(),
                argument_hint: String::new(),
                kind: CommandKind::Skill,
                source: "Claude".into(),
                target: None,
            },
        ];
        let goal = input.goal_objective();
        assert!(goal.starts_with("Write a report\n"));
        assert!(goal.contains("/repo/docs/SKILL.md"));
        assert!(goal.contains("Invoke the /morning skill using the native Skill tool"));
        assert_eq!(
            crate::UserInput::text("ordinary goal").goal_objective(),
            "ordinary goal"
        );
    }

    #[test]
    fn selected_skill_survives_queue_serialization_and_old_messages_load() {
        let old: crate::UserInput =
            serde_json::from_str(r#"{"text":"hello","attachments":[]}"#).unwrap();
        assert!(old.command.is_none());
        let input = crate::UserInput {
            text: "/review check".into(),
            attachments: vec![],
            skills: vec![],
            command: Some(ComposerCommand {
                name: "review".into(),
                description: "Review".into(),
                argument_hint: String::new(),
                kind: CommandKind::Skill,
                source: "repo".into(),
                target: Some("/repo/SKILL.md".into()),
            }),
        };
        let restored: crate::UserInput =
            serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        assert_eq!(restored, input);
    }
    #[test]
    fn preserves_namespaced_commands_and_multiline_arguments() {
        assert_eq!(
            split_command(" /plugin:review fix\nthis"),
            Some(("plugin:review", "fix\nthis"))
        );
        assert_eq!(split_command("please /goal fix"), None);
        assert_eq!(split_command("/"), None);
    }
}

/// Provider-owned goal state. A completed turn does not mean a completed goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    pub time_used_seconds: u64,
    pub tokens_used: u64,
    pub can_pause: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl GoalStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Pursuing goal",
            Self::Paused => "Goal paused",
            Self::Blocked => "Goal blocked",
            Self::UsageLimited => "Goal usage limit reached",
            Self::BudgetLimited => "Goal budget reached",
            Self::Complete => "Goal complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalAction {
    Set(String),
    Pause,
    Resume,
    Clear,
}
