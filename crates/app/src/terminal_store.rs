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
        match std::fs::read(path) {
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
        }
    }
    pub fn upsert(&mut self, record: TerminalRecord) {
        if let Some(old) = self.records.iter_mut().find(|r| r.id == record.id) {
            *old = record;
        } else {
            self.records.push(record);
        }
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
}
