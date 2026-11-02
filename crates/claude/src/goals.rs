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
            // Bytes, not `read_line`: invalid UTF-8 (or a write caught mid
            // character) must not fail the read and drop the events before it.
            let mut line = Vec::new();
            let bytes = reader.read_until(b'\n', &mut line)?;
            if bytes == 0 || line.last() != Some(&b'\n') {
                break;
            }
            self.offset += bytes as u64;
            let Ok(value) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) else {
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

    #[test]
    fn invalid_utf8_and_split_characters_do_not_lose_goal_updates() {
        use std::io::Write;
        let path =
            std::env::temp_dir().join(format!("del-goal-utf8-{}.jsonl", uuid::Uuid::new_v4()));
        let goal = |objective: &str| {
            json!({"type":"attachment","sessionId":"one","attachment":{
                "type":"goal_status","condition":objective,"met":false}})
            .to_string()
        };
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "{}", goal("first")).unwrap();
        file.write_all(b"{\"type\":\"user\",\"text\":\"\xff\xfe\"}\n")
            .unwrap();
        writeln!(file, "{}", goal("second")).unwrap();
        // A record caught mid-write, ending inside a multi-byte character.
        let third = goal("thïrd");
        let split = third.find('ï').unwrap() + 1;
        file.write_all(&third.as_bytes()[..split]).unwrap();
        file.flush().unwrap();
        let mut reader = Reader::new();
        let objectives = |events: Vec<AgentEvent>| -> Vec<String> {
            events
                .into_iter()
                .filter_map(|e| match e {
                    AgentEvent::GoalUpdated { goal: Some(goal) } => Some(goal.objective),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            objectives(reader.read(&path, "one").unwrap()),
            ["first", "second"]
        );
        file.write_all(&third.as_bytes()[split..]).unwrap();
        writeln!(file).unwrap();
        file.flush().unwrap();
        assert_eq!(objectives(reader.read(&path, "one").unwrap()), ["thïrd"]);
        std::fs::remove_file(path).unwrap();
    }
}
