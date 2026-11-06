//! What the sidebar shows: the manager's session records plus the few things
//! it does not track (user renames and archiving), and how those are saved.
//!
//! Pure data and logic, no UI and no backend calls, so it is unit-tested.

use del_agent::{Provider, SessionId, SessionRecord, SessionStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Title shown for a session that has no prompt yet.
pub const DEFAULT_TITLE: &str = "New session";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Meta {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    unread: bool,
    #[serde(default, skip_serializing_if = "std::collections::HashSet::is_empty")]
    unread_side_chats: std::collections::HashSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    side_chat_costs: std::collections::BTreeMap<String, std::collections::BTreeMap<String, f64>>,
    #[serde(default)]
    order: Option<u64>,
}

/// User-level overrides on top of the manager's index, keyed by session id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overrides(HashMap<String, Meta>);

/// One sidebar row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: SessionId,
    pub provider: Provider,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub title: String,
    pub status: SessionStatus,
    pub updated_secs: u64,
    pub archived: bool,
    pub pinned: bool,
    pub unread: bool,
}

/// A terminal session stays separate from the manager's chat index.
#[derive(Debug, Clone)]
pub struct TerminalRow {
    pub account_id: Option<String>,
    pub id: SessionId,
    pub title: String,
    pub cwd: PathBuf,
    pub provider: Option<Provider>,
    pub active: bool,
    pub summary: String,
    pub pinned: bool,
}

impl Overrides {
    fn meta(&mut self, id: &SessionId) -> &mut Meta {
        self.0.entry(id.0.clone()).or_default()
    }

    /// Set a user title; blank input is ignored.
    pub fn rename(&mut self, id: &SessionId, title: &str) {
        let title = title.trim();
        if !title.is_empty() {
            self.meta(id).title = Some(title.to_string());
        }
    }

    pub fn set_archived(&mut self, id: &SessionId, archived: bool) {
        self.meta(id).archived = archived;
    }

    pub fn set_pinned(&mut self, id: &SessionId, pinned: bool) {
        let next = self
            .0
            .values()
            .filter_map(|meta| meta.order)
            .max()
            .unwrap_or(0)
            + 1;
        let meta = self.meta(id);
        if meta.pinned != pinned {
            meta.pinned = pinned;
            meta.order = pinned.then_some(next);
        }
    }

    pub fn set_order(&mut self, ids: &[SessionId]) {
        for (index, id) in ids.iter().enumerate() {
            self.meta(id).order = Some(index as u64 + 1);
        }
    }

    pub fn order(&self, id: &SessionId) -> Option<u64> {
        self.0.get(&id.0).and_then(|meta| meta.order)
    }

    pub fn initialize_pins(&mut self, records: &[SessionRecord]) {
        let ids: Vec<_> = self
            .rows(records)
            .into_iter()
            .filter(|row| row.pinned)
            .map(|row| row.id)
            .collect();
        if ids.iter().any(|id| self.order(id).is_none()) {
            self.set_order(&ids);
        }
    }

    pub fn set_unread(&mut self, id: &SessionId, unread: bool) {
        self.meta(id).unread = unread;
    }

    pub fn title(&self, id: &SessionId) -> Option<&str> {
        self.0.get(&id.0).and_then(|meta| meta.title.as_deref())
    }
    pub fn archived(&self, id: &SessionId) -> bool {
        self.0.get(&id.0).is_some_and(|meta| meta.archived)
    }
    pub fn main_unread(&self, id: &SessionId) -> bool {
        self.0.get(&id.0).is_some_and(|meta| meta.unread)
    }
    pub fn unread(&self, id: &SessionId) -> bool {
        self.0
            .get(&id.0)
            .is_some_and(|meta| meta.unread || !meta.unread_side_chats.is_empty())
    }
    pub fn side_chat_unread(&self, id: &SessionId, side: &str) -> bool {
        self.0
            .get(&id.0)
            .is_some_and(|meta| meta.unread_side_chats.contains(side))
    }
    pub fn set_side_chat_unread(&mut self, id: &SessionId, side: &str, unread: bool) {
        if unread {
            self.meta(id).unread_side_chats.insert(side.to_owned());
        } else {
            self.meta(id).unread_side_chats.remove(side);
        }
    }
    pub fn side_chat_costs(
        &self,
        id: &SessionId,
    ) -> impl Iterator<Item = &std::collections::BTreeMap<String, f64>> {
        self.0
            .get(&id.0)
            .into_iter()
            .flat_map(|meta| meta.side_chat_costs.values())
    }
    pub fn merge_side_chat_costs(
        &mut self,
        id: &SessionId,
        side: &str,
        costs: &std::collections::BTreeMap<String, f64>,
    ) -> bool {
        if costs.is_empty() {
            return false;
        }
        let saved = self
            .meta(id)
            .side_chat_costs
            .entry(side.to_owned())
            .or_default();
        let before = saved.clone();
        for (scope, cost) in costs {
            crate::session_usage::merge_cost(saved, scope, *cost);
        }
        *saved != before
    }
    pub fn move_side_chat_state(&mut self, from: &SessionId, to: &SessionId) {
        if let Some(meta) = self.0.get_mut(&from.0) {
            let unread = std::mem::take(&mut meta.unread_side_chats);
            let costs = std::mem::take(&mut meta.side_chat_costs);
            for (side, costs) in costs {
                self.merge_side_chat_costs(to, &side, &costs);
            }
            self.meta(to).unread_side_chats.extend(unread);
        }
    }

    pub fn remove(&mut self, id: &SessionId) {
        self.0.remove(&id.0);
    }

    /// Rows for every record, most recently used first. Records whose
    /// override says archived are marked, not dropped.
    pub fn rows(&self, records: &[SessionRecord]) -> Vec<Row> {
        let mut rows: Vec<Row> = records
            .iter()
            .map(|r| {
                let meta = self.0.get(&r.id.0);
                Row {
                    id: r.id.clone(),
                    provider: r.provider,
                    model: r.model.clone(),
                    cwd: r.cwd.clone(),
                    title: meta
                        .and_then(|m| m.title.clone())
                        .or_else(|| r.title.clone().filter(|t| !t.trim().is_empty()))
                        .unwrap_or_else(|| DEFAULT_TITLE.to_string()),
                    status: r.status,
                    updated_secs: r.updated_at_ms / 1000,
                    archived: meta.is_some_and(|m| m.archived),
                    pinned: meta.is_some_and(|m| m.pinned),
                    unread: meta.is_some_and(|m| m.unread || !m.unread_side_chats.is_empty()),
                }
            })
            .collect();
        rows.sort_by_cached_key(|r| {
            (
                !r.pinned,
                self.order(&r.id)
                    .unwrap_or(if r.pinned { 0 } else { u64::MAX }),
                std::cmp::Reverse(r.updated_secs),
            )
        });
        rows
    }

    /// A file that cannot be parsed is moved aside (never overwritten later)
    /// and reported; the sidebar then starts from defaults.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        load_or_set_aside(path, "sidebar state")
    }

    /// Write atomically (temp file, fsync, then rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(
            path,
            serde_json::to_string_pretty(self)?.as_bytes(),
            PRIVATE,
        )
    }
}

/// Coalesces sidebar-state writes off the UI thread. Each request carries a
/// generation; a write that is already superseded is skipped, and writes are
/// serialized so an older snapshot can never land after a newer one.
#[derive(Default)]
pub struct OverridesWriter {
    requested: std::sync::atomic::AtomicU64,
    written: std::sync::Mutex<u64>,
}

impl OverridesWriter {
    /// Record a new snapshot; returns its generation for [`Self::write`].
    pub fn request(&self) -> u64 {
        self.requested
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// Write `overrides` unless a newer request has already been written or queued.
    pub fn write(
        &self,
        generation: u64,
        overrides: &Overrides,
        path: &Path,
    ) -> std::io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if generation <= *written
            || generation < self.requested.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(());
        }
        overrides.save(path)?;
        *written = generation;
        Ok(())
    }

    /// True while a requested snapshot has not reached disk yet.
    pub fn pending(&self) -> bool {
        *self.written.lock().unwrap_or_else(|e| e.into_inner())
            < self.requested.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Owner-only mode for files holding account or session metadata.
pub const PRIVATE: u32 = 0o600;

/// Replace `path` atomically: write a unique temporary file beside it, flush
/// it to disk, then rename over the destination and flush the directory.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    if let Ok(dir) = std::fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// One background thread that performs [`write_atomic`] for UI-thread callers.
/// Requests for the same path coalesce: only the newest bytes are written, and
/// a path never has two writes in flight, so the file always ends up with the
/// latest snapshot. Each write is still fsynced; it just never blocks a frame.
struct Writer {
    state: std::sync::Mutex<WriterState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct WriterState {
    /// Waiting writes in request order; at most one entry per path.
    pending: Vec<(PathBuf, Vec<u8>, u32)>,
    /// The path being written right now, if any.
    writing: Option<PathBuf>,
}

fn writer() -> &'static Writer {
    static WRITER: std::sync::OnceLock<&'static Writer> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let writer: &'static Writer = Box::leak(Box::new(Writer {
            state: Default::default(),
            changed: std::sync::Condvar::new(),
        }));
        std::thread::Builder::new()
            .name("del-writes".into())
            .spawn(move || {
                loop {
                    let (path, bytes, mode) = {
                        let mut state = writer.state.lock().unwrap_or_else(|e| e.into_inner());
                        while state.pending.is_empty() {
                            state = writer
                                .changed
                                .wait(state)
                                .unwrap_or_else(|e| e.into_inner());
                        }
                        let next = state.pending.remove(0);
                        state.writing = Some(next.0.clone());
                        next
                    };
                    if let Err(error) = write_atomic(&path, &bytes, mode) {
                        eprintln!("Could not save {}: {error}", path.display());
                    }
                    writer
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .writing = None;
                    writer.changed.notify_all();
                }
            })
            .expect("spawn the background writer");
        writer
    })
}

/// Queue a durable [`write_atomic`] off the calling thread. A later request for
/// the same path replaces one that has not started, keeping writes ordered.
pub fn write_atomic_later(path: PathBuf, bytes: Vec<u8>, mode: u32) {
    let writer = writer();
    let mut state = writer.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.pending.iter_mut().find(|entry| entry.0 == path) {
        entry.1 = bytes;
        entry.2 = mode;
    } else {
        state.pending.push((path, bytes, mode));
    }
    drop(state);
    writer.changed.notify_all();
}

/// Block until every queued [`write_atomic_later`] has reached disk.
pub fn flush_pending_writes() {
    let writer = writer();
    let mut state = writer.state.lock().unwrap_or_else(|e| e.into_inner());
    while !state.pending.is_empty() || state.writing.is_some() {
        state = writer
            .changed
            .wait(state)
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Preserve an unreadable file as `<name>.corrupt-<unix seconds>` beside it.
/// `keep_original` copies instead of moving, for files whose absence would
/// itself lose data (the caller must then refuse to overwrite the original).
pub fn preserve_corrupt(path: &Path, keep_original: bool) -> std::io::Result<PathBuf> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut aside = path.with_file_name(format!("{name}.corrupt-{seconds}"));
    let mut n = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{name}.corrupt-{seconds}-{n}"));
        n += 1;
    }
    if keep_original {
        std::fs::copy(path, &aside)?;
    } else {
        std::fs::rename(path, &aside)?;
    }
    Ok(aside)
}

/// Read JSON from `path`; missing means default. An unparsable file is moved
/// aside so later saves cannot clobber it, and the problem is reported.
pub fn load_or_set_aside<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
    what: &str,
) -> (T, Option<String>) {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => (value, None),
            Err(error) => {
                let message = match preserve_corrupt(path, false) {
                    Ok(aside) => format!(
                        "Could not read {what}: {error}. The file was preserved as {}.",
                        aside.display()
                    ),
                    Err(move_error) => format!(
                        "Could not read {what}: {error}; could not preserve it: {move_error}"
                    ),
                };
                eprintln!("[del] {message}");
                (T::default(), Some(message))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (T::default(), None),
        Err(error) => (
            T::default(),
            Some(format!("Could not read {what}: {error}")),
        ),
    }
}

/// Where overrides are saved, next to the manager's index.
pub fn overrides_path() -> Option<PathBuf> {
    del_agent::data_dir().map(|d| d.join("sidebar.json"))
}

/// Last path component, for compact display.
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, title: Option<&str>, updated_ms: u64) -> SessionRecord {
        SessionRecord {
            segments: Vec::new(),
            fork: None,
            id: SessionId::new(id),
            provider: Provider::Claude,
            account_id: None,
            cwd: "/tmp/proj".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: Default::default(),
            title: title.map(str::to_string),
            title_turns: 0,
            created_at_ms: 0,
            updated_at_ms: updated_ms,
            status: SessionStatus::Idle,
        }
    }

    #[test]
    fn rows_sort_recent_first_and_default_the_title() {
        let rows = Overrides::default().rows(&[
            record("a", Some("one"), 1000),
            record("b", None, 5000),
            record("c", Some("  "), 3000),
        ]);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, vec![DEFAULT_TITLE, DEFAULT_TITLE, "one"]);
        assert_eq!(rows[0].id, SessionId::new("b"));
    }

    #[test]
    fn rename_wins_over_auto_title_and_blank_is_ignored() {
        let mut o = Overrides::default();
        let id = SessionId::new("a");
        o.rename(&id, "  Mine ");
        o.rename(&id, "   ");
        assert_eq!(o.rows(&[record("a", Some("auto"), 0)])[0].title, "Mine");
    }

    #[test]
    fn archive_marks_rows_and_restore_unmarks() {
        let mut o = Overrides::default();
        let id = SessionId::new("a");
        o.set_archived(&id, true);
        assert!(o.rows(&[record("a", None, 0)])[0].archived);
        o.set_archived(&id, false);
        assert!(!o.rows(&[record("a", None, 0)])[0].archived);
    }

    #[test]
    fn pinned_rows_sort_first_and_round_trip() {
        let mut overrides = Overrides::default();
        let older = SessionId::new("older");
        overrides.set_pinned(&older, true);
        overrides.set_unread(&older, true);
        let rows = overrides.rows(&[
            record("newer", Some("Newer"), 5_000),
            record("older", Some("Older"), 1_000),
        ]);
        assert_eq!(rows[0].id, older);
        assert!(rows[0].pinned);

        let saved = serde_json::to_string(&overrides).unwrap();
        let mut restored: Overrides = serde_json::from_str(&saved).unwrap();
        assert!(restored.rows(&[record("older", None, 0)])[0].pinned);
        assert!(restored.rows(&[record("older", None, 0)])[0].unread);
        restored.remove(&older);
        assert!(!restored.rows(&[record("older", None, 0)])[0].unread);
    }

    #[test]
    fn pin_order_is_stable_and_repin_appends() {
        let mut overrides = Overrides::default();
        let a = SessionId::new("a");
        let b = SessionId::new("b");
        overrides.set_pinned(&a, true);
        overrides.set_pinned(&b, true);
        let records = [record("b", None, 9000), record("a", None, 1000)];
        assert_eq!(
            overrides
                .rows(&records)
                .iter()
                .map(|r| &r.id)
                .collect::<Vec<_>>(),
            vec![&a, &b]
        );
        overrides.set_pinned(&a, false);
        overrides.set_pinned(&a, true);
        assert_eq!(overrides.rows(&records)[0].id, b);
        overrides.set_order(&[a.clone(), b.clone()]);
        let restored: Overrides =
            serde_json::from_str(&serde_json::to_string(&overrides).unwrap()).unwrap();
        assert_eq!(restored.rows(&records)[0].id, a);
    }

    #[test]
    fn manual_regular_order_survives_activity_and_legacy_pins_are_initialized() {
        let mut overrides: Overrides =
            serde_json::from_str(r#"{"a":{"pinned":true},"b":{"pinned":true}}"#).unwrap();
        let records = [
            record("a", None, 1000),
            record("b", None, 2000),
            record("c", None, 3000),
            record("d", None, 4000),
        ];
        overrides.initialize_pins(&records);
        overrides.set_order(&[SessionId::new("c"), SessionId::new("d")]);
        let mut changed = records.clone();
        changed[0].updated_at_ms = 90000;
        changed[3].updated_at_ms = 99000;
        assert_eq!(
            overrides
                .rows(&changed)
                .iter()
                .map(|r| r.id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a", "c", "d"]
        );
    }

    #[test]
    fn save_and_load_round_trip_and_corrupt_is_preserved_aside() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sidebar.json");
        let mut o = Overrides::default();
        o.rename(&SessionId::new("a"), "Mine");
        o.save(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(Overrides::load(&path), (o.clone(), None));
        std::fs::write(&path, "garbage").unwrap();
        let (loaded, error) = Overrides::load(&path);
        assert_eq!(loaded, Overrides::default());
        assert!(error.unwrap().contains("preserved"));
        // The unreadable file is kept beside the original, so saving defaults loses nothing.
        assert!(!path.exists());
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("sidebar.json.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            std::fs::read(dir.path().join(&aside[0])).unwrap(),
            b"garbage"
        );
        o.save(&path).unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            2,
            "no temporary files remain"
        );
    }

    #[test]
    fn older_and_newer_sidebar_files_load_with_defaults() {
        // Pre-unread/order files and fields from a newer build both load.
        let overrides: Overrides = serde_json::from_str(
            r#"{"a":{"title":"Mine"},"b":{"pinned":true,"future_field":[1,2]},"c":{}}"#,
        )
        .unwrap();
        let rows = overrides.rows(&[
            record("a", Some("auto"), 3000),
            record("b", None, 1000),
            record("c", None, 2000),
        ]);
        assert_eq!(rows[0].id, SessionId::new("b"));
        assert!(rows[0].pinned && !rows[0].unread && !rows[0].archived);
        assert_eq!(rows[1].title, "Mine");
        assert_eq!(overrides.order(&SessionId::new("a")), None);
    }

    #[test]
    fn pinning_twice_keeps_position_and_activity_never_reorders_pins() {
        let mut overrides = Overrides::default();
        let [a, b, c] = ["a", "b", "c"].map(SessionId::new);
        for id in [&a, &b, &c] {
            overrides.set_pinned(id, true);
        }
        // A repeated pin request (e.g. a duplicate click) must not move the pin.
        overrides.set_pinned(&a, true);
        let mut records = vec![
            record("a", None, 1),
            record("b", None, 2),
            record("c", None, 3),
        ];
        let ids = |o: &Overrides, r: &[SessionRecord]| {
            o.rows(r).into_iter().map(|r| r.id.0).collect::<Vec<_>>()
        };
        assert_eq!(ids(&overrides, &records), ["a", "b", "c"]);
        records[0].updated_at_ms = 99_000;
        records[2].updated_at_ms = 0;
        assert_eq!(ids(&overrides, &records), ["a", "b", "c"]);
        // Unpinned rows below the pins remain most-recent first.
        records.push(record("d", None, 5_000));
        records.push(record("e", None, 7_000));
        assert_eq!(ids(&overrides, &records), ["a", "b", "c", "e", "d"]);
        overrides.set_pinned(&b, false);
        assert_eq!(ids(&overrides, &records), ["a", "c", "e", "d", "b"]);
        overrides.set_pinned(&b, true);
        assert_eq!(ids(&overrides, &records), ["a", "c", "b", "e", "d"]);
    }

    #[test]
    fn legacy_pins_keep_their_visible_order_when_migrated() {
        let mut overrides: Overrides =
            serde_json::from_str(r#"{"old":{"pinned":true},"older":{"pinned":true}}"#).unwrap();
        let records = [record("older", None, 1_000), record("old", None, 2_000)];
        let before: Vec<_> = overrides.rows(&records).into_iter().map(|r| r.id).collect();
        overrides.initialize_pins(&records);
        let after: Vec<_> = overrides.rows(&records).into_iter().map(|r| r.id).collect();
        assert_eq!(before, after);
        // Once migrated, later activity does not reorder them, and a new pin appends.
        overrides.set_pinned(&SessionId::new("new"), true);
        let records = [
            record("older", None, 9_000),
            record("old", None, 2_000),
            record("new", None, 50_000),
        ];
        assert_eq!(
            overrides
                .rows(&records)
                .into_iter()
                .map(|r| r.id.0)
                .collect::<Vec<_>>(),
            ["old", "older", "new"]
        );
    }

    #[test]
    fn unread_state_is_per_session_and_side_chats_are_separate() {
        let mut overrides = Overrides::default();
        let [a, b] = ["a", "b"].map(SessionId::new);
        overrides.set_unread(&a, true);
        overrides.set_side_chat_unread(&b, "fork", true);
        assert!(overrides.unread(&a) && overrides.main_unread(&a));
        assert!(overrides.unread(&b) && !overrides.main_unread(&b));
        // Clearing the main reply leaves hidden side-chat replies unread.
        overrides.set_unread(&b, false);
        assert!(overrides.unread(&b));
        overrides.set_side_chat_unread(&b, "fork", false);
        assert!(!overrides.unread(&b));
        assert!(overrides.unread(&a), "other chats keep their dots");
        let rows = overrides.rows(&[record("a", None, 0), record("b", None, 0)]);
        assert_eq!(
            rows.iter()
                .map(|r| (r.id.0.as_str(), r.unread))
                .collect::<Vec<_>>(),
            [("a", true), ("b", false)]
        );
    }

    #[test]
    fn atomic_writes_replace_whole_files_and_failures_keep_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        write_atomic(&path, b"first", PRIVATE).unwrap();
        write_atomic(&path, b"second", PRIVATE).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // Renaming over a directory fails; the existing file must remain intact.
        let blocked = dir.path().join("nested/blocked");
        std::fs::create_dir_all(blocked.join("child")).unwrap();
        assert!(write_atomic(&blocked, b"new", PRIVATE).is_err());
        assert!(blocked.join("child").is_dir());
        let names: Vec<_> = std::fs::read_dir(dir.path().join("nested"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    }

    #[test]
    fn missing_settings_load_as_defaults_without_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (loaded, error): (Overrides, _) =
            load_or_set_aside(&dir.path().join("absent.json"), "sidebar state");
        assert_eq!(loaded, Overrides::default());
        assert!(error.is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn coalesced_writer_never_lands_an_older_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sidebar.json");
        let writer = OverridesWriter::default();
        let mut old = Overrides::default();
        old.rename(&SessionId::new("a"), "Old");
        let mut new = old.clone();
        new.rename(&SessionId::new("a"), "New");
        let first = writer.request();
        let second = writer.request();
        assert!(writer.pending());
        writer.write(second, &new, &path).unwrap();
        writer.write(first, &old, &path).unwrap();
        assert_eq!(Overrides::load(&path).0, new);
        assert!(!writer.pending());
    }
}
