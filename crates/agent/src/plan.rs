//! Provider-authored progress. Completion is never inferred from turn lifecycle.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_text: Option<String>,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanUpdate {
    Replace {
        steps: Vec<PlanStep>,
        explanation: Option<String>,
    },
    /// Task tools may update a status without repeating the task's title.
    Upsert {
        id: String,
        text: Option<String>,
        active_text: Option<String>,
        status: Option<PlanStatus>,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    pub explanation: Option<String>,
}

impl Plan {
    pub fn apply(&mut self, update: &PlanUpdate) {
        match update {
            PlanUpdate::Replace { steps, explanation } => {
                self.steps = steps.clone();
                self.explanation = explanation.clone();
            }
            PlanUpdate::Upsert {
                id,
                text,
                active_text,
                status,
            } => {
                if let Some(step) = self.steps.iter_mut().find(|step| &step.id == id) {
                    if let Some(text) = text {
                        step.text = text.clone();
                    }
                    if let Some(text) = active_text {
                        step.active_text = Some(text.clone());
                    }
                    if let Some(status) = status {
                        step.status = *status;
                    }
                } else if let Some(text) = text {
                    self.steps.push(PlanStep {
                        id: id.clone(),
                        text: text.clone(),
                        active_text: active_text.clone(),
                        status: status.unwrap_or(PlanStatus::Pending),
                    });
                }
            }
            PlanUpdate::Remove { id } => self.steps.retain(|step| &step.id != id),
        }
    }
}

/// The todo schema shared by Claude's legacy TodoWrite and OpenCode v2.
/// Reject a malformed snapshot atomically; never silently drop unknown states.
pub fn todo_steps(value: &serde_json::Value) -> Option<Vec<PlanStep>> {
    value
        .as_array()?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            Some(PlanStep {
                id: index.to_string(),
                text: item["content"].as_str()?.to_owned(),
                active_text: item["activeForm"].as_str().map(str::to_owned),
                status: status(item["status"].as_str()?)?,
            })
        })
        .collect()
}

pub fn status(value: &str) -> Option<PlanStatus> {
    Some(match value {
        "pending" => PlanStatus::Pending,
        "in_progress" | "inProgress" => PlanStatus::InProgress,
        "completed" => PlanStatus::Completed,
        "cancelled" => PlanStatus::Cancelled,
        _ => return None,
    })
}
