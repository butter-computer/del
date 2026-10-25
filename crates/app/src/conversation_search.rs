//! Text-only search over saved provider conversations. No provider process is
//! started here; unchanged transcripts stay cached between palette openings.
use crate::chat::model::{Block, ChatModel};
use crate::state::Tab;
#[cfg(test)]
use del_agent::AgentEvent;
use del_agent::Provider;
use serde_json::Value;
use std::{collections::HashMap, io::BufRead, ops::Range, path::PathBuf, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: &'static str,
    pub text: String,
}

pub fn messages(model: &ChatModel) -> Vec<Message> {
    model
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::User { text, .. } => Some(Message {
                role: "You",
                text: text.clone(),
            }),
            Block::Assistant { text, .. } => Some(Message {
                role: "Assistant",
                text: text.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[derive(Clone)]
pub struct Source {
    pub tab: Tab,
    pub title: String,
    pub provider: Option<Provider>,
    pub cwd: PathBuf,
    pub conversation_id: Option<String>,
    pub archived: bool,
    pub live: Option<Vec<Message>>,
}

#[derive(Clone)]
pub struct Document {
    pub source: Source,
    pub messages: Arc<Vec<Message>>,
}

#[derive(Clone)]
pub struct Hit {
    pub document: Arc<Document>,
    pub message: Option<usize>,
    pub range: Range<usize>,
    pub snippet: String,
    pub snippet_range: Range<usize>,
}

#[derive(Default)]
pub struct Index {
    files: HashMap<PathBuf, CachedFile>,
    opencode: del_opencode::history::Cache,
}
struct CachedFile {
    modified: Option<std::time::SystemTime>,
    size: u64,
    messages: Arc<Vec<Message>>,
}

pub struct Snapshot {
    pub documents: Vec<Arc<Document>>,
    pub errors: Vec<String>,
}

impl Index {
    pub fn refresh(&mut self, sources: Vec<Source>) -> Snapshot {
        let rollouts =
            crate::terminal_conversation::rollouts(&crate::terminal_conversation::codex_root());
        let mut used = std::collections::HashSet::new();
        let mut snapshot = Snapshot {
            documents: vec![],
            errors: vec![],
        };
        for source in sources {
            let id = source.conversation_id.as_deref();
            let path = match (source.provider, id) {
                (Some(Provider::Claude), Some(id)) => del_claude::history::path(&source.cwd, id),
                (Some(Provider::Codex), Some(id)) => rollouts
                    .iter()
                    .find(|p| {
                        p.file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(|name| name.ends_with(&format!("{id}.jsonl")))
                    })
                    .cloned(),
                _ => None,
            };
            let mut saved = Arc::new(vec![]);
            if let Some(path) = path {
                used.insert(path.clone());
                match self.read(&path, source.provider, id.unwrap_or_default()) {
                    Ok(messages) => saved = messages,
                    Err(error) => snapshot.errors.push(format!("{}: {error}", source.title)),
                }
            }
            if source.provider == Some(Provider::OpenCode)
                && let Some(id) = id
            {
                match self.opencode.read(&source.cwd, id) {
                    Ok(events) => {
                        let mut model = ChatModel::default();
                        for event in events.iter() {
                            model.apply(event);
                        }
                        saved = Arc::new(messages(&model));
                    }
                    Err(error) => snapshot.errors.push(format!("{}: {error}", source.title)),
                }
            }
            // Live views include the currently streaming text, which may not
            // have reached the provider's JSONL file yet.
            if let Some(live) = &source.live
                && !live.is_empty()
            {
                saved = Arc::new(live.clone());
            }
            snapshot.documents.push(Arc::new(Document {
                source,
                messages: saved,
            }));
        }
        self.files.retain(|path, _| used.contains(path));
        snapshot
    }

    fn read(
        &mut self,
        path: &PathBuf,
        provider: Option<Provider>,
        id: &str,
    ) -> std::io::Result<Arc<Vec<Message>>> {
        let meta = std::fs::metadata(path)?;
        let modified = meta.modified().ok();
        if let Some(cached) = self.files.get(path)
            && cached.size == meta.len()
            && cached.modified == modified
        {
            return Ok(cached.messages.clone());
        }
        let reader = std::io::BufReader::new(std::fs::File::open(path)?);
        let messages = if provider == Some(Provider::Claude) {
            let mut model = ChatModel::default();
            for event in del_claude::history::read(reader, id)? {
                model.apply(&event);
            }
            messages(&model)
        } else {
            read_codex(reader)?
        };
        let messages = Arc::new(messages);
        self.files.insert(
            path.clone(),
            CachedFile {
                modified,
                size: meta.len(),
                messages: messages.clone(),
            },
        );
        Ok(messages)
    }
}

/// Raw response messages are the persisted source of truth. Legacy event_msg
/// copies are used only when that older rollout contains no response messages.
pub fn read_codex(reader: impl BufRead) -> std::io::Result<Vec<Message>> {
    let mut messages = vec![];
    let mut legacy = vec![];
    for line in reader.lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &record["payload"];
        if record["type"] == "response_item" && payload["type"] == "message" {
            let role = match payload["role"].as_str() {
                Some("user") => "You",
                Some("assistant") => "Assistant",
                _ => continue,
            };
            let text = payload["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| match part["type"].as_str() {
                    Some("input_text" | "output_text" | "text") => part["text"].as_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            // Runtime context records are not user-authored conversation messages.
            if role == "You"
                && (text.starts_with("<environment_context>")
                    || text.starts_with("# AGENTS.md instructions for ")
                    || text.starts_with("<permissions instructions>"))
            {
                continue;
            }
            if !text.trim().is_empty() {
                let (text, _) = crate::chat::attachments::AttachmentSummary::from_history(&text);
                messages.push(Message { role, text });
            }
        } else if record["type"] == "event_msg" {
            let role = match payload["type"].as_str() {
                Some("user_message") => "You",
                Some("agent_message") => "Assistant",
                _ => continue,
            };
            if let Some(text) = payload["message"].as_str() {
                legacy.push(Message {
                    role,
                    text: text.into(),
                });
            }
        }
    }
    Ok(if messages.is_empty() {
        legacy
    } else {
        messages
    })
}

/// Case-insensitive matching with byte ranges mapped back to the original UTF-8.
/// Lowercasing can expand a character (İ), so offsets into lowercase text cannot
/// be used directly to slice or highlight the original message.
pub fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return vec![];
    }
    let mut folded = String::new();
    let mut offsets = vec![];
    for (start, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let before = folded.len();
            folded.push(lower);
            offsets.extend((before..folded.len()).map(|_| (start, start + ch.len_utf8())));
        }
    }
    folded
        .match_indices(&query)
        .map(|(start, value)| offsets[start].0..offsets[start + value.len() - 1].1)
        .collect()
}

fn snippet(text: &str, range: &Range<usize>) -> (String, Range<usize>) {
    let start = text[..range.start]
        .char_indices()
        .rev()
        .nth(55)
        .map_or(0, |(i, _)| i);
    let end = text[range.end..]
        .char_indices()
        .nth(110)
        .map_or(text.len(), |(i, _)| range.end + i);
    let prefix = if start > 0 { "…" } else { "" };
    let snippet = format!(
        "{prefix}{}{}",
        text[start..end].replace(['\n', '\r', '\t'], " "),
        if end < text.len() { "…" } else { "" }
    );
    let offset = prefix.len() + range.start - start;
    (snippet, offset..offset + range.len())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Chat,
    Terminal,
}

fn query_parts(query: &str) -> (Option<Scope>, String) {
    let mut scope = None;
    let mut words = Vec::new();
    for word in query.split_whitespace() {
        if word.eq_ignore_ascii_case("/c") {
            scope = Some(Scope::Chat);
        } else if word.eq_ignore_ascii_case("/t") {
            scope = Some(Scope::Terminal);
        } else {
            words.push(word);
        }
    }
    (scope, words.join(" "))
}

pub fn search(documents: &[Arc<Document>], query: &str) -> Vec<Hit> {
    let mut hits = vec![];
    let (scope, query) = query_parts(query);
    if query.is_empty() && scope.is_none() {
        return hits;
    }
    for document in documents {
        let kind = if matches!(document.source.tab, Tab::Terminal(_)) {
            Scope::Terminal
        } else {
            Scope::Chat
        };
        if scope.is_some_and(|scope| scope != kind) {
            continue;
        }
        let title_match = if query.is_empty() {
            Some(0..0)
        } else {
            ranges(&document.source.title, &query).into_iter().next()
        };
        if let Some(range) = title_match {
            let (snippet, snippet_range) = snippet(&document.source.title, &range);
            hits.push(Hit {
                document: document.clone(),
                message: None,
                range,
                snippet,
                snippet_range,
            });
        }
        for (index, message) in document.messages.iter().enumerate() {
            for range in ranges(&message.text, &query) {
                let (snippet, snippet_range) = snippet(&message.text, &range);
                hits.push(Hit {
                    document: document.clone(),
                    message: Some(index),
                    range,
                    snippet,
                    snippet_range,
                });
            }
        }
    }
    hits
}

#[cfg(test)]
pub fn model(messages: &[Message]) -> ChatModel {
    let mut model = ChatModel::default();
    for (i, message) in messages.iter().enumerate() {
        let event = if message.role == "You" {
            AgentEvent::UserMessage {
                text: message.text.clone(),
            }
        } else {
            AgentEvent::AssistantMessage {
                turn_id: format!("search-{i}"),
                text: message.text.clone(),
            }
        };
        model.apply(&event);
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use del_agent::SessionId;
    use serde_json::json;

    fn document(terminal: bool, archived: bool, text: &str) -> Arc<Document> {
        Arc::new(Document {
            source: Source {
                tab: if terminal {
                    Tab::Terminal(SessionId::new("t"))
                } else {
                    Tab::Session(SessionId::new("c"))
                },
                title: "Saved task".into(),
                provider: Some(Provider::Codex),
                cwd: "/tmp".into(),
                conversation_id: None,
                archived,
                live: None,
            },
            messages: Arc::new(vec![Message {
                role: "You",
                text: text.into(),
            }]),
        })
    }
    #[test]
    fn searches_every_occurrence_across_archived_and_terminal_conversations() {
        let docs = [
            document(false, true, "needle twice NEEDLE"),
            document(true, false, "old needle"),
        ];
        let hits = search(&docs, "needle");
        assert_eq!(hits.len(), 3);
        assert!(hits[0].document.source.archived);
        assert!(matches!(hits[2].document.source.tab, Tab::Terminal(_)));
        assert_eq!(
            &hits[1].document.messages[0].text[hits[1].range.clone()],
            "NEEDLE"
        );
        assert_eq!(search(&docs, "saved").len(), 2);
        assert!(search(&docs, "   ").is_empty());
    }
    #[test]
    fn slash_filters_scope_results_and_keep_match_offsets() {
        let docs = [
            document(false, true, "hello café /t/path"),
            document(true, false, "hello CAFÉ"),
        ];
        for (query, terminal) in [("/t café", true), ("café /C", false)] {
            let hits = search(&docs, query);
            assert_eq!(hits.len(), 1);
            assert_eq!(
                matches!(hits[0].document.source.tab, Tab::Terminal(_)),
                terminal
            );
            let text = &hits[0].document.messages[0].text;
            assert_eq!(text[hits[0].range.clone()].to_lowercase(), "café");
        }
        let hits = search(&docs, "/t");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.is_none());
        assert!(matches!(hits[0].document.source.tab, Tab::Terminal(_)));
        assert_eq!(search(&docs, "/c saved").len(), 1);
        assert_eq!(search(&docs, "/t/path").len(), 1);
        assert_eq!(search(&docs, "hello").len(), 2);
    }

    #[test]
    fn unicode_snippets_preserve_original_byte_ranges() {
        let text = format!(
            "{}İstanbul 😀 CAFÉ Needle{}",
            "界".repeat(100),
            "尾".repeat(200)
        );
        for query in ["i", "😀", "café", "NEEDLE"] {
            let range = ranges(&text, query).remove(0);
            let (snippet, highlight) = snippet(&text, &range);
            assert_eq!(&snippet[highlight], &text[range]);
        }
        assert_eq!(ranges("abc abc", "abc"), vec![0..3, 4..7]);
    }
    #[test]
    fn codex_raw_messages_exclude_context_and_duplicate_legacy_events() {
        let records = [
            json!({"type":"response_item","payload":{"type":"message","role":"system","content":[{"type":"input_text","text":"secret instructions"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"},{"type":"input_image","image_url":"large-image-data"}]}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"hello"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"saved answer"}]}}),
        ];
        let input = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n{partial";
        let messages = read_codex(input.as_bytes()).unwrap();
        assert_eq!(
            messages,
            vec![
                Message {
                    role: "You",
                    text: "hello".into()
                },
                Message {
                    role: "Assistant",
                    text: "saved answer".into()
                }
            ]
        );
        let old = json!({"type":"event_msg","payload":{"type":"agent_message","message":"legacy answer"}}).to_string();
        assert_eq!(read_codex(old.as_bytes()).unwrap()[0].text, "legacy answer");
    }
    #[test]
    fn cache_reuses_unchanged_files_and_refreshes_appends_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let record = |text: &str| {
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}).to_string() + "\n"
        };
        std::fs::write(&path, record("first")).unwrap();
        let mut index = Index::default();
        let first = index.read(&path, Some(Provider::Codex), "id").unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &index.read(&path, Some(Provider::Codex), "id").unwrap()
        ));
        std::fs::write(&path, record("first") + &record("later")).unwrap();
        assert_eq!(
            index
                .read(&path, Some(Provider::Codex), "id")
                .unwrap()
                .len(),
            2
        );
        std::fs::write(&path, record("replaced")).unwrap();
        assert_eq!(
            index.read(&path, Some(Provider::Codex), "id").unwrap()[0].text,
            "replaced"
        );
        std::fs::remove_file(path).unwrap();
        assert!(index.refresh(vec![]).documents.is_empty());
        assert!(index.files.is_empty());
    }
    #[test]
    fn live_messages_are_searchable_without_a_provider_file_or_process() {
        let mut source = document(false, false, "").source.clone();
        source.live = Some(vec![Message {
            role: "Assistant",
            text: "streaming now".into(),
        }]);
        let snapshot = Index::default().refresh(vec![source]);
        assert_eq!(search(&snapshot.documents, "streaming").len(), 1);
        assert!(snapshot.errors.is_empty());
    }
}
