//! Reply timestamps survive view recreation; provider transcripts backfill older replies.
use super::model::{Block, ChatModel};
use chrono::{DateTime, Local};
use del_agent::{Provider, SessionRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, Write},
    path::PathBuf,
};

/// One journal line. Older lines carried the whole reply; new lines identify it by
/// ordinal plus a short prefix and a stable fingerprint of the text.
#[derive(Serialize, Deserialize)]
pub struct Saved {
    ordinal: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<u64>,
    milliseconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elapsed_milliseconds: Option<u64>,
}

impl Saved {
    pub fn with_elapsed(mut self, elapsed: Option<std::time::Duration>) -> Self {
        self.elapsed_milliseconds = elapsed.and_then(|d| d.as_millis().try_into().ok());
        self
    }

    fn matches(&self, text: &str) -> bool {
        match (&self.text, self.hash) {
            (Some(saved), _) => saved == text,
            (None, Some(hash)) => {
                hash == fingerprint(text)
                    && self
                        .prefix
                        .as_deref()
                        .is_none_or(|prefix| text.starts_with(prefix))
            }
            (None, None) => false,
        }
    }
}

/// FNV-1a: stable across builds, unlike `DefaultHasher`.
fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn path(id: &str) -> Option<PathBuf> {
    let name: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    Some(
        del_agent::data_dir()?
            .join("reply-times")
            .join(format!("{name}.jsonl")),
    )
}

/// The record for reply `ix`; cheap enough for the UI thread. Write it with [`append`].
pub fn entry(model: &ChatModel, ix: usize, time: DateTime<Local>) -> Option<Saved> {
    let Some(Block::Assistant { text, .. }) = model.blocks.get(ix) else {
        return None;
    };
    let ordinal = model.blocks[..ix]
        .iter()
        .filter(|b| matches!(b, Block::Assistant { .. }))
        .count();
    Some(Saved {
        ordinal,
        text: None,
        prefix: Some(text.chars().take(48).collect()),
        hash: Some(fingerprint(text)),
        milliseconds: time.timestamp_millis(),
        elapsed_milliseconds: None,
    })
}

/// File IO; run off the UI thread. One write per record keeps concurrent appends whole.
pub fn append(id: &str, entry: &Saved) -> std::io::Result<()> {
    let Some(path) = path(id) else { return Ok(()) };
    let mut line = serde_json::to_vec(entry)?;
    line.push(b'\n');
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(&line)
}

#[derive(Clone, Copy)]
struct NativeTime {
    at: DateTime<Local>,
    started: Option<DateTime<Local>>,
}

#[derive(Default)]
pub struct Restored {
    pub times: HashMap<usize, DateTime<Local>>,
    pub elapsed: HashMap<usize, std::time::Duration>,
    pub started: HashMap<usize, DateTime<Local>>,
}

#[derive(Default)]
pub struct History {
    saved: Vec<Saved>,
    native: HashMap<String, VecDeque<NativeTime>>,
}
impl History {
    #[cfg(test)]
    pub fn restore(self, model: &ChatModel) -> HashMap<usize, DateTime<Local>> {
        self.restore_details(model).times
    }

    pub fn restore_details(mut self, model: &ChatModel) -> Restored {
        let mut restored = Restored::default();
        let mut ordinal = 0;
        let mut start = 0;
        for (ix, block) in model.blocks.iter().enumerate() {
            if matches!(
                block,
                Block::User {
                    steer: false,
                    goal: false,
                    ..
                }
            ) {
                start = ix + 1;
            }
            let Block::Assistant { text, .. } = block else {
                continue;
            };
            let native = self.native.get_mut(text).and_then(VecDeque::pop_front);
            let saved = self
                .saved
                .iter()
                .rev()
                .find(|entry| entry.ordinal == ordinal && entry.matches(text));
            ordinal += 1;
            let time = saved
                .and_then(|entry| DateTime::from_timestamp_millis(entry.milliseconds))
                .map(|time| time.with_timezone(&Local))
                .or(native.map(|n| n.at));
            // Peer turns belong to the same user request. Keep its earliest
            // provider start, not just the start of the final short reply.
            if let Some(at) = native.and_then(|n| n.started) {
                restored
                    .started
                    .entry(start)
                    .and_modify(|t| *t = (*t).min(at))
                    .or_insert(at);
            }
            let elapsed = saved
                .and_then(|s| s.elapsed_milliseconds)
                .map(std::time::Duration::from_millis)
                .or_else(|| (time? - *restored.started.get(&start)?).to_std().ok());
            if let Some(elapsed) = elapsed {
                restored.elapsed.insert(ix, elapsed);
            }
            if let Some(time) = time {
                restored.times.insert(ix, time);
            }
        }
        restored
    }
}

pub fn load(record: &SessionRecord) -> History {
    let mut history = History::default();
    if let Some(path) = path(&record.id.0)
        && let Ok(file) = std::fs::File::open(path)
    {
        history.saved = std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect();
    }
    let account = crate::accounts::for_record(record);
    let root = account.as_ref().map(|a| a.root());
    let native = match record.provider {
        Provider::Codex => crate::terminal_conversation::rollouts(
            &root
                .clone()
                .unwrap_or_else(crate::terminal_conversation::codex_root),
        )
        .into_iter()
        .find(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .ends_with(&format!("{}.jsonl", record.id.0))
            })
        }),
        Provider::Claude => match root {
            Some(root) => del_claude::history::path_from(&root, &record.cwd, &record.id.0),
            None => del_claude::history::path(&record.cwd, &record.id.0),
        },
        _ => None,
    };
    if let Some(path) = native
        && let Ok(file) = std::fs::File::open(path)
    {
        history.native = read_native(std::io::BufReader::new(file));
    }
    history
}

fn read_native(reader: impl BufRead) -> HashMap<String, VecDeque<NativeTime>> {
    let mut times: HashMap<String, VecDeque<NativeTime>> = HashMap::new();
    let mut records = Vec::<serde_json::Value>::new();
    let mut positions = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if record["isSidechain"] == true {
            continue;
        }
        if let Some(uuid) = record["uuid"].as_str() {
            if let Some(&ix) = positions.get(uuid) {
                records[ix] = record;
                continue;
            }
            positions.insert(uuid.to_owned(), records.len());
        }
        records.push(record);
    }
    // Claude history follows the latest parent chain, excluding abandoned retries.
    let mut chain = std::collections::HashSet::new();
    let mut cursor = records
        .iter()
        .rposition(|r| matches!(r["type"].as_str(), Some("user" | "assistant")));
    while let Some(ix) = cursor {
        if !chain.insert(ix) {
            break;
        }
        cursor = records[ix]["parentUuid"]
            .as_str()
            .and_then(|id| positions.get(id))
            .copied();
    }
    let mut started = None;
    for (ix, record) in records.iter().enumerate() {
        if matches!(record["type"].as_str(), Some("user" | "assistant")) && !chain.contains(&ix) {
            continue;
        }
        let Some(time) = record["timestamp"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        else {
            continue;
        };
        let time = time.with_timezone(&Local);
        if record["type"] == "event_msg" && record["payload"]["type"] == "task_started" {
            started = Some(time);
        }
        if record["type"] == "event_msg"
            && record["payload"]["type"] == "task_complete"
            && let Some(text) = record["payload"]["last_agent_message"].as_str()
            && let Some(last) = times.get_mut(text).and_then(|entries| entries.back_mut())
        {
            last.at = time;
        }
        // Claude's user text marks work start; tool-result records do not.
        if record["type"] == "user" {
            let content = &record["message"]["content"];
            if content.is_string()
                || content
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "text"))
            {
                started = Some(time);
            }
        }
        let message =
            if record["type"] == "response_item" && record["payload"]["role"] == "assistant" {
                &record["payload"]
            } else if record["type"] == "assistant" && record["isSidechain"] != true {
                &record["message"]
            } else {
                continue;
            };
        let text = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| matches!(part["type"].as_str(), Some("text" | "output_text")))
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            times
                .entry(text)
                .or_default()
                .push_back(NativeTime { at: time, started });
        }
    }
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_timing_spans_peer_turns_and_restores_saved_duration() {
        let rows = [
            serde_json::json!({"timestamp":"2026-10-30T10:00:00Z","type":"event_msg","payload":{"type":"task_started"}}),
            serde_json::json!({"timestamp":"2026-10-30T10:00:10Z","type":"response_item","payload":{"role":"assistant","content":[{"type":"output_text","text":"Waiting"}]}}),
            serde_json::json!({"timestamp":"2026-10-30T10:01:00Z","type":"event_msg","payload":{"type":"task_started"}}),
            serde_json::json!({"timestamp":"2026-10-30T10:01:05Z","type":"response_item","payload":{"role":"assistant","content":[{"type":"output_text","text":"Done"}]}}),
            serde_json::json!({"timestamp":"2026-10-30T10:01:09Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":"Done"}}),
        ].into_iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n");
        let mut model = ChatModel::default();
        model.push_user("delegate".into(), false);
        for text in ["Waiting", "Done"] {
            model.apply(&del_agent::AgentEvent::AssistantMessage {
                turn_id: text.into(),
                text: text.into(),
            });
            model.apply(&del_agent::AgentEvent::TurnCompleted {
                turn_id: text.into(),
                status: del_agent::TurnStatus::Completed,
            });
        }
        let restored = History {
            native: read_native(rows.as_bytes()),
            ..Default::default()
        }
        .restore_details(&model);
        assert_eq!(restored.elapsed[&2].as_secs(), 69);
        let saved = entry(&model, 2, restored.times[&2])
            .unwrap()
            .with_elapsed(Some(std::time::Duration::from_secs(72)));
        let saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let restored = History {
            saved: vec![saved],
            ..Default::default()
        }
        .restore_details(&model);
        assert_eq!(restored.elapsed[&2].as_secs(), 72);
    }

    #[test]
    fn claude_work_timing_ignores_tool_results_and_previous_requests() {
        let rows = [
            serde_json::json!({"type":"user","uuid":"u","timestamp":"2026-10-30T10:00:00Z","message":{"content":"First"}}),
            serde_json::json!({"type":"assistant","uuid":"a","parentUuid":"u","timestamp":"2026-10-30T10:00:03Z","message":{"content":[{"type":"text","text":"First answer"}]}}),
            serde_json::json!({"type":"user","uuid":"u2","parentUuid":"a","timestamp":"2026-10-30T11:00:00Z","message":{"content":[{"type":"text","text":"Second"}]}}),
            serde_json::json!({"type":"user","uuid":"t","parentUuid":"u2","timestamp":"2026-10-30T11:00:08Z","message":{"content":[{"type":"tool_result","content":"output"}]}}),
            serde_json::json!({"type":"assistant","uuid":"b","parentUuid":"t","timestamp":"2026-10-30T11:00:12Z","message":{"content":[{"type":"text","text":"Second answer"}]}}),
        ].into_iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n");
        let mut model = ChatModel::default();
        for (prompt, text) in [("First", "First answer"), ("Second", "Second answer")] {
            model.push_user(prompt.into(), false);
            model.apply(&del_agent::AgentEvent::AssistantMessage {
                turn_id: prompt.into(),
                text: text.into(),
            });
            model.apply(&del_agent::AgentEvent::TurnCompleted {
                turn_id: prompt.into(),
                status: del_agent::TurnStatus::Completed,
            });
        }
        let restored = History {
            native: read_native(rows.as_bytes()),
            ..Default::default()
        }
        .restore_details(&model);
        assert_eq!(restored.elapsed[&1].as_secs(), 3);
        assert_eq!(restored.elapsed[&3].as_secs(), 12);
    }
    #[test]
    fn claude_timestamps_follow_the_active_branch() {
        let rows = [
            serde_json::json!({"type":"user","uuid":"u","message":{"content":"Hi"}}),
            serde_json::json!({"type":"assistant","uuid":"old","parentUuid":"u","timestamp":"2026-10-26T09:00:00Z","message":{"content":[{"type":"text","text":"Hi"}]}}),
            serde_json::json!({"type":"assistant","uuid":"new","parentUuid":"u","timestamp":"2026-10-26T10:00:00Z","message":{"content":[{"type":"text","text":"Hi"}]}}),
        ].into_iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n");
        let times = read_native(rows.as_bytes());
        assert_eq!(times["Hi"].len(), 1);
        assert_eq!(
            times["Hi"][0]
                .at
                .with_timezone(&chrono::Utc)
                .format("%H:%M")
                .to_string(),
            "10:00"
        );
    }

    #[test]
    fn repeated_replies_keep_their_individual_provider_times() {
        let input = concat!(
            "{\"timestamp\":\"2026-10-26T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi\"}]}}\n",
            "{\"timestamp\":\"2026-10-26T11:00:00Z\",\"type\":\"response_item\",\"payload\":{\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi\"}]}}\n"
        );
        let mut model = ChatModel::default();
        for turn in ["one", "two"] {
            model.apply(&del_agent::AgentEvent::AssistantMessage {
                turn_id: turn.into(),
                text: "Hi".into(),
            });
            model.apply(&del_agent::AgentEvent::TurnCompleted {
                turn_id: turn.into(),
                status: del_agent::TurnStatus::Completed,
            });
        }
        let times = History {
            native: read_native(input.as_bytes()),
            ..Default::default()
        }
        .restore(&model);
        assert_eq!(times.len(), 2);
        assert_eq!((times[&1] - times[&0]).num_seconds(), 3600);
        let saved = entry(&model, 1, times[&1]).unwrap();
        let encoded = serde_json::to_string(&saved).unwrap();
        assert!(
            !encoded.contains("\"text\""),
            "journal lines no longer carry the reply: {encoded}"
        );
        let restored = History {
            saved: vec![serde_json::from_str(&encoded).unwrap()],
            ..Default::default()
        }
        .restore(&model);
        assert_eq!(restored.get(&1), times.get(&1));
        assert!(!restored.contains_key(&0));
        // Older journals stored the whole reply and remain readable.
        let legacy: Saved = serde_json::from_str(&format!(
            "{{\"ordinal\":0,\"text\":\"Hi\",\"milliseconds\":{}}}",
            times[&0].timestamp_millis()
        ))
        .unwrap();
        let restored = History {
            saved: vec![legacy],
            ..Default::default()
        }
        .restore(&model);
        assert_eq!(restored.get(&0), times.get(&0));
        // A fingerprint for different text never matches.
        let other: Saved = serde_json::from_str(
            &serde_json::to_string(&entry(&model, 0, times[&0]).unwrap())
                .unwrap()
                .replace("\"ordinal\":0", "\"ordinal\":1"),
        )
        .unwrap();
        let mut changed = ChatModel::default();
        changed.apply(&del_agent::AgentEvent::AssistantMessage {
            turn_id: "a".into(),
            text: "Hi".into(),
        });
        changed.apply(&del_agent::AgentEvent::AssistantMessage {
            turn_id: "b".into(),
            text: "Hi there".into(),
        });
        assert!(
            History {
                saved: vec![other],
                ..Default::default()
            }
            .restore(&changed)
            .is_empty()
        );
    }
}
