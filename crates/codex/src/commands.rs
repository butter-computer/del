use del_agent::{CommandKind, ComposerCommand};
use serde_json::{Value, json};

pub fn catalog(response: &Value) -> Vec<ComposerCommand> {
    let mut commands = Vec::new();
    for entry in response["data"].as_array().into_iter().flatten() {
        for skill in entry["skills"].as_array().into_iter().flatten() {
            if skill["enabled"] != true {
                continue;
            }
            let (Some(name), Some(path)) = (skill["name"].as_str(), skill["path"].as_str()) else {
                continue;
            };
            commands.push(ComposerCommand {
                name: name.into(),
                description: skill["interface"]["shortDescription"]
                    .as_str()
                    .or(skill["shortDescription"].as_str())
                    .or(skill["description"].as_str())
                    .unwrap_or_default()
                    .into(),
                argument_hint: String::new(),
                kind: CommandKind::Skill,
                source: skill["scope"].as_str().unwrap_or("Skill").into(),
                target: Some(path.into()),
            });
        }
    }
    commands.extend([
        ComposerCommand {
            name: "goal".into(),
            description: "Keep working toward an objective".into(),
            argument_hint: "<objective> · pause · resume · clear".into(),
            kind: CommandKind::Goal,
            source: "Codex".into(),
            target: None,
        },
        ComposerCommand {
            name: "fast".into(),
            description: "Faster responses · increased usage".into(),
            argument_hint: "on · off".into(),
            kind: CommandKind::Fast,
            source: "Codex".into(),
            target: None,
        },
    ]);
    commands
}

pub fn goal_request(thread: &str, arguments: &str) -> (&'static str, Value) {
    let mut params = json!({"threadId": thread});
    let method = match arguments.trim() {
        "" | "status" => "thread/goal/get",
        "clear" => "thread/goal/clear",
        "pause" => {
            params["status"] = json!("paused");
            "thread/goal/set"
        }
        "resume" => {
            params["status"] = json!("active");
            "thread/goal/set"
        }
        objective => {
            params["objective"] = json!(objective);
            params["status"] = json!("active");
            "thread/goal/set"
        }
    };
    (method, params)
}

pub fn goal_message(method: &str, response: &Value) -> String {
    if method == "thread/goal/clear" {
        return "Goal cleared.".into();
    }
    let goal = &response["goal"];
    let Some(objective) = goal["objective"].as_str() else {
        return "No goal set. Use `/goal <objective>` to start one.".into();
    };
    let status = goal["status"].as_str().unwrap_or("unknown");
    format!(
        "Goal · {status}\n\n{objective}\n\n{} tokens used · `/goal pause` · `/goal resume` · `/goal clear`",
        goal["tokensUsed"].as_u64().unwrap_or(0)
    )
}

/// Parse only native goal records; missing/unknown status is not success.
pub fn goal(value: &Value) -> Option<del_agent::Goal> {
    use del_agent::{Goal, GoalStatus};
    Some(Goal {
        objective: value["objective"].as_str()?.into(),
        status: match value["status"].as_str()? {
            "active" => GoalStatus::Active,
            "paused" => GoalStatus::Paused,
            "blocked" => GoalStatus::Blocked,
            "usageLimited" => GoalStatus::UsageLimited,
            "budgetLimited" => GoalStatus::BudgetLimited,
            "complete" => GoalStatus::Complete,
            _ => return None,
        },
        time_used_seconds: value["timeUsedSeconds"].as_u64().unwrap_or(0),
        tokens_used: value["tokensUsed"].as_u64().unwrap_or(0),
        can_pause: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_goal_operations_do_not_become_objectives() {
        assert_eq!(
            goal_request("t", "pause"),
            ("thread/goal/set", json!({"threadId":"t","status":"paused"}))
        );
        assert_eq!(goal_request("t", "resume").1["status"], "active");
        assert_eq!(goal_request("t", "").0, "thread/goal/get");
        assert_eq!(goal_request("t", "clear").0, "thread/goal/clear");
        assert_eq!(goal_request("t", "ship it").1["objective"], "ship it");
    }
    #[test]
    fn disabled_skills_are_excluded_and_paths_preserved() {
        let rows = catalog(&json!({"data":[{"skills":[
            {"name":"review","path":"/project/SKILL.md","description":"Review","scope":"repo","enabled":true},
            {"name":"hidden","path":"/hidden/SKILL.md","enabled":false}
        ]}]}));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].target.as_deref(), Some("/project/SKILL.md"));
    }
}
