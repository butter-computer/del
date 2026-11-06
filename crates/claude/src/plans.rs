//! Read successful task-tool results before their structured data is summarized.
use del_agent::{AgentEvent, PlanUpdate};
use serde_json::Value;

pub fn completed(name: &str, input: &Value, output: &Value) -> Option<AgentEvent> {
    let update = match name {
        "TodoWrite" => PlanUpdate::Replace {
            steps: del_agent::plan::todo_steps(&input["todos"])?,
            explanation: None,
        },
        "TaskCreate" => PlanUpdate::Upsert {
            id: output["task"]["id"].as_str()?.to_owned(),
            text: Some(input["subject"].as_str()?.to_owned()),
            active_text: active_text(input),
            status: Some(del_agent::PlanStatus::Pending),
        },
        "TaskUpdate" => {
            if output["success"] == false {
                return None;
            }
            let id = ["taskId", "id", "task_id"]
                .iter()
                .find_map(|key| input[key].as_str())?
                .to_owned();
            if input["status"] == "deleted" {
                PlanUpdate::Remove { id }
            } else {
                let status = match input["status"].as_str() {
                    Some(value) => Some(del_agent::plan::status(value)?),
                    None => None,
                };
                PlanUpdate::Upsert {
                    id,
                    text: input["subject"].as_str().map(str::to_owned),
                    active_text: active_text(input),
                    status,
                }
            }
        }
        _ => return None,
    };
    Some(AgentEvent::PlanUpdated {
        id: if name == "TodoWrite" {
            "claude:todos"
        } else {
            "claude:tasks"
        }
        .into(),
        update,
    })
}

fn active_text(value: &Value) -> Option<String> {
    value["activeForm"]
        .as_str()
        .or_else(|| value["active_form"].as_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Translator;
    use serde_json::json;
    #[test]
    fn replay_real_claude_task_progress() {
        // Recorded from the installed CLI; unrelated metadata was removed.
        let mut translator = Translator::new();
        let mut plan = del_agent::Plan::default();
        let mut statuses = Vec::new();
        for line in include_str!("../tests/fixtures/plan_tasks.jsonl").lines() {
            for event in translator.handle_line(line) {
                if let AgentEvent::PlanUpdated { update, .. } = event {
                    plan.apply(&update);
                    statuses.push(plan.steps[0].status);
                }
            }
        }
        assert_eq!(
            statuses,
            vec![
                del_agent::PlanStatus::Pending,
                del_agent::PlanStatus::InProgress,
                del_agent::PlanStatus::Completed
            ]
        );
        assert_eq!(plan.steps[0].text, "Verify checklist rendering");
    }
    #[test]
    fn restore_real_native_history_keeps_an_unfinished_task_unfinished() {
        let source = include_str!("../tests/fixtures/plan_history.jsonl");
        let first: Value = serde_json::from_str(source.lines().next().unwrap()).unwrap();
        let events = crate::history::read(
            std::io::Cursor::new(source),
            first["sessionId"].as_str().unwrap(),
        )
        .unwrap();
        let mut plan = del_agent::Plan::default();
        for event in events {
            if let AgentEvent::PlanUpdated { update, .. } = event {
                plan.apply(&update);
            }
        }
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].text, "Verify plan restoration");
        assert_eq!(plan.steps[0].status, del_agent::PlanStatus::InProgress);
    }

    #[test]
    fn task_results_correlate_ids_and_failed_updates_do_not_change_progress() {
        let mut translator = Translator::new();
        translator.enqueue_turn("turn".into());
        let mut plan = del_agent::Plan::default();
        for (call, name, input, output, failed) in [
            (
                "create",
                "TaskCreate",
                json!({"subject":"Read the repo", "activeForm":"Reading the repo"}),
                json!({"task":{"id":"42","subject":"Read the repo"}}),
                false,
            ),
            (
                "start",
                "TaskUpdate",
                json!({"taskId":"42","status":"in_progress"}),
                json!({"success":true}),
                false,
            ),
            (
                "failed",
                "TaskUpdate",
                json!({"taskId":"42","status":"completed"}),
                json!({}),
                true,
            ),
        ] {
            let events = translator.handle_value(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":call,"name":name,"input":input}]}}));
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, AgentEvent::PlanUpdated { .. }))
            );
            let events = translator.handle_value(&json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":call,"is_error":failed,"content":"result"}]},"tool_use_result":output}));
            for event in events {
                if let AgentEvent::PlanUpdated { update, .. } = event {
                    plan.apply(&update);
                }
            }
        }
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].id, "42");
        assert_eq!(plan.steps[0].status, del_agent::PlanStatus::InProgress);
        assert_eq!(
            plan.steps[0].active_text.as_deref(),
            Some("Reading the repo")
        );
        let AgentEvent::PlanUpdated { update, .. } = completed(
            "TaskUpdate",
            &json!({"taskId":"42","status":"deleted"}),
            &json!({}),
        )
        .unwrap() else {
            panic!()
        };
        plan.apply(&update);
        assert!(plan.steps.is_empty());
        assert!(completed("TaskCreate", &json!({"subject":"Missing ID"}), &json!({})).is_none());
        assert!(
            completed(
                "TaskUpdate",
                &json!({"taskId":"42","status":"surprise"}),
                &json!({})
            )
            .is_none()
        );
    }

    #[test]
    fn legacy_todos_are_full_snapshots_and_empty_clears_them() {
        let AgentEvent::PlanUpdated {
            update: PlanUpdate::Replace { steps, .. },
            ..
        } = completed(
            "TodoWrite",
            &json!({"todos":[{"content":"Check", "status":"completed", "activeForm":"Checking"}]}),
            &json!({}),
        )
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(steps[0].status, del_agent::PlanStatus::Completed);
        assert_eq!(steps[0].active_text.as_deref(), Some("Checking"));
        assert!(
            completed(
                "TodoWrite",
                &json!({"todos":[{"content":"Check", "status":"unknown"}]}),
                &json!({})
            )
            .is_none()
        );
        assert!(
            matches!(completed("TodoWrite", &json!({"todos":[]}), &json!({})), Some(AgentEvent::PlanUpdated {update: PlanUpdate::Replace {steps, ..}, ..}) if steps.is_empty())
        );
    }
}
