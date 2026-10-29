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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    order: Option<u64>,
}

/// User-level overrides on top of the manager's index, keyed by session id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn move_side_chat_unread(&mut self, from: &SessionId, to: &SessionId) {
        if let Some(meta) = self.0.get_mut(&from.0) {
            let unread = std::mem::take(&mut meta.unread_side_chats);
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
