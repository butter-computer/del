//! Restore the UI from Claude's on-disk transcript; resuming stream-json does
//! not replay earlier messages. Never send these records back to the provider.
use crate::protocol::Translator;
use del_agent::{AgentEvent, ToolStatus, TurnStatus};
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
    let mut local_parent: Option<&str> = None;
    let mut local_tool: Option<String> = None;
    for (index, record) in records.iter().enumerate() {
        if !chain.contains(&index) {
            continue;
        }
        let content = &record["message"]["content"];
        // Claude stores CLI-only commands as user records linked to a metadata
        // caveat. Require that provenance: ordinary users may paste the same XML.
        let native_text = content.as_str().or_else(|| {
            let blocks = content.as_array()?;
            (blocks.len() == 1 && blocks[0]["type"] == "text")
                .then(|| blocks[0]["text"].as_str())
                .flatten()
        });
        if record["type"] == "user"
            && record["origin"]["kind"] == "task-notification"
            && let Some(body) = native_text.and_then(|text| wrapped(text, "task-notification"))
            && let Some(summary) = field(body, "summary")
        {
            let id = format!("notification-{}", record["uuid"].as_str().unwrap_or("task"));
            out.push(AgentEvent::ToolCallStarted {
                id: id.clone(),
                name: "background_task".into(),
                input_summary: summary.into(),
            });
            out.push(AgentEvent::ToolCallFinished {
                id,
                status: if field(body, "status") == Some("failed") {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Completed
                },
                output_summary: field(body, "output-file")
                    .map(|path| format!("Output: {path}"))
                    .unwrap_or_default(),
            });
            local_parent = None;
            local_tool = None;
            continue;
        }
        let caveat = record["type"] == "user"
            && record["isMeta"] == true
            && native_text.is_some_and(|text| wrapped(text, "local-command-caveat").is_some());
        let local = record["type"] == "user"
            && local_parent.is_some()
            && record["parentUuid"].as_str() == local_parent;
        if caveat {
            local_parent = record["uuid"].as_str();
            local_tool = None;
            continue;
        }
        if local && let Some(text) = native_text {
            if let Some(command) = local_command(text) {
                let id = format!("local-{}", record["uuid"].as_str().unwrap_or("command"));
                out.push(AgentEvent::ToolCallStarted {
                    id: id.clone(),
                    name: "terminal_command".into(),
                    input_summary: command,
                });
                out.push(AgentEvent::ToolCallFinished {
                    id: id.clone(),
                    status: ToolStatus::Completed,
                    output_summary: String::new(),
                });
                local_tool = Some(id);
                local_parent = record["uuid"].as_str();
                continue;
            }
            let output = wrapped(text, "local-command-stdout")
                .map(|text| (text, ToolStatus::Completed))
                .or_else(|| {
                    wrapped(text, "local-command-stderr").map(|text| (text, ToolStatus::Failed))
                });
            if let Some((output, status)) = output
                && let Some(id) = &local_tool
            {
                out.push(AgentEvent::ToolCallFinished {
                    id: id.clone(),
                    status,
                    output_summary: output.to_owned(),
                });
                local_parent = record["uuid"].as_str();
                continue;
            }
        }
        local_parent = None;
        local_tool = None;
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
            if turn.is_none() {
                // A reply with no visible user message before it (e.g. a meta
                // or empty prompt) still needs a turn that history closes;
                // otherwise the translator opens a native turn left running.
                let turn_id = record["uuid"].as_str().unwrap_or("history").to_owned();
                out.extend(translator.enqueue_turn(turn_id.clone()));
                turn = Some(turn_id);
            }
            for block in content.as_array().into_iter().flatten() {
                if let Some(text) = block["thinking"].as_str().filter(|s| !s.is_empty()) {
                    out.push(AgentEvent::ReasoningDelta {
                        turn_id: turn.clone().unwrap_or_default(),
                        text: text.to_owned(),
                    });
                }
            }
        }
        out.extend(
            translator
                .handle_value(record)
                .into_iter()
                .map(|mut event| {
                    // Native history doesn't retain CLI process boundaries. Keep context,
                    // but don't reinterpret cumulative cost as new spend on every resume.
                    if let AgentEvent::UsageSnapshot {
                        cost_scope,
                        cost_usd,
                        ..
                    } = &mut event
                        && !cost_scope.as_deref().is_some_and(|scope| {
                            scope.starts_with("estimate:") || scope.contains(":estimate:")
                        })
                    {
                        *cost_scope = None;
                        *cost_usd = None;
                    }
                    event
                }),
        );
    }
    if let Some(turn_id) = turn {
        out.push(AgentEvent::TurnCompleted {
            turn_id,
            status: TurnStatus::Completed,
        });
    }
    del_agent::history::clean_events(&mut out);
    Ok(out)
}

fn wrapped<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    text.trim()
        .strip_prefix(&format!("<{tag}>"))?
        .strip_suffix(&format!("</{tag}>"))
}

fn field<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    text.split_once(&format!("<{tag}>"))?
        .1
        .split_once(&format!("</{tag}>"))
        .map(|(value, _)| value)
}

fn local_command(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix("<command-name>")?;
    let (name, rest) = rest.split_once("</command-name>")?;
    if !name.starts_with('/') || name.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix("<command-message>")?;
    let (_, rest) = rest.split_once("</command-message>")?;
    let args = wrapped(rest, "command-args")?.trim();
    Some(if args.is_empty() {
        name.into()
    } else {
        format!("{name} {args}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_local_commands_become_activity_without_consuming_real_messages() {
        let command = "<command-name>/terminal-setup</command-name>\n <command-message>terminal-setup</command-message>\n <command-args></command-args>";
        let mut records = [
            json!({"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>CLI output only</local-command-caveat>"}}),
            json!({"type":"user","message":{"content":command}}),
            json!({"type":"user","message":{"content":"<local-command-stdout>Setup note \u{1b}[2mwith formatting\u{1b}[22m</local-command-stdout>"}}),
            json!({"type":"user","message":{"content":"cool twin cool"}}),
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"\u{1b}[32mAll good\u{1b}[0m"}]}}),
            // Identical XML typed by the user has no native caveat provenance.
            json!({"type":"user","message":{"content":command}}),
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"Literal <local-command-stdout>example</local-command-stdout>"}]}}),
        ];
        for (i, record) in records.iter_mut().enumerate() {
            record["uuid"] = json!(i.to_string());
            record["parentUuid"] = if i == 0 {
                Value::Null
            } else {
                json!((i - 1).to_string())
            };
        }
        let input = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let events = read(input.as_bytes(), "test").unwrap();
        let users: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let AgentEvent::UserMessage { text } = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(users, ["cool twin cool", command]);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { name, input_summary, .. } if name == "terminal_command" && input_summary == "/terminal-setup")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallFinished { output_summary, .. } if output_summary == "Setup note with formatting")));
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "All good")
            )
        );
        assert!(events.iter().any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "Literal <local-command-stdout>example</local-command-stdout>")));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }

    #[test]
    fn native_task_notifications_are_activity_but_user_examples_are_preserved() {
        let text = "<task-notification><status>completed</status><summary>Background command finished</summary><output-file>/tmp/output</output-file></task-notification>";
        let records = [
            json!({"type":"user","uuid":"n","origin":{"kind":"task-notification"},"message":{"content":text}}),
            json!({"type":"user","uuid":"u","parentUuid":"n","message":{"content":text}}),
        ];
        let input = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let events = read(input.as_bytes(), "test").unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::UserMessage { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { input_summary, .. } if input_summary == "Background command finished")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallFinished { output_summary, .. } if output_summary == "Output: /tmp/output")));
    }

    #[test]
    fn command_envelopes_require_exact_structure() {
        assert_eq!(
            local_command(
                "<command-name>/model</command-name><command-message>model</command-message><command-args>sonnet</command-args>"
            ),
            Some("/model sonnet".into())
        );
        assert!(local_command("Explain <command-name>/model</command-name>").is_none());
        assert!(
            wrapped(
                "<local-command-stdout>example</local-command-stdout> extra",
                "local-command-stdout"
            )
            .is_none()
        );
    }

    #[test]
    #[ignore = "reads a user-selected native transcript via DEL_HISTORY_TEST_FILE"]
    fn actual_terminal_transcript_restores_clean_command_activity() {
        let path = std::env::var("DEL_HISTORY_TEST_FILE").unwrap();
        let id = std::path::Path::new(&path)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap();
        let events = read(BufReader::new(std::fs::File::open(&path).unwrap()), id).unwrap();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted {input_summary, ..} if input_summary == "/terminal-setup")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallFinished {output_summary, ..} if output_summary.contains("Terminal setup cannot be run from del.") && !output_summary.contains('\u{1b}') && !output_summary.contains("<local-command-stdout>"))));
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::UserMessage {text} if text.starts_with("<command-name>") || text.starts_with("<local-command-stdout>"))));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::UserMessage {text} if text == "cool twin cool"))
        );
    }

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

    fn transcript(records: &[Value]) -> String {
        records.iter().map(|r| format!("{r}\n")).collect()
    }

    /// Every turn history opens is also closed, so a restored chat is idle.
    fn assert_balanced(events: &[AgentEvent]) {
        let mut open = Vec::new();
        for event in events {
            match event {
                AgentEvent::TurnStarted { turn_id } => open.push(turn_id.clone()),
                AgentEvent::TurnCompleted { turn_id, .. } => {
                    assert_eq!(open.pop().as_ref(), Some(turn_id), "{events:?}")
                }
                _ => {}
            }
        }
        assert!(open.is_empty(), "turns left running: {open:?}");
    }

    #[test]
    fn reply_without_a_visible_prompt_is_closed_not_left_running() {
        let records = [
            json!({"type":"user","uuid":"m","parentUuid":null,"isMeta":true,"message":{"content":"meta"}}),
            json!({"type":"assistant","uuid":"a","parentUuid":"m","message":{"content":[
                {"type":"thinking","thinking":"plan"},{"type":"text","text":"orphan reply"}]}}),
            json!({"type":"user","uuid":"u","parentUuid":"a","message":{"content":"next"}}),
            json!({"type":"assistant","uuid":"b","parentUuid":"u","message":{"content":[{"type":"text","text":"answer"}]}}),
        ];
        let events = read(transcript(&records).as_bytes(), "s").unwrap();
        assert_balanced(&events);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::UserMessage { text } if text == "meta"))
        );
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ReasoningDelta { turn_id, text } if turn_id == "a" && text == "plan")
        ));
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::AssistantMessage { turn_id, text } if turn_id == "a" && text == "orphan reply")
        ));
    }

    #[test]
    fn prompts_tools_and_file_changes_restore_in_order_for_this_session_only() {
        let records = [
            json!({"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s","message":{"content":[
                {"type":"text","text":"look"},{"type":"image","source":{}},{"type":"document"}]}}),
            json!({"type":"user","uuid":"x","parentUuid":"u1","sessionId":"other","message":{"content":"foreign"}}),
            json!({"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s","message":{"content":[
                {"type":"tool_use","id":"w","name":"Write","input":{"file_path":"/w/new.txt","content":"one\ntwo"}}]}}),
            json!({"type":"user","uuid":"r1","parentUuid":"a1","sessionId":"s",
                "tool_use_result":{"type":"create"},
                "message":{"content":[{"type":"tool_result","tool_use_id":"w","content":[{"type":"text","text":"written"}]}]}}),
            json!({"type":"assistant","uuid":"a2","parentUuid":"r1","sessionId":"s","message":{"content":[
                {"type":"tool_use","id":"b","name":"Bash","input":{"command":"false"}}]}}),
            json!({"type":"user","uuid":"r2","parentUuid":"a2","sessionId":"s",
                "message":{"content":[{"type":"tool_result","tool_use_id":"b","is_error":true,"content":"exit 1"}]}}),
            json!({"type":"assistant","uuid":"a3","parentUuid":"r2","sessionId":"s","message":{"content":[{"type":"text","text":"done"}]}}),
        ];
        let events = read(transcript(&records).as_bytes(), "s").unwrap();
        assert_balanced(&events);
        assert_eq!(
            events,
            vec![
                AgentEvent::UserMessage {
                    text: "look\n[Image]\n[Document]".into()
                },
                AgentEvent::TurnStarted {
                    turn_id: "u1".into()
                },
                AgentEvent::ToolCallStarted {
                    id: "w".into(),
                    name: "Write".into(),
                    input_summary: "/w/new.txt".into()
                },
                AgentEvent::ToolCallFinished {
                    id: "w".into(),
                    status: del_agent::ToolStatus::Completed,
                    output_summary: "written".into()
                },
                AgentEvent::FileChange {
                    path: "/w/new.txt".into(),
                    kind: del_agent::FileChangeKind::Created,
                    diff: Some("+one\n+two\n".into())
                },
                AgentEvent::ToolCallStarted {
                    id: "b".into(),
                    name: "Bash".into(),
                    input_summary: "false".into()
                },
                AgentEvent::ToolCallFinished {
                    id: "b".into(),
                    status: del_agent::ToolStatus::Failed,
                    output_summary: "exit 1".into()
                },
                AgentEvent::AssistantMessage {
                    turn_id: "u1".into(),
                    text: "done".into()
                },
                AgentEvent::TurnCompleted {
                    turn_id: "u1".into(),
                    status: TurnStatus::Completed
                },
            ]
        );
    }

    #[test]
    fn empty_or_missing_transcripts_restore_nothing() {
        assert!(read(&b""[..], "s").unwrap().is_empty());
        assert!(read(&b"\n\nnot json\n"[..], "s").unwrap().is_empty());
        let dir = tempfile::tempdir().unwrap();
        // Ids are filenames, never paths: a non-UUID id is not looked up.
        assert!(
            load_from(dir.path(), dir.path(), "../escape")
                .unwrap()
                .is_empty()
        );
        let missing = uuid::Uuid::new_v4().to_string();
        assert!(
            load_from(dir.path(), dir.path(), &missing)
                .unwrap()
                .is_empty()
        );
    }
}
