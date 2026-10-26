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
        let next = self.0.values().filter_map(|meta| meta.order).max().unwrap_or(0) + 1;
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
        let ids: Vec<_> = self.rows(records).into_iter().filter(|row| row.pinned).map(|row| row.id).collect();
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
    pub fn unread(&self, id: &SessionId) -> bool {
        self.0.get(&id.0).is_some_and(|meta| meta.unread)
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
                    unread: meta.is_some_and(|m| m.unread),
                }
            })
            .collect();
        rows.sort_by_key(|r| (!r.pinned,
            self.order(&r.id).unwrap_or(if r.pinned { 0 } else { u64::MAX }),
            std::cmp::Reverse(r.updated_secs)));
        rows
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write atomically (temp file, then rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
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
        assert_eq!(overrides.rows(&records).iter().map(|r| &r.id).collect::<Vec<_>>(), vec![&a, &b]);
        overrides.set_pinned(&a, false);
        overrides.set_pinned(&a, true);
        assert_eq!(overrides.rows(&records)[0].id, b);
        overrides.set_order(&[a.clone(), b.clone()]);
        let restored: Overrides = serde_json::from_str(&serde_json::to_string(&overrides).unwrap()).unwrap();
        assert_eq!(restored.rows(&records)[0].id, a);
    }

    #[test]
    fn manual_regular_order_survives_activity_and_legacy_pins_are_initialized() {
        let mut overrides: Overrides = serde_json::from_str(r#"{"a":{"pinned":true},"b":{"pinned":true}}"#).unwrap();
        let records = [record("a", None, 1000), record("b", None, 2000), record("c", None, 3000), record("d", None, 4000)];
        overrides.initialize_pins(&records);
        overrides.set_order(&[SessionId::new("c"), SessionId::new("d")]);
        let mut changed = records.clone();
        changed[0].updated_at_ms = 90000;
        changed[3].updated_at_ms = 99000;
        assert_eq!(overrides.rows(&changed).iter().map(|r| r.id.0.as_str()).collect::<Vec<_>>(), vec!["b", "a", "c", "d"]);
    }

    #[test]
    fn save_and_load_round_trip_and_corrupt_is_empty() {
        let dir = std::env::temp_dir().join(format!("del-sidebar-{}", std::process::id()));
        let path = dir.join("sidebar.json");
        let mut o = Overrides::default();
        o.rename(&SessionId::new("a"), "Mine");
        o.save(&path).unwrap();
        assert_eq!(Overrides::load(&path), o);
        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(Overrides::load(&path), Overrides::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
