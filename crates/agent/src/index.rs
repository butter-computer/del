//! Persistent session index: one JSON object per line, rewritten atomically.

use crate::backend::Result;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub provider: Provider,
    pub cwd: PathBuf,
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    pub permission_mode: PermissionMode,
    pub title: Option<String>,
    #[serde(default)]
    pub title_turns: u32,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub status: SessionStatus,
}

impl SessionRecord {
    pub fn config(&self) -> SessionConfig {
        SessionConfig {
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            service_tier: self.service_tier.clone(),
            provider: self.provider,
            permission_mode: self.permission_mode,
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `<user data dir>/del/sessions.jsonl`.
/// Directory for del's own data: `$DEL_DATA_DIR` if set (tests, trial runs),
/// otherwise `<platform data dir>/del`.
pub fn data_dir() -> Option<PathBuf> {
    match std::env::var_os("DEL_DATA_DIR") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => dirs::data_dir().map(|d| d.join("del")),
    }
}

pub fn default_index_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("sessions.jsonl"))
}

#[derive(Debug)]
pub struct SessionIndex {
    path: PathBuf,
    records: Vec<SessionRecord>,
}

impl SessionIndex {
    /// Load the index, creating nothing on disk until the first write.
    /// Unparseable lines are skipped. Sessions recorded as in-flight are
    /// normalized to `Idle`: nothing survives a restart.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut records: Vec<SessionRecord> = Vec::new();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    if let Ok(mut r) = serde_json::from_str::<SessionRecord>(line) {
                        if matches!(
                            r.status,
                            SessionStatus::Running | SessionStatus::WaitingApproval
                        ) {
                            r.status = SessionStatus::Idle;
                        }
                        records.retain(|x| x.id != r.id);
                        records.push(r);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Self { path, records })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Most recently updated first.
    pub fn list(&self) -> Vec<SessionRecord> {
        let mut v = self.records.clone();
        v.sort_by_key(|r| std::cmp::Reverse(r.updated_at_ms));
        v
    }

    pub fn get(&self, id: &SessionId) -> Option<&SessionRecord> {
        self.records.iter().find(|r| &r.id == id)
    }

    pub fn upsert(&mut self, record: SessionRecord) -> Result<()> {
        match self.records.iter_mut().find(|r| r.id == record.id) {
            Some(slot) => *slot = record,
            None => self.records.push(record),
        }
        self.flush()
    }

    pub fn update(&mut self, id: &SessionId, f: impl FnOnce(&mut SessionRecord)) -> Result<()> {
        if let Some(r) = self.records.iter_mut().find(|r| &r.id == id) {
            f(r);
            self.flush()?;
        }
        Ok(())
    }

    pub fn remove(&mut self, id: &SessionId) -> Result<()> {
        self.records.retain(|r| &r.id != id);
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = String::new();
        for r in &self.records {
            out.push_str(
                &serde_json::to_string(r).map_err(|e| crate::AgentError::Backend(e.to_string()))?,
            );
            out.push('\n');
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, at: u64, status: SessionStatus) -> SessionRecord {
        SessionRecord {
            id: SessionId::new(id),
            provider: Provider::Claude,
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::Ask,
            title: None,
            title_turns: 0,
            created_at_ms: at,
            updated_at_ms: at,
            status,
        }
    }

    #[test]
    fn persists_orders_and_normalizes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/sessions.jsonl");
        let mut idx = SessionIndex::open(&p).unwrap();
        assert!(idx.list().is_empty());
        idx.upsert(rec("a", 1, SessionStatus::Running)).unwrap();
        idx.upsert(rec("b", 2, SessionStatus::Error)).unwrap();
        idx.update(&SessionId::new("a"), |r| r.updated_at_ms = 3)
            .unwrap();
        let again = SessionIndex::open(&p).unwrap();
        let l = again.list();
        assert_eq!(
            l.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(l[0].status, SessionStatus::Idle);
        assert_eq!(l[1].status, SessionStatus::Error);
    }

    #[test]
    fn skips_garbage_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let good = serde_json::to_string(&rec("a", 1, SessionStatus::Idle)).unwrap();
        std::fs::write(&p, format!("not json\n{good}\n\n")).unwrap();
        let mut idx = SessionIndex::open(&p).unwrap();
        assert_eq!(idx.list().len(), 1);
        idx.remove(&SessionId::new("a")).unwrap();
        assert!(SessionIndex::open(&p).unwrap().list().is_empty());
    }
}
