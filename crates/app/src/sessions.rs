//! The list of sessions shown in the sidebar: what each one is, which is
//! selected, and how the list is saved between runs.
//!
//! Pure data and logic, no UI and no backend calls, so it is unit-tested.

use del_agent::{Provider, SessionId, SessionStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Title of a session that has not been named yet.
pub const DEFAULT_TITLE: &str = "New session";

/// Longest title derived automatically from a first prompt, in characters.
const AUTO_TITLE_CHARS: usize = 40;

/// One row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Stable local key; never reused. Survives the backend assigning an id.
    pub key: u64,
    /// The provider's session id, once the session has been started.
    pub backend_id: Option<SessionId>,
    pub provider: Provider,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub title: String,
    /// True once the user renamed it, so a first prompt no longer retitles it.
    #[serde(default)]
    pub renamed: bool,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub archived: bool,
    /// Live state; never saved. Every session starts a run as `Idle`.
    #[serde(skip, default = "idle")]
    pub status: SessionStatus,
}

fn idle() -> SessionStatus {
    SessionStatus::Idle
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionList {
    entries: Vec<Entry>,
    active: Option<u64>,
    next_key: u64,
}

impl SessionList {
    /// Add a not-yet-started session and make it active. Returns its key.
    pub fn add(&mut self, provider: Provider, model: Option<String>, cwd: PathBuf, now: u64) -> u64 {
        let key = self.next_key;
        self.next_key += 1;
        self.entries.push(Entry {
            key,
            backend_id: None,
            provider,
            model,
            cwd,
            title: DEFAULT_TITLE.to_string(),
            renamed: false,
            created_at: now,
            updated_at: now,
            archived: false,
            status: SessionStatus::Idle,
        });
        self.active = Some(key);
        key
    }

    pub fn get(&self, key: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }

    pub fn get_mut(&mut self, key: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.key == key)
    }

    pub fn active(&self) -> Option<&Entry> {
        self.active.and_then(|k| self.get(k))
    }

    pub fn active_key(&self) -> Option<u64> {
        self.active
    }

    pub fn key_for_backend(&self, id: &SessionId) -> Option<u64> {
        self.entries
            .iter()
            .find(|e| e.backend_id.as_ref() == Some(id))
            .map(|e| e.key)
    }

    /// Sessions to list: not archived, most recently used first.
    pub fn visible(&self) -> Vec<&Entry> {
        let mut rows: Vec<&Entry> = self.entries.iter().filter(|e| !e.archived).collect();
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.key.cmp(&a.key)));
        rows
    }

    pub fn archived(&self) -> Vec<&Entry> {
        let mut rows: Vec<&Entry> = self.entries.iter().filter(|e| e.archived).collect();
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        rows
    }

    pub fn select(&mut self, key: u64) {
        if self.get(key).is_some_and(|e| !e.archived) {
            self.active = Some(key);
        }
    }

    /// Step through the visible list by `delta` rows, wrapping around.
    pub fn select_relative(&mut self, delta: isize) {
        let rows = self.visible();
        if rows.is_empty() {
            return;
        }
        let at = rows
            .iter()
            .position(|e| Some(e.key) == self.active)
            .unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(rows.len() as isize) as usize;
        self.active = Some(rows[next].key);
    }

    pub fn rename(&mut self, key: u64, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(e) = self.get_mut(key) {
            e.title = title.to_string();
            e.renamed = true;
        }
    }

    /// Name an untitled session after its first prompt.
    pub fn auto_title(&mut self, key: u64, prompt: &str) {
        let Some(e) = self.get_mut(key) else { return };
        if e.renamed || e.title != DEFAULT_TITLE {
            return;
        }
        let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        if line.is_empty() {
            return;
        }
        let mut title: String = line.chars().take(AUTO_TITLE_CHARS).collect();
        if line.chars().count() > AUTO_TITLE_CHARS {
            title.push('…');
        }
        e.title = title;
    }

    /// Archive a session. If it was active, the next visible one becomes
    /// active (or none).
    pub fn archive(&mut self, key: u64) {
        let Some(e) = self.get_mut(key) else { return };
        e.archived = true;
        if self.active == Some(key) {
            self.active = self.visible().first().map(|e| e.key);
        }
    }

    pub fn restore(&mut self, key: u64) {
        if let Some(e) = self.get_mut(key) {
            e.archived = false;
            self.active = Some(key);
        }
    }

    pub fn set_status(&mut self, key: u64, status: SessionStatus) {
        if let Some(e) = self.get_mut(key) {
            e.status = status;
        }
    }

    pub fn touch(&mut self, key: u64, now: u64) {
        if let Some(e) = self.get_mut(key) {
            e.updated_at = now;
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Parse a saved list. Corrupt or unreadable data yields an empty list
    /// rather than an error: losing the sidebar must never block startup.
    pub fn from_json(text: &str) -> Self {
        let mut list: Self = serde_json::from_str(text).unwrap_or_default();
        // Never trust a saved active key or counter.
        list.next_key = list.entries.iter().map(|e| e.key + 1).max().unwrap_or(0).max(list.next_key);
        if list.active.is_none_or(|k| list.get(k).is_none_or(|e| e.archived)) {
            list.active = list.visible().first().map(|e| e.key);
        }
        list
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::from_json(&t))
            .unwrap_or_default()
    }

    /// Write atomically (temp file, then rename) so a crash never leaves a
    /// half-written list.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(tmp, path)
    }
}

/// Where the list is saved: `<data dir>/del/sessions.json`.
pub fn default_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("del").join("sessions.json"))
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Short relative time: "now", "5m", "3h", "2d", "6w".
pub fn relative_time(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..=44 => "now".into(),
        45..=3_599 => format!("{}m", (s / 60).max(1)),
        3_600..=86_399 => format!("{}h", s / 3_600),
        86_400..=604_799 => format!("{}d", s / 86_400),
        _ => format!("{}w", s / 604_800),
    }
}

/// Plain-text provider glyph for the sidebar.
pub fn glyph(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "CX",
        Provider::Claude => "CL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list3() -> (SessionList, u64, u64, u64) {
        let mut l = SessionList::default();
        let a = l.add(Provider::Claude, None, "/a".into(), 10);
        let b = l.add(Provider::Codex, None, "/b".into(), 20);
        let c = l.add(Provider::Claude, None, "/c".into(), 30);
        (l, a, b, c)
    }

    #[test]
    fn add_activates_and_orders_by_recency() {
        let (l, a, b, c) = list3();
        assert_eq!(l.active_key(), Some(c));
        let keys: Vec<u64> = l.visible().iter().map(|e| e.key).collect();
        assert_eq!(keys, vec![c, b, a]);
    }

    #[test]
    fn archive_active_moves_selection_and_restore_reselects() {
        let (mut l, a, b, c) = list3();
        l.archive(c);
        assert_eq!(l.active_key(), Some(b));
        assert_eq!(l.archived().len(), 1);
        l.restore(c);
        assert_eq!(l.active_key(), Some(c));
        l.archive(a);
        l.archive(b);
        l.archive(c);
        assert_eq!(l.active_key(), None);
    }

    #[test]
    fn select_ignores_archived_and_unknown() {
        let (mut l, a, _, c) = list3();
        l.archive(a);
        l.select(a);
        l.select(99);
        assert_eq!(l.active_key(), Some(c));
    }

    #[test]
    fn relative_selection_wraps() {
        let (mut l, a, b, c) = list3();
        l.select_relative(1);
        assert_eq!(l.active_key(), Some(b));
        l.select_relative(1);
        assert_eq!(l.active_key(), Some(a));
        l.select_relative(1);
        assert_eq!(l.active_key(), Some(c));
        l.select_relative(-1);
        assert_eq!(l.active_key(), Some(a));
    }

    #[test]
    fn rename_trims_rejects_blank_and_blocks_auto_title() {
        let (mut l, a, ..) = list3();
        l.rename(a, "  Fix bug ");
        assert_eq!(l.get(a).unwrap().title, "Fix bug");
        l.rename(a, "   ");
        assert_eq!(l.get(a).unwrap().title, "Fix bug");
        l.auto_title(a, "something else");
        assert_eq!(l.get(a).unwrap().title, "Fix bug");
    }

    #[test]
    fn auto_title_uses_first_line_and_truncates() {
        let (mut l, a, b, _) = list3();
        l.auto_title(a, "\n  hello world \nsecond");
        assert_eq!(l.get(a).unwrap().title, "hello world");
        l.auto_title(a, "again");
        assert_eq!(l.get(a).unwrap().title, "hello world");
        l.auto_title(b, &"x".repeat(60));
        assert_eq!(l.get(b).unwrap().title.chars().count(), AUTO_TITLE_CHARS + 1);
    }

    #[test]
    fn json_round_trip_resets_status_and_keeps_counter() {
        let (mut l, a, _, c) = list3();
        l.set_status(a, SessionStatus::Running);
        l.get_mut(a).unwrap().backend_id = Some(SessionId::new("s1"));
        let back = SessionList::from_json(&l.to_json());
        assert_eq!(back.get(a).unwrap().status, SessionStatus::Idle);
        assert_eq!(back.get(a).unwrap().backend_id, Some(SessionId::new("s1")));
        assert_eq!(back.active_key(), Some(c));
        let mut back = back;
        assert_eq!(back.add(Provider::Codex, None, "/d".into(), 40), c + 1);
    }

    #[test]
    fn corrupt_json_is_empty() {
        assert!(SessionList::from_json("not json").visible().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("del-sessions-{}", std::process::id()));
        let path = dir.join("sessions.json");
        let (l, ..) = list3();
        l.save(&path).unwrap();
        assert_eq!(SessionList::load(&path).visible().len(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relative_time_buckets() {
        assert_eq!(relative_time(100, 100), "now");
        assert_eq!(relative_time(400, 100), "5m");
        assert_eq!(relative_time(100 + 7200, 100), "2h");
        assert_eq!(relative_time(100 + 3 * 86_400, 100), "3d");
        assert_eq!(relative_time(100 + 14 * 86_400, 100), "2w");
        assert_eq!(relative_time(5, 100), "now");
    }
}
