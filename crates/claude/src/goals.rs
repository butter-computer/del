//! Goal state comes from Claude's native transcript records. Reading status
//! must never send a message, which could wake an interrupted native goal.
use del_agent::{AgentEvent, Goal, GoalStatus};
use serde_json::Value;
use std::io::{BufRead, BufReader};

pub fn record(value: &Value) -> Option<AgentEvent> {
    let a = &value["attachment"];
    if value["type"] != "attachment" || a["type"] != "goal_status" {
        return None;
    }
    if a["sentinel"] == true && a["met"] == true {
        return Some(AgentEvent::GoalUpdated { goal: None });
    }
    let objective = a["condition"].as_str()?.to_owned();
    Some(AgentEvent::GoalUpdated {
        goal: Some(Goal {
            objective,
            status: if a["met"] == true {
                GoalStatus::Complete
            } else if a["failed"] == true {
                GoalStatus::Blocked
            } else {
                GoalStatus::Active
            },
            time_used_seconds: a["durationMs"].as_u64().unwrap_or(0) / 1000,
            tokens_used: a["tokens"].as_u64().unwrap_or(0),
            can_pause: false,
        }),
    })
}

/// Follow only newly appended complete JSONL records, retaining partial lines.
pub struct Reader {
    offset: u64,
}
impl Reader {
    pub fn new() -> Self {
        Self { offset: 0 }
    }
    pub fn read(
        &mut self,
        path: &std::path::Path,
        session: &str,
    ) -> std::io::Result<Vec<AgentEvent>> {
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::File::open(path)?;
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let mut events = vec![];
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 || !line.ends_with('\n') {
                break;
            }
            self.offset += bytes as u64;
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["isSidechain"] == true
                || value["sessionId"].as_str().is_some_and(|id| id != session)
            {
                continue;
            }
            if let Some(event) = record(&value) {
                events.push(event);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_status_distinguishes_completion_clear_and_failure() {
        let event =
            |fields: Value| record(&json!({"type":"attachment","attachment":fields})).unwrap();
        assert!(matches!(
            event(json!({"type":"goal_status","condition":"ship","met":false,"sentinel":true})),
            AgentEvent::GoalUpdated {
                goal: Some(Goal {
                    status: GoalStatus::Active,
                    ..
                })
            }
        ));
        assert!(matches!(
            event(
                json!({"type":"goal_status","condition":"ship","met":true,"durationMs":4200,"tokens":50})
            ),
            AgentEvent::GoalUpdated {
                goal: Some(Goal {
                    status: GoalStatus::Complete,
                    time_used_seconds: 4,
                    tokens_used: 50,
                    ..
                })
            }
        ));
        assert_eq!(
            event(json!({"type":"goal_status","condition":"ship","met":true,"sentinel":true})),
            AgentEvent::GoalUpdated { goal: None }
        );
        assert!(matches!(
            event(json!({"type":"goal_status","condition":"ship","met":false,"failed":true})),
            AgentEvent::GoalUpdated {
                goal: Some(Goal {
                    status: GoalStatus::Blocked,
                    ..
                })
            }
        ));
        assert!(
            record(&json!({"type":"assistant","message":{"content":"Goal set: fake"}})).is_none()
        );
    }
    #[test]
    fn incremental_reader_keeps_partial_lines_and_filters_other_sessions() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("del-goal-reader-{}.jsonl", uuid::Uuid::new_v4()));
        let mut file = std::fs::File::create(&path).unwrap();
        let entry = json!({"type":"attachment","sessionId":"one","attachment":{"type":"goal_status","condition":"ship","met":false,"sentinel":true}}).to_string();
        write!(file, "{entry}").unwrap();
        file.flush().unwrap();
        let mut reader = Reader::new();
        assert!(reader.read(&path, "one").unwrap().is_empty());
        writeln!(file).unwrap();
        file.flush().unwrap();
        assert_eq!(reader.read(&path, "one").unwrap().len(), 1);
        assert!(reader.read(&path, "one").unwrap().is_empty());
        writeln!(file, "{}", entry.replace("one", "two")).unwrap();
        file.flush().unwrap();
        assert!(reader.read(&path, "one").unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
