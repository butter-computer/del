//! Persistent session index: one JSON object per line, rewritten atomically
//! by a background writer.

use crate::backend::Result;
use crate::types::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<crate::manager::ForkRecord>,
    pub id: SessionId,
    pub provider: Provider,
    #[serde(default)]
    pub account_id: Option<String>,
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

/// Background writer for the index file. Mutations hand it the serialized
/// file and return immediately; pending writes coalesce, so the in-memory
/// index stays authoritative and callers (including the UI thread waiting on
/// the index lock) never wait for disk I/O.
#[derive(Debug)]
struct Writer {
    path: PathBuf,
    state: Mutex<WriteState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct WriteState {
    pending: Option<Vec<u8>>,
    requested: u64,
    written: u64,
    error: Option<String>,
    closed: bool,
}

impl Writer {
    fn lock(&self) -> MutexGuard<'_, WriteState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self) {
        let mut state = self.lock();
        loop {
            if let Some(bytes) = state.pending.take() {
                let generation = state.requested;
                drop(state);
                let result = write_atomic(&self.path, &bytes);
                state = self.lock();
                state.written = generation;
                state.error = result.err().map(|e| {
                    eprintln!("[del] could not save {}: {e}", self.path.display());
                    e.to_string()
                });
                self.changed.notify_all();
            } else if state.closed {
                return;
            } else {
                state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

/// Waits for index writes requested so far to reach disk.
#[derive(Debug, Clone)]
pub struct IndexFlush(Arc<Writer>);

impl IndexFlush {
    /// Block until every write requested before this call has finished.
    /// Returns the most recent write error, if any.
    pub fn wait(&self) -> Result<()> {
        let mut state = self.0.lock();
        let target = state.requested;
        while state.written < target {
            state = self
                .0
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        match &state.error {
            Some(e) => Err(crate::AgentError::Backend(e.clone())),
            None => Ok(()),
        }
    }
}

/// Write `bytes` to a private, uniquely named temporary file beside `path`,
/// sync it, rename it over `path`, then sync the directory.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Debug)]
pub struct SessionIndex {
    path: PathBuf,
    records: Vec<SessionRecord>,
    temporary: HashSet<SessionId>,
    /// Lines this version cannot parse (corrupt, or written by a newer
    /// version), kept verbatim so a rewrite never erases them.
    unparsed: Vec<String>,
    writer: Arc<Writer>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SessionIndex {
    /// Load the index, creating nothing on disk until the first write.
    /// Unparseable lines are preserved verbatim. Sessions recorded as
    /// in-flight are normalized to `Idle`: nothing survives a restart.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut records: Vec<SessionRecord> = Vec::new();
        let mut unparsed = Vec::new();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut positions: HashMap<SessionId, usize> = HashMap::new();
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    match serde_json::from_str::<SessionRecord>(line) {
                        Ok(mut r) => {
                            if matches!(
                                r.status,
                                SessionStatus::Running | SessionStatus::WaitingApproval
                            ) {
                                r.status = SessionStatus::Idle;
                            }
                            // A later line for the same session wins.
                            match positions.get(&r.id) {
                                Some(&i) => records[i] = r,
                                None => {
                                    positions.insert(r.id.clone(), records.len());
                                    records.push(r);
                                }
                            }
                        }
                        Err(_) => unparsed.push(line.to_owned()),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let writer = Arc::new(Writer {
            path: path.clone(),
            state: Mutex::default(),
            changed: Condvar::new(),
        });
        Ok(Self {
            path,
            records,
            temporary: HashSet::new(),
            unparsed,
            writer,
            thread: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Most recently updated first.
    pub fn list(&self) -> Vec<SessionRecord> {
        let mut v: Vec<_> = self
            .records
            .iter()
            .filter(|r| !self.temporary.contains(&r.id))
            .cloned()
            .collect();
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

    pub fn upsert_temporary(&mut self, record: SessionRecord) -> Result<()> {
        self.temporary.insert(record.id.clone());
        self.upsert(record)
    }

    pub fn is_temporary(&self, id: &SessionId) -> bool {
        self.temporary.contains(id)
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
        self.temporary.remove(id);
        self.flush()
    }

    /// Handle for waiting until queued writes reach disk, without holding
    /// whatever lock guards this index.
    pub fn flusher(&self) -> IndexFlush {
        IndexFlush(self.writer.clone())
    }

    /// Queue the current contents for the background writer.
    fn flush(&mut self) -> Result<()> {
        let mut out = String::new();
        for line in &self.unparsed {
            out.push_str(line);
            out.push('\n');
        }
        for r in &self.records {
            if self.temporary.contains(&r.id) {
                continue;
            }
            out.push_str(
                &serde_json::to_string(r).map_err(|e| crate::AgentError::Backend(e.to_string()))?,
            );
            out.push('\n');
        }
        if self.thread.is_none() {
            let writer = self.writer.clone();
            self.thread = Some(
                std::thread::Builder::new()
                    .name("del-session-index".into())
                    .spawn(move || writer.run())?,
            );
        }
        let mut state = self.writer.lock();
        state.pending = Some(out.into_bytes());
        state.requested += 1;
        self.writer.changed.notify_all();
        Ok(())
    }
}

impl Drop for SessionIndex {
    /// Finish any queued write before the index goes away.
    fn drop(&mut self) {
        self.writer.lock().closed = true;
        self.writer.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, at: u64, status: SessionStatus) -> SessionRecord {
        SessionRecord {
            fork: None,
            id: SessionId::new(id),
            provider: Provider::Claude,
            account_id: None,
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
        idx.flusher().wait().unwrap();
        let again = SessionIndex::open(&p).unwrap();
        let l = again.list();
        assert_eq!(
            l.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(l[0].status, SessionStatus::Idle);
        assert_eq!(l[1].status, SessionStatus::Error);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "sessions.jsonl")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn preserves_unparsed_lines_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let good = serde_json::to_string(&rec("a", 1, SessionStatus::Idle)).unwrap();
        let newer = r#"{"id":"z","provider":"future_provider"}"#;
        std::fs::write(&p, format!("not json\n{good}\n\n{newer}\n")).unwrap();
        let mut idx = SessionIndex::open(&p).unwrap();
        assert_eq!(idx.list().len(), 1);
        idx.remove(&SessionId::new("a")).unwrap();
        drop(idx);
        assert!(SessionIndex::open(&p).unwrap().list().is_empty());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("not json\n"));
        assert!(text.contains(newer));
    }

    #[test]
    fn later_duplicate_lines_win() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let lines: Vec<_> = [
            rec("a", 1, SessionStatus::Idle),
            rec("b", 2, SessionStatus::Idle),
            rec("a", 5, SessionStatus::Error),
        ]
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect();
        std::fs::write(&p, lines.join("\n")).unwrap();
        let idx = SessionIndex::open(&p).unwrap();
        let l = idx.list();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].id.as_str(), l[0].status), ("a", SessionStatus::Error));
    }
}
