//! Restore the UI from Claude's on-disk transcript; resuming stream-json does
//! not replay earlier messages. Never send these records back to the provider.
use crate::protocol::Translator;
use del_agent::{AgentEvent, TurnStatus};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

pub(crate) fn load(cwd: &Path, id: &str) -> std::io::Result<Vec<AgentEvent>> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")));
    let Some(path) = root.and_then(|root| crate::titles::transcript_path(&root, cwd, id)) else {
        return Ok(vec![]);
    };
    read(BufReader::new(std::fs::File::open(path)?), id)
}

fn read(reader: impl BufRead, id: &str) -> std::io::Result<Vec<AgentEvent>> {
    let mut records = Vec::<Value>::new();
    let mut positions = HashMap::new();
    for line in reader.lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record["isSidechain"] == true || record["sessionId"].as_str().is_some_and(|s| s != id) {
            continue;
        }
        if let Some(uuid) = record["uuid"].as_str() {
            if let Some(&index) = positions.get(uuid) {
                records[index] = record;
                continue;
            }
            positions.insert(uuid.to_owned(), records.len());
        }
        records.push(record);
    }
    // Follow the latest conversation branch, including non-message records
    // which Claude inserts between messages. Exclude abandoned retry branches.
    let mut chain = HashSet::new();
    let mut cursor = records
        .iter()
        .rposition(|r| matches!(r["type"].as_str(), Some("user" | "assistant")));
    while let Some(index) = cursor {
        if !chain.insert(index) {
            break;
        }
        cursor = records[index]["parentUuid"]
            .as_str()
            .and_then(|id| positions.get(id))
            .copied();
    }
    let mut out = vec![];
    let mut translator = Translator::new();
    let mut turn: Option<String> = None;
    for (index, record) in records.iter().enumerate() {
        if !chain.contains(&index) {
            continue;
        }
        let content = &record["message"]["content"];
        if record["type"] == "user" && record["isMeta"] != true {
            let text = content.as_str().map(str::to_owned).unwrap_or_else(|| {
                content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| match b["type"].as_str() {
                        Some("text") => b["text"].as_str(),
                        Some("image") => Some("[Image]"),
                        Some("document") => Some("[Document]"),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            if !text.is_empty() {
                if let Some(turn_id) = turn.take() {
                    out.push(AgentEvent::TurnCompleted {
                        turn_id,
                        status: TurnStatus::Completed,
                    });
                }
                translator = Translator::new();
                let turn_id = record["uuid"].as_str().unwrap_or("history").to_owned();
                out.push(AgentEvent::UserMessage { text });
                out.extend(translator.enqueue_turn(turn_id.clone()));
                turn = Some(turn_id);
            }
        }
        if record["type"] == "assistant" {
            for block in content.as_array().into_iter().flatten() {
                if let Some(text) = block["thinking"].as_str().filter(|s| !s.is_empty()) {
                    out.push(AgentEvent::ReasoningDelta {
                        turn_id: turn.clone().unwrap_or_default(),
                        text: text.to_owned(),
                    });
                }
            }
        }
        out.extend(translator.handle_line(&record.to_string()));
    }
    if let Some(turn_id) = turn {
        out.push(AgentEvent::TurnCompleted {
            turn_id,
            status: TurnStatus::Completed,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn restores_messages_thinking_tools_and_latest_branch_once() {
        let records = [
            json!({"type":"user","uuid":"u","parentUuid":null,"message":{"content":"hello"}}),
            json!({"type":"assistant","uuid":"old","parentUuid":"u","message":{"content":[{"type":"text","text":"discard"}]}}),
            json!({"type":"attachment","uuid":"meta","parentUuid":"u"}),
            json!({"type":"assistant","uuid":"a","parentUuid":"meta","message":{"content":[{"type":"thinking","thinking":"reason"},{"type":"tool_use","id":"t","name":"Bash","input":{"command":"pwd"}}]}}),
            json!({"type":"user","uuid":"r","parentUuid":"a","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"/tmp"}]}}),
            json!({"type":"assistant","uuid":"b","parentUuid":"r","message":{"content":[{"type":"text","text":"answer"}]}}),
            json!({"type":"assistant","uuid":"b","parentUuid":"r","message":{"content":[{"type":"text","text":"answer"}]}}),
            json!({"type":"user","uuid":"s","parentUuid":"b","isSidechain":true,"message":{"content":"hidden"}}),
        ];
        let input = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n{partial";
        let events = read(input.as_bytes(), "test").unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::UserMessage { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::AssistantMessage { .. }))
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AssistantMessage {text, ..} if text == "answer"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ReasoningDelta {text, ..} if text == "reason"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolCallFinished {id, ..} if id == "t"))
        );
        assert!(matches!(
            events.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }
}
