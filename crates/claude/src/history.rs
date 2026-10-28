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

pub fn load(cwd: &Path, id: &str) -> std::io::Result<Vec<AgentEvent>> {
    let Some(path) = path(cwd, id) else {
        return Ok(vec![]);
    };
    read(BufReader::new(std::fs::File::open(path)?), id)
}

pub fn load_from(root: &Path, cwd: &Path, id: &str) -> std::io::Result<Vec<AgentEvent>> {
    let Some(path) = crate::titles::transcript_path(root, cwd, id) else {
        return Ok(vec![]);
    };
    read(BufReader::new(std::fs::File::open(path)?), id)
}

pub fn path_from(root: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    crate::titles::transcript_path(root, cwd, id)
}

/// Locate saved messages without resuming the CLI.
pub fn path(cwd: &Path, id: &str) -> Option<PathBuf> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;
    crate::titles::transcript_path(&root, cwd, id)
}

/// Transcript records larger than this (e.g. a huge pasted file) are skipped.
const MAX_RECORD_BYTES: u64 = 32 * 1024 * 1024;

/// Next `\n`-terminated line, without its terminator. Lines over `max`
/// bytes are consumed and returned empty so they parse as nothing.
fn next_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: u64) -> std::io::Result<bool> {
    use std::io::Read;
    buf.clear();
    if reader.by_ref().take(max + 1).read_until(b'\n', buf)? == 0 {
        return Ok(false);
    }
    if buf.last() != Some(&b'\n') && buf.len() as u64 > max {
        // Oversized: discard the rest of it.
        buf.clear();
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                break;
            }
            match available.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    reader.consume(i + 1);
                    break;
                }
                None => {
                    let len = available.len();
                    reader.consume(len);
                }
            }
        }
    }
    Ok(true)
}

pub fn read(mut reader: impl BufRead, id: &str) -> std::io::Result<Vec<AgentEvent>> {
    let mut records = Vec::<Value>::new();
    let mut positions = HashMap::new();
    let mut line = Vec::new();
    while next_line(&mut reader, &mut line, MAX_RECORD_BYTES)? {
        // Lossy so one bad byte drops (at most) one record, not the whole transcript.
        let Ok(record) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) else {
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
        out.extend(translator.handle_value(record));
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
    fn oversized_and_invalid_records_are_skipped_not_fatal() {
        let ok = json!({"type":"user","uuid":"u","parentUuid":null,"message":{"content":"hello"}});
        let mut input = b"\xff\xfe not json\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 64));
        input.push(b'\n');
        input.extend(ok.to_string().as_bytes());
        input.push(b'\n');
        let mut reader = std::io::BufReader::with_capacity(8, input.as_slice());
        let mut line = Vec::new();
        let mut lines = Vec::new();
        while next_line(&mut reader, &mut line, 32).unwrap() {
            lines.push(line.len());
        }
        assert_eq!(lines[1], 0, "oversized line is dropped");
        assert_eq!(lines.len(), 3);
        let events = read(input.as_slice(), "test").unwrap();
        assert!(matches!(&events[0], AgentEvent::UserMessage { text } if text == "hello"));
    }

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
