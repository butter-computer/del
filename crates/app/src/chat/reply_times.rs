//! Reply timestamps survive view recreation; provider transcripts backfill older replies.
use super::model::{Block, ChatModel};
use chrono::{DateTime, Local};
use del_agent::{Provider, SessionRecord};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, VecDeque}, io::{BufRead, Write}, path::PathBuf};

#[derive(Serialize, Deserialize)]
pub struct Saved {
    ordinal: usize,
    text: String,
    milliseconds: i64,
}

fn path(id: &str) -> Option<PathBuf> {
    let name: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
    Some(del_agent::data_dir()?.join("reply-times").join(format!("{name}.jsonl")))
}

pub fn save(id: &str, model: &ChatModel, ix: usize, time: DateTime<Local>) -> std::io::Result<()> {
    let Some(path) = path(id) else { return Ok(()) };
    let Some(Block::Assistant { text, .. }) = model.blocks.get(ix) else { return Ok(()) };
    let ordinal = model.blocks[..ix].iter().filter(|b| matches!(b, Block::Assistant { .. })).count();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, &Saved { ordinal, text: text.clone(), milliseconds: time.timestamp_millis() })?;
    file.write_all(b"\n")
}

#[derive(Default)]
pub struct History {
    saved: Vec<Saved>,
    native: HashMap<String, VecDeque<DateTime<Local>>>,
}
impl History {
    pub fn restore(mut self, model: &ChatModel) -> HashMap<usize, DateTime<Local>> {
        let mut times = HashMap::new();
        for (ordinal, (ix, text)) in model.blocks.iter().enumerate().filter_map(|(ix, block)| {
            if let Block::Assistant { text, .. } = block { Some((ix, text)) } else { None }
        }).enumerate() {
            let native = self.native.get_mut(text).and_then(VecDeque::pop_front);
            let saved = self.saved.iter().rev().find(|entry| entry.ordinal == ordinal && &entry.text == text)
                .and_then(|entry| DateTime::from_timestamp_millis(entry.milliseconds)).map(|time| time.with_timezone(&Local));
            if let Some(time) = saved.or(native) { times.insert(ix, time); }
        }
        times
    }
}

pub fn load(record: &SessionRecord) -> History {
    let mut history = History::default();
    if let Some(path) = path(&record.id.0) && let Ok(file) = std::fs::File::open(path) {
        history.saved = std::io::BufReader::new(file).lines().map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok()).collect();
    }
    let account = crate::accounts::for_record(record);
    let root = account.as_ref().map(|a| a.root());
    let native = match record.provider {
        Provider::Codex => crate::terminal_conversation::rollouts(&root.clone().unwrap_or_else(crate::terminal_conversation::codex_root)).into_iter()
            .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(&format!("{}.jsonl", record.id.0)))),
        Provider::Claude => match root {
            Some(root) => del_claude::history::path_from(&root, &record.cwd, &record.id.0),
            None => del_claude::history::path(&record.cwd, &record.id.0),
        },
        _ => None,
    };
    if let Some(path) = native && let Ok(file) = std::fs::File::open(path) {
        history.native = read_native(std::io::BufReader::new(file));
    }
    history
}

fn read_native(reader: impl BufRead) -> HashMap<String, VecDeque<DateTime<Local>>> {
    let mut times: HashMap<String, VecDeque<DateTime<Local>>> = HashMap::new();
    let mut records = Vec::<serde_json::Value>::new();
    let mut positions = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if record["isSidechain"] == true { continue; }
        if let Some(uuid) = record["uuid"].as_str() {
            if let Some(&ix) = positions.get(uuid) { records[ix] = record; continue; }
            positions.insert(uuid.to_owned(), records.len());
        }
        records.push(record);
    }
    // Claude history follows the latest parent chain, excluding abandoned retries.
    let mut chain = std::collections::HashSet::new();
    let mut cursor = records.iter().rposition(|r| matches!(r["type"].as_str(), Some("user" | "assistant")));
    while let Some(ix) = cursor {
        if !chain.insert(ix) { break; }
        cursor = records[ix]["parentUuid"].as_str().and_then(|id| positions.get(id)).copied();
    }
    for (ix, record) in records.iter().enumerate() {
        if record["type"] == "assistant" && !chain.contains(&ix) { continue; }
        let Some(time) = record["timestamp"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()) else { continue };
        let message = if record["type"] == "response_item" && record["payload"]["role"] == "assistant" {
            &record["payload"]
        } else if record["type"] == "assistant" && record["isSidechain"] != true {
            &record["message"]
        } else { continue };
        let text = message["content"].as_array().into_iter().flatten()
            .filter(|part| matches!(part["type"].as_str(), Some("text" | "output_text")))
            .filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n");
        if !text.is_empty() { times.entry(text).or_default().push_back(time.with_timezone(&Local)); }
    }
    times
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_timestamps_follow_the_active_branch() {
        let rows = [
            serde_json::json!({"type":"user","uuid":"u","message":{"content":"Hi"}}),
            serde_json::json!({"type":"assistant","uuid":"old","parentUuid":"u","timestamp":"2026-10-26T09:00:00Z","message":{"content":[{"type":"text","text":"Hi"}]}}),
            serde_json::json!({"type":"assistant","uuid":"new","parentUuid":"u","timestamp":"2026-10-26T10:00:00Z","message":{"content":[{"type":"text","text":"Hi"}]}}),
        ].into_iter().map(|r| r.to_string()).collect::<Vec<_>>().join("\n");
        let times = read_native(rows.as_bytes());
        assert_eq!(times["Hi"].len(), 1);
        assert_eq!(times["Hi"][0].with_timezone(&chrono::Utc).format("%H:%M").to_string(), "10:00");
    }

    #[test]
    fn repeated_replies_keep_their_individual_provider_times() {
        let input = concat!(
            "{\"timestamp\":\"2026-10-26T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi\"}]}}\n",
            "{\"timestamp\":\"2026-10-26T11:00:00Z\",\"type\":\"response_item\",\"payload\":{\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hi\"}]}}\n"
        );
        let mut model = ChatModel::default();
        for turn in ["one", "two"] {
            model.apply(&del_agent::AgentEvent::AssistantMessage { turn_id: turn.into(), text: "Hi".into() });
            model.apply(&del_agent::AgentEvent::TurnCompleted { turn_id: turn.into(), status: del_agent::TurnStatus::Completed });
        }
        let times = History { native: read_native(input.as_bytes()), ..Default::default() }.restore(&model);
        assert_eq!(times.len(), 2);
        assert_eq!((times[&1] - times[&0]).num_seconds(), 3600);
        let saved = Saved { ordinal: 1, text: "Hi".into(), milliseconds: times[&1].timestamp_millis() };
        let encoded = serde_json::to_string(&saved).unwrap();
        let restored = History { saved: vec![serde_json::from_str(&encoded).unwrap()], ..Default::default() }.restore(&model);
        assert_eq!(restored.get(&1), times.get(&1));
        assert!(!restored.contains_key(&0));
    }
}
