//! Terminal launch history is independent of the existing agent chat index.
use crate::terminal_host::TerminalLaunchSpec;
use del_agent::SessionId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct TerminalRecord {
    pub id: SessionId,
    pub title: String,
    pub launch: TerminalLaunchSpec,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct TerminalStore {
    #[serde(default)]
    pub records: Vec<TerminalRecord>,
    #[serde(skip)]
    pub error: Option<String>,
}
impl TerminalStore {
    pub fn path() -> PathBuf {
        del_agent::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("terminals.json")
    }
    pub fn load(path: &Path) -> Self {
        let mut store: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| Self {
                records: Vec::new(),
                error: Some(format!(
                    "Could not read terminal history: {e}. Existing file preserved."
                )),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => Self {
                records: Vec::new(),
                error: Some(format!("Could not read terminal history: {e}")),
            },
        };
        for record in &mut store.records {
            if record.launch.kind == crate::terminal_host::TerminalLaunchKind::Claude {
                record.title =
                    crate::terminal_conversation::claude_osc_title(&record.title).to_owned();
            }
            if crate::terminal_conversation::title_is_pending(&record.title) {
                record.title = record.launch.kind.label().to_owned();
            }
        }
        store
    }
    pub fn upsert(&mut self, mut record: TerminalRecord) {
        if let Some(old) = self.records.iter_mut().find(|r| r.id == record.id) {
            record.pinned = old.pinned;
            if record.conversation_id.is_none() {
                record.conversation_id = old.conversation_id.clone();
            }
            *old = record;
        } else {
            self.records.push(record);
        }
    }
    /// Returns true only when a provider publishes a different nonempty title.
    pub fn set_title(&mut self, id: &SessionId, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() || crate::terminal_conversation::title_is_pending(title) {
            return false;
        }
        let Some(record) = self.records.iter_mut().find(|record| &record.id == id) else {
            return false;
        };
        if record.title == title {
            return false;
        }
        record.title = title.to_owned();
        true
    }

    pub fn set_pinned(&mut self, id: &SessionId, pinned: bool) {
        if let Some(record) = self.records.iter_mut().find(|r| &r.id == id) {
            record.pinned = pinned;
        }
    }
    pub fn remove(&mut self, id: &SessionId) {
        self.records.retain(|record| &record.id != id);
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(error) = &self.error {
            return Err(std::io::Error::other(error.clone()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let data = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopened_terminal_keeps_its_exact_provider_conversation() {
        let saved = r#"{"records":[{"id":"terminal-a","title":"First task","conversation_id":"01a16585-21de-7e41-9a31-9857f8268084","launch":{"kind":"codex","cwd":"/tmp","program":"codex","args":[],"title":"Codex","login_env":true}},{"id":"terminal-b","title":"Second task","conversation_id":"01a16585-7da2-71c1-af17-6c8de9e50723","launch":{"kind":"codex","cwd":"/tmp","program":"codex","args":[],"title":"Codex","login_env":true}}]}"#;
        let store: TerminalStore = serde_json::from_str(saved).unwrap();
        let roundtrip = serde_json::to_value(&store).unwrap();
        assert_eq!(
            roundtrip["records"][0]["conversation_id"],
            "01a16585-21de-7e41-9a31-9857f8268084"
        );
        assert_eq!(
            roundtrip["records"][1]["conversation_id"],
            "01a16585-7da2-71c1-af17-6c8de9e50723"
        );
    }

    #[test]
    fn provider_title_changes_persist_without_rewriting_launch_or_other_sessions() {
        let mut store = TerminalStore::default();
        for id in ["first", "second"] {
            store.upsert(TerminalRecord {
                id: SessionId::new(id),
                title: "Codex".into(),
                launch: TerminalLaunchSpec::shell(std::env::temp_dir()),
                pinned: false,
                conversation_id: None,
            });
        }
        let first = SessionId::new("first");
        assert!(store.set_title(&first, "Actual conversation title"));
        assert!(!store.set_title(&first, "Actual conversation title"));
        assert!(!store.set_title(&first, ""));
        assert!(!store.set_title(&first, "⠧ | renaming... ⠧"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminals.json");
        store.save(&path).unwrap();
        let loaded = TerminalStore::load(&path);
        assert_eq!(loaded.records[0].title, "Actual conversation title");
        assert_eq!(loaded.records[0].launch.title, "Shell");
        assert_eq!(loaded.records[1].title, "Codex");
    }

    #[test]
    fn legacy_progress_titles_load_as_provider_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminals.json");
        std::fs::write(&path, r#"{"records":[{"id":"t","title":"⠧ | renaming... ⠧","launch":{"kind":"codex","cwd":"/tmp","args":[],"title":"Codex","login_env":true}}]}"#).unwrap();
        let store = TerminalStore::load(&path);
        assert!(store.error.is_none());
        assert_eq!(store.records[0].title, "Codex");
    }

    #[test]
    fn unreadable_history_is_preserved_instead_of_overwritten() {
        let path =
            std::env::temp_dir().join(format!("del-terminal-history-{}.json", std::process::id()));
        std::fs::write(&path, "invalid existing history").unwrap();
        let store = TerminalStore::load(&path);
        assert!(store.error.is_some());
        assert!(store.save(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "invalid existing history"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_records_default_to_unpinned_and_pin_changes_persist() {
        let old = r#"{"records":[{"id":"shell","title":"Shell","launch":{"kind":"shell","cwd":"/tmp","program":null,"args":[],"title":"Shell","login_env":true}}]}"#;
        let mut store: TerminalStore = serde_json::from_str(old).unwrap();
        assert!(!store.records[0].pinned);
        store.set_pinned(&SessionId::new("shell"), true);
        let saved = serde_json::to_string(&store).unwrap();
        let restored: TerminalStore = serde_json::from_str(&saved).unwrap();
        assert!(restored.records[0].pinned);
        store.remove(&SessionId::new("shell"));
        assert!(store.records.is_empty());
    }
}
