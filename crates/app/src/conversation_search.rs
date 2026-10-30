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
    pub account: Option<crate::accounts::Account>,
    pub tab: Tab,
    pub title: String,
    pub project_name: String,
    pub provider: Option<Provider>,
    pub cwd: PathBuf,
    pub conversation_id: Option<String>,
    pub archived: bool,
    /// Shared with the open chat view, which rebuilds it only after changes.
    pub live: Option<Arc<Vec<Message>>>,
}

#[derive(Clone)]
pub struct Document {
    pub source: Source,
    pub messages: Arc<Vec<Message>>,
    /// Case-folded message text, computed once per document rather than per query.
    pub folded: FoldedMessages,
}

/// Shared between a cached file and every `Document` built from it.
#[derive(Clone, Default)]
pub struct FoldedMessages(Arc<std::sync::OnceLock<Vec<Folded>>>);

impl FoldedMessages {
    fn get(&self, messages: &[Message]) -> &[Folded] {
        self.0
            .get_or_init(|| messages.iter().map(|message| fold(&message.text)).collect())
    }
}

/// Lowercased text with each folded byte mapped back to its original character.
pub struct Folded {
    text: String,
    offsets: Vec<(usize, usize)>,
}

fn fold(text: &str) -> Folded {
    let mut folded = String::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len());
    for (start, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let before = folded.len();
            folded.push(lower);
            offsets.extend((before..folded.len()).map(|_| (start, start + ch.len_utf8())));
        }
    }
    Folded {
        text: folded,
        offsets,
    }
}

impl Folded {
    /// Byte ranges in the original text for an already lowercased, non-empty needle.
    fn find(&self, needle: &str) -> Vec<Range<usize>> {
        self.text
            .match_indices(needle)
            .map(|(start, value)| self.offsets[start].0..self.offsets[start + value.len() - 1].1)
            .collect()
    }
}

#[derive(Clone)]
pub struct Hit {
    pub document: Arc<Document>,
    pub message: Option<usize>,
    pub range: Range<usize>,
    pub snippet: String,
    pub snippet_range: Range<usize>,
}

/// One Codex sessions folder, indexed by the id that ends each rollout name.
struct Rollouts {
    paths: Vec<PathBuf>,
    by_id: HashMap<String, usize>,
}
impl Rollouts {
    fn new(paths: Vec<PathBuf>) -> Self {
        let by_id = paths
            .iter()
            .enumerate()
            .filter(|(_, path)| path.extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|(ix, path)| {
                let stem = path.file_stem()?.to_str()?;
                Some((stem.get(stem.len().checked_sub(36)?..)?.to_owned(), ix))
            })
            .collect();
        Self { paths, by_id }
    }
    fn find(&self, id: &str) -> Option<PathBuf> {
        if let Some(&ix) = self.by_id.get(id) {
            return Some(self.paths[ix].clone());
        }
        // Ids that are not 36-character UUIDs still match by name suffix.
        let suffix = format!("{id}.jsonl");
        self.paths
            .iter()
            .find(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|name| name.ends_with(&suffix))
            })
            .cloned()
    }
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
    folded: FoldedMessages,
}

pub struct Snapshot {
    pub documents: Vec<Arc<Document>>,
    pub errors: Vec<String>,
}

impl Index {
    pub fn refresh(&mut self, sources: Vec<Source>) -> Snapshot {
        // Each Codex sessions folder is walked at most once per refresh, and
        // only when a Codex conversation needs it.
        let mut rollouts: HashMap<PathBuf, Rollouts> = HashMap::new();
        let mut used = std::collections::HashSet::new();
        let mut snapshot = Snapshot {
            documents: vec![],
            errors: vec![],
        };
        for source in sources {
            let id = source.conversation_id.as_deref();
            let path = match (source.provider, id) {
                (Some(Provider::Claude), Some(id)) => match &source.account {
                    Some(a) => del_claude::history::path_from(&a.root(), &source.cwd, id),
                    None => del_claude::history::path(&source.cwd, id),
                },
                (Some(Provider::Codex), Some(id)) => {
                    let root = source
                        .account
                        .as_ref()
                        .map_or_else(crate::terminal_conversation::codex_root, |a| a.root());
                    rollouts
                        .entry(root)
                        .or_insert_with_key(|root| {
                            Rollouts::new(crate::terminal_conversation::rollouts(root))
                        })
                        .find(id)
                }
                _ => None,
            };
            let mut saved = Arc::new(vec![]);
            let mut folded = FoldedMessages::default();
            if let Some(path) = path {
                used.insert(path.clone());
                match self.read(&path, source.provider, id.unwrap_or_default()) {
                    Ok((messages, cached)) => {
                        saved = messages;
                        folded = cached;
                    }
                    Err(error) => snapshot.errors.push(format!("{}: {error}", source.title)),
                }
            }
            if source.provider == Some(Provider::OpenCode)
                && let Some(id) = id
            {
                match self.opencode.read_with_environment(
                    &source.cwd,
                    id,
                    &source
                        .account
                        .as_ref()
                        .map(|a| a.environment())
                        .unwrap_or_default(),
                ) {
                    Ok(events) => {
                        let mut model = ChatModel::default();
                        for event in events.iter() {
                            model.apply(event);
                        }
                        saved = Arc::new(messages(&model));
                        folded = FoldedMessages::default();
                    }
                    Err(error) => snapshot.errors.push(format!("{}: {error}", source.title)),
                }
            }
            // Live views include the currently streaming text, which may not
            // have reached the provider's JSONL file yet.
            if let Some(live) = &source.live
                && !live.is_empty()
            {
                saved = live.clone();
                folded = FoldedMessages::default();
            }
            snapshot.documents.push(Arc::new(Document {
                source,
                messages: saved,
                folded,
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
    ) -> std::io::Result<(Arc<Vec<Message>>, FoldedMessages)> {
        let meta = std::fs::metadata(path)?;
        let modified = meta.modified().ok();
        if let Some(cached) = self.files.get(path)
            && cached.size == meta.len()
            && cached.modified == modified
        {
            return Ok((cached.messages.clone(), cached.folded.clone()));
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
        let folded = FoldedMessages::default();
        self.files.insert(
            path.clone(),
            CachedFile {
                modified,
                size: meta.len(),
                messages: messages.clone(),
                folded: folded.clone(),
            },
        );
        Ok((messages, folded))
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
    fold(text).find(&query)
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

#[cfg(test)]
pub fn search(documents: &[Arc<Document>], query: &str) -> Vec<Hit> {
    search_until(documents, query, || false).unwrap_or_default()
}

/// Like [`search`], but gives up (returning `None`) once `superseded` reports
/// that a newer query replaced this one.
pub fn search_until(
    documents: &[Arc<Document>],
    query: &str,
    superseded: impl Fn() -> bool,
) -> Option<Vec<Hit>> {
    let mut hits = vec![];
    let (scope, query) = query_parts(query);
    if query.is_empty() && scope.is_none() {
        return Some(hits);
    }
    let needle = query.to_lowercase();
    for document in documents {
        if superseded() {
            return None;
        }
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
        if needle.is_empty() {
            continue;
        }
        let folded = document.folded.get(&document.messages);
        for (index, (message, folded)) in document.messages.iter().zip(folded).enumerate() {
            for range in folded.find(&needle) {
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
    Some(hits)
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
                account: None,
                tab: if terminal {
                    Tab::Terminal(SessionId::new("t"))
                } else {
                    Tab::Session(SessionId::new("c"))
                },
                title: "Saved task".into(),
                provider: Some(Provider::Codex),
                project_name: "tmp".into(),
                cwd: "/tmp".into(),
                conversation_id: None,
                archived,
                live: None,
            },
            messages: Arc::new(vec![Message {
                role: "You",
                text: text.into(),
            }]),
            folded: Default::default(),
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
        let (first, folded) = index.read(&path, Some(Provider::Codex), "id").unwrap();
        let (again, folded_again) = index.read(&path, Some(Provider::Codex), "id").unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert!(
            Arc::ptr_eq(&folded.0, &folded_again.0),
            "folded text is shared with the cache"
        );
        std::fs::write(&path, record("first") + &record("later")).unwrap();
        assert_eq!(
            index
                .read(&path, Some(Provider::Codex), "id")
                .unwrap()
                .0
                .len(),
            2
        );
        std::fs::write(&path, record("replaced")).unwrap();
        assert_eq!(
            index.read(&path, Some(Provider::Codex), "id").unwrap().0[0].text,
            "replaced"
        );
        std::fs::remove_file(path).unwrap();
        assert!(index.refresh(vec![]).documents.is_empty());
        assert!(index.files.is_empty());
    }
    #[test]
    fn superseded_searches_stop_early() {
        let documents = vec![document(false, false, "needle")];
        assert_eq!(
            search_until(&documents, "needle", || false).map(|hits| hits.len()),
            Some(1)
        );
        assert!(search_until(&documents, "needle", || true).is_none());
    }

    #[test]
    fn live_messages_are_searchable_without_a_provider_file_or_process() {
        let mut source = document(false, false, "").source.clone();
        source.live = Some(Arc::new(vec![Message {
            role: "Assistant",
            text: "streaming now".into(),
        }]));
        let snapshot = Index::default().refresh(vec![source]);
        assert_eq!(search(&snapshot.documents, "streaming").len(), 1);
        assert!(snapshot.errors.is_empty());
    }
}
