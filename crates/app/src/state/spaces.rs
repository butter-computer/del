//! Persistent workspace collections. Switching a collection never closes a session.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Deck {
    pub tabs: Vec<Tab>,
    pub selected: Option<Tab>,
    pub preview: Option<Tab>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub projects: BTreeSet<PathBuf>,
    #[serde(default)]
    pub pins: HashMap<String, bool>,
    #[serde(default)]
    pub order: Vec<SessionId>,
    #[serde(default)]
    pub deck: Deck,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Spaces {
    pub items: Vec<Space>,
    pub active: String,
    pub global_tabs: bool,
    pub global: Deck,
}
impl Default for Spaces {
    fn default() -> Self {
        Self {
            items: vec![Space {
                id: "default".into(),
                name: "Default".into(),
                projects: Default::default(),
                pins: Default::default(),
                order: Vec::new(),
                deck: Deck::default(),
            }],
            active: "default".into(),
            global_tabs: false,
            global: Deck::default(),
        }
    }
}
impl Spaces {
    pub fn current(&self) -> &Space {
        self.items
            .iter()
            .find(|s| s.id == self.active)
            .unwrap_or(&self.items[0])
    }
    pub fn current_mut(&mut self) -> &mut Space {
        let at = self
            .items
            .iter()
            .position(|s| s.id == self.active)
            .unwrap_or(0);
        &mut self.items[at]
    }
    pub fn includes(&self, root: &std::path::Path) -> bool {
        self.includes_in(self.current(), root)
    }
    pub fn includes_in(&self, space: &Space, root: &std::path::Path) -> bool {
        space.projects.contains(root)
            || (space.id == self.items[0].id
                && !self.items.iter().any(|s| s.projects.contains(root)))
    }
    fn normalize(&mut self) {
        let mut seen = HashSet::new();
        self.items
            .retain(|s| !s.id.is_empty() && seen.insert(s.id.clone()));
        if self.items.is_empty() {
            *self = Self::default();
        }
        if !self.items.iter().any(|s| s.id == self.active) {
            self.active = self.items[0].id.clone();
        }
        for s in &mut self.items {
            if s.name.trim().is_empty() {
                s.name = "Space".into();
            }
        }
    }
}

/// Finger-driven horizontal paging; only a completed settle changes the active space.
#[derive(Default)]
pub struct Swipe {
    pub offset: f32,
    pub width: f32,
    pub target: Option<String>,
    x: f32,
    y: f32,
    horizontal: bool,
    vertical: bool,
    ended: bool,
    last: Option<std::time::Instant>,
    settle: Option<(std::time::Instant, f32, f32)>,
}
impl Swipe {
    pub fn update(
        &mut self,
        (x, y): (f32, f32),
        phase: gpui::TouchPhase,
        now: std::time::Instant,
        width: f32,
        previous: Option<String>,
        next: Option<String>,
    ) -> bool {
        if matches!(phase, gpui::TouchPhase::Started)
            || self.last.is_none()
            || (!self.ended
                && self
                    .last
                    .is_some_and(|t| now.duration_since(t) > Duration::from_millis(300)))
        {
            *self = Self {
                width: width.max(1.),
                ..Self::default()
            };
        }
        self.last = Some(now);
        if self.ended {
            return self.horizontal;
        }
        if matches!(phase, gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled) {
            self.ended = true;
            let commit = !matches!(phase, gpui::TouchPhase::Cancelled)
                && self.target.is_some()
                && self.offset.abs() >= self.width * 0.25;
            let end = if commit {
                self.offset.signum() * self.width
            } else {
                0.
            };
            self.settle = Some((now, self.offset, end));
            return self.horizontal;
        }
        self.x += x;
        self.y += y;
        if !self.horizontal && self.y.abs() > 6. && self.y.abs() > self.x.abs() * 1.8 {
            self.vertical = true;
        }
        if self.vertical {
            return false;
        }
        if !self.horizontal && self.x.abs() > 6. && self.x.abs() > self.y.abs() * 1.8 {
            self.horizontal = true;
        }
        if self.horizontal {
            self.target = if self.x < 0. { next } else { previous };
            self.offset = if self.target.is_some() {
                self.x.clamp(-self.width, self.width)
            } else {
                self.x.signum() * (self.x.abs() * 0.18).min(self.width * 0.15)
            };
        }
        self.horizontal
    }
    pub fn settling(&self) -> bool {
        self.settle.is_some()
    }
    pub fn advance(&mut self, now: std::time::Instant, reduced: bool) -> Option<String> {
        let (start, from, to) = self.settle?;
        let t = if reduced {
            1.
        } else {
            (now.duration_since(start).as_secs_f32() / 0.22).min(1.)
        };
        self.offset = from + (to - from) * (1. - (1. - t).powi(4));
        if t < 1. {
            return None;
        }
        let target = if to != 0. { self.target.take() } else { None };
        self.offset = 0.;
        self.target = None;
        self.settle = None;
        target
    }
}

impl AppState {
    fn space_deck(&self) -> Deck {
        Deck {
            tabs: self.tabs.clone(),
            selected: self
                .tabs
                .iter()
                .find(|t| t.selection() == self.selection)
                .cloned(),
            preview: self.preview_tab.clone(),
        }
    }
    fn checkpoint_space(&mut self) {
        let deck = self.space_deck();
        if self.spaces.global_tabs {
            self.spaces.global = deck;
        } else {
            self.spaces.current_mut().deck = deck;
        }
    }
    pub fn all_open_tabs(&self) -> Vec<Tab> {
        let mut tabs = self.tabs.clone();
        for space in &self.spaces.items {
            if !self.spaces.global_tabs && space.id == self.spaces.active {
                continue;
            }
            for tab in &space.deck.tabs {
                if !tabs.contains(tab) {
                    tabs.push(tab.clone());
                }
            }
        }
        tabs
    }
    fn valid_space_tab(&self, tab: &Tab) -> bool {
        match tab {
            Tab::Draft(k) => self.drafts.contains_key(k),
            Tab::Session(id) => self.records.iter().any(|r| &r.id == id),
            Tab::Terminal(id) => self.terminal_rows.iter().any(|r| &r.id == id),
        }
    }
    fn restore_space_deck(&mut self, mut deck: Deck, cx: &mut Context<Self>) {
        deck.tabs.retain(|t| self.valid_space_tab(t));
        self.tabs = deck.tabs;
        for tab in self.tabs.clone() {
            if !self.uids.contains_key(&tab) {
                self.new_uid(tab);
            }
        }
        self.preview_tab = deck.preview.filter(|t| self.tabs.contains(t));
        let selected = deck
            .selected
            .filter(|t| self.tabs.contains(t))
            .or_else(|| self.tabs.first().cloned());
        if let Some(tab) = selected {
            self.select_tab(&tab, cx);
        } else {
            let cwd = self
                .spaces
                .current()
                .projects
                .iter()
                .next()
                .cloned()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("/"));
            let provider = self.effective().0;
            self.open_draft(provider, None, cwd, cx);
        }
    }
    pub fn switch_space(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == self.spaces.active || !self.spaces.items.iter().any(|s| s.id == id) {
            return;
        }
        self.checkpoint_space();
        self.space_swipe = Swipe {
            ended: true,
            last: Some(std::time::Instant::now()),
            ..Swipe::default()
        };
        self.spaces.active = id.to_owned();
        self.ghosts.clear();
        self.renaming = None;
        self.project_renaming = None;
        self.show_archived = false;
        self.sidebar_scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        self.sidebar_hover = Default::default();
        self.sidebar_drag = Default::default();
        if !self.spaces.global_tabs {
            self.restore_space_deck(self.spaces.current().deck.clone(), cx);
        }
        cx.notify();
    }
    pub fn create_space(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        // Existing projects stay in their original collection when another space adds one.
        if self.spaces.items.len() == 1 {
            let projects: Vec<_> = self
                .unified_rows()
                .into_iter()
                .map(|r| self.project_root(&r.cwd))
                .chain(self.drafts.values().map(|d| self.project_root(&d.cwd)))
                .chain(self.project_preferences.roots.iter().cloned())
                .collect();
            self.spaces.current_mut().projects.extend(projects);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.spaces.items.push(Space {
            id: id.clone(),
            name: name.to_owned(),
            projects: Default::default(),
            pins: Default::default(),
            order: Vec::new(),
            deck: Deck::default(),
        });
        self.switch_space(&id, cx);
    }
    pub fn rename_space(&mut self, id: &str, name: String, cx: &mut Context<Self>) {
        if name.trim().is_empty() {
            return;
        }
        if let Some(space) = self.spaces.items.iter_mut().find(|s| s.id == id) {
            space.name = name.trim().to_owned();
        }
        cx.notify();
    }
    /// Move collection membership and its UI state; sessions and processes stay alive.
    pub fn move_project_to_space(
        &mut self,
        path: &std::path::Path,
        destination: &str,
        cx: &mut Context<Self>,
    ) {
        let root = self.project_root(path);
        if destination == self.spaces.active
            || !self.spaces.items.iter().any(|s| s.id == destination)
            || !self.space_has_project(&root)
        {
            return;
        }
        self.checkpoint_space();
        let ids: HashSet<_> = self
            .records
            .iter()
            .map(|r| (&r.id, &r.cwd))
            .chain(self.terminal_rows.iter().map(|r| (&r.id, &r.cwd)))
            .filter(|(_, cwd)| self.project_root(cwd) == root)
            .map(|(id, _)| id.clone())
            .collect();
        let drafts: HashSet<_> = self
            .drafts
            .iter()
            .filter(|(_, d)| self.project_root(&d.cwd) == root)
            .map(|(id, _)| *id)
            .collect();
        let belongs = |tab: &Tab| match tab {
            Tab::Draft(id) => drafts.contains(id),
            Tab::Session(id) | Tab::Terminal(id) => ids.contains(id),
        };
        let destination_pins: HashMap<_, _> = self
            .space_rows_for(
                self.spaces
                    .items
                    .iter()
                    .find(|space| space.id == destination)
                    .unwrap(),
            )
            .into_iter()
            .filter(|row| ids.contains(&row.id))
            .map(|row| (row.id.0, row.pinned))
            .collect();
        let pins: Vec<_> = self
            .space_rows()
            .into_iter()
            .filter(|r| ids.contains(&r.id))
            .map(|r| (r.id.0, r.pinned))
            .collect();
        let global_tabs: Vec<_> = if self.spaces.global_tabs {
            self.tabs.iter().filter(|t| belongs(t)).cloned().collect()
        } else {
            Vec::new()
        };
        let source = self.spaces.current_mut();
        source.projects.remove(&root);
        let tabs: Vec<_> = source
            .deck
            .tabs
            .iter()
            .filter(|t| belongs(t))
            .cloned()
            .chain(global_tabs)
            .collect();
        source.deck.tabs.retain(|t| !belongs(t));
        let selected = source.deck.selected.take();
        source.deck.selected = selected.clone().filter(|t| !belongs(t));
        source.deck.preview = source.deck.preview.take().filter(|t| !belongs(t));
        let order: Vec<_> = source
            .order
            .iter()
            .filter(|id| ids.contains(*id))
            .cloned()
            .collect();
        source.order.retain(|id| !ids.contains(id));
        source
            .pins
            .retain(|id, _| !ids.contains(&SessionId(id.clone())));
        let target = self
            .spaces
            .items
            .iter_mut()
            .find(|s| s.id == destination)
            .unwrap();
        target.projects.insert(root);
        for (id, pinned) in pins {
            let pinned = pinned || destination_pins.get(&id).copied().unwrap_or(false);
            target
                .pins
                .entry(id)
                .and_modify(|value| *value |= pinned)
                .or_insert(pinned);
        }
        for id in order {
            if !target.order.contains(&id) {
                target.order.push(id);
            }
        }
        for tab in tabs {
            if !target.deck.tabs.contains(&tab) {
                target.deck.tabs.push(tab);
            }
        }
        if target.deck.selected.is_none() {
            target.deck.selected = selected.filter(belongs);
        }
        if !self.spaces.global_tabs {
            let panel = self.panel;
            self.restore_space_deck(self.spaces.current().deck.clone(), cx);
            self.panel = panel;
        }
        self.sidebar_hover = Default::default();
        self.sidebar_drag = Default::default();
        cx.notify();
    }

    pub fn delete_space(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.spaces.items.len() <= 1 {
            return;
        }
        let Some(index) = self.spaces.items.iter().position(|s| s.id == id) else {
            return;
        };
        self.checkpoint_space();
        let removed = self.spaces.items.remove(index);
        let target = &mut self.spaces.items[0];
        target.projects.extend(removed.projects);
        for (id, pinned) in removed.pins {
            target.pins.entry(id).or_insert(pinned);
        }
        for id in removed.order {
            if !target.order.contains(&id) {
                target.order.push(id);
            }
        }
        for tab in removed.deck.tabs {
            if !target.deck.tabs.contains(&tab) {
                target.deck.tabs.push(tab);
            }
        }
        if target.deck.selected.is_none() {
            target.deck.selected = removed.deck.selected;
        }
        let destination = target.id.clone();
        if self.spaces.active == id {
            self.spaces.active = destination.clone();
        }
        if self.spaces.active == destination && !self.spaces.global_tabs {
            self.restore_space_deck(self.spaces.current().deck.clone(), cx);
        }
        self.space_swipe = Default::default();
        cx.notify();
    }
    pub fn set_global_space_tabs(&mut self, global: bool, cx: &mut Context<Self>) {
        if global == self.spaces.global_tabs {
            return;
        }
        // Restoring a deck selects its chat, but changing this preference must
        // leave the current Settings page open.
        let panel = self.panel;
        self.checkpoint_space();
        if global {
            self.tabs = self.all_open_tabs();
            self.spaces.global_tabs = true;
            self.spaces.global = self.space_deck();
        } else {
            let global = self.space_deck();
            let known: Vec<_> = self
                .spaces
                .items
                .iter()
                .flat_map(|s| s.deck.tabs.clone())
                .collect();
            let deck = &mut self.spaces.current_mut().deck;
            deck.tabs
                .extend(global.tabs.iter().filter(|t| !known.contains(t)).cloned());
            if global
                .selected
                .as_ref()
                .is_some_and(|t| deck.tabs.contains(t))
            {
                deck.selected = global.selected;
            }
            self.spaces.global_tabs = false;
            self.restore_space_deck(self.spaces.current().deck.clone(), cx);
        }
        self.panel = panel;
        self.ghosts.clear();
        cx.notify();
    }
    pub fn draft_project(&self, key: u64) -> Option<PathBuf> {
        self.drafts.get(&key).map(|d| self.project_root(&d.cwd))
    }
    pub fn space_rows(&self) -> Vec<Row> {
        self.space_rows_for(self.spaces.current())
    }
    pub fn space_rows_for(&self, space: &Space) -> Vec<Row> {
        let mut rows: Vec<_> = self
            .unified_rows()
            .into_iter()
            .filter(|r| self.spaces.includes_in(space, &self.project_root(&r.cwd)))
            .map(|mut r| {
                r.pinned = space
                    .pins
                    .get(r.id.as_str())
                    .copied()
                    .unwrap_or(space.id == self.spaces.items[0].id && r.pinned);
                r
            })
            .collect();
        let order = &space.order;
        rows.sort_by_key(|r| {
            (
                !r.pinned,
                order
                    .iter()
                    .position(|id| id == &r.id)
                    .unwrap_or(usize::MAX),
            )
        });
        rows
    }
    pub fn space_has_project(&self, path: &std::path::Path) -> bool {
        self.spaces.includes(&self.project_root(path))
    }
    pub fn space_pinned(&self, id: &SessionId, fallback: bool) -> bool {
        self.spaces
            .current()
            .pins
            .get(id.as_str())
            .copied()
            .unwrap_or(self.spaces.active == self.spaces.items[0].id && fallback)
    }
    pub(super) fn toggle_space_pin(
        &mut self,
        id: &SessionId,
        fallback: bool,
        cx: &mut Context<Self>,
    ) {
        let pinned = self.space_pinned(id, fallback);
        // Seed from the displayed order so a new pin appends after existing pins.
        let rows = self.space_rows();
        let space = self.spaces.current_mut();
        if space.order.is_empty() {
            space.order = rows
                .into_iter()
                .filter(|r| r.pinned)
                .map(|r| r.id)
                .collect();
        }
        space.order.retain(|old| old != id);
        if !pinned {
            space.order.push(id.clone());
        }
        space.pins.insert(id.0.clone(), !pinned);
        cx.notify();
    }
    pub fn attach_space_project(&mut self, path: &std::path::Path) {
        let root = self.project_root(path);
        self.spaces.current_mut().projects.insert(root);
    }
    pub fn restore_spaces(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let Some(path) = del_agent::data_dir().map(|d| d.join("spaces.json")) else {
            return;
        };
        if !path.exists() {
            return;
        }
        let (mut spaces, error): (Spaces, _) = sessions::load_or_set_aside(&path, "spaces");
        if let Some(error) = error {
            self.error = Some(error);
            return;
        }
        spaces.normalize();
        self.spaces = spaces;
        for tab in self.all_open_tabs() {
            if !self.uids.contains_key(&tab) {
                self.new_uid(tab);
            }
        }
        let deck = if self.spaces.global_tabs {
            self.spaces.global.clone()
        } else {
            self.spaces.current().deck.clone()
        };
        self.restore_space_deck(deck, cx);
        cx.notify();
    }
    pub fn save_spaces(&self) -> std::io::Result<()> {
        if cfg!(test) {
            return Ok(());
        }
        let Some(path) = del_agent::data_dir().map(|d| d.join("spaces.json")) else {
            return Ok(());
        };
        let mut spaces = self.spaces.clone();
        if spaces.global_tabs {
            spaces.global = self.space_deck();
        } else {
            spaces.current_mut().deck = self.space_deck();
        }
        let bytes = serde_json::to_vec(&spaces)?;
        if std::fs::read(&path).ok().as_deref() != Some(&bytes) {
            sessions::write_atomic(&path, &bytes, sessions::PRIVATE)?;
        }
        Ok(())
    }
    pub(super) fn forget_space_tab(&mut self, tab: &Tab) {
        for space in &mut self.spaces.items {
            if self.spaces.global_tabs || space.id == self.spaces.active {
                space.deck.tabs.retain(|t| t != tab);
                if space.deck.selected.as_ref() == Some(tab) {
                    space.deck.selected = None;
                }
                if space.deck.preview.as_ref() == Some(tab) {
                    space.deck.preview = None;
                }
            }
        }
        self.spaces.global.tabs.retain(|t| t != tab);
    }
    pub(super) fn replace_space_tab(&mut self, old: &Tab, new: Tab) -> bool {
        let mut replaced = false;
        for deck in self
            .spaces
            .items
            .iter_mut()
            .map(|s| &mut s.deck)
            .chain(std::iter::once(&mut self.spaces.global))
        {
            for tab in &mut deck.tabs {
                if tab == old {
                    *tab = new.clone();
                    replaced = true;
                }
            }
            if deck.selected.as_ref() == Some(old) {
                deck.selected = Some(new.clone());
            }
            if deck.preview.as_ref() == Some(old) {
                deck.preview = Some(new.clone());
            }
        }
        if let Some(uid) = self.uids.remove(old) {
            self.uids.insert(new, uid);
        }
        replaced
    }
}

#[cfg(test)]
mod tests {
    use super::{Spaces, Swipe};
    use crate::state::Tab;
    use del_agent::SessionId;

    #[gpui::test]
    fn spaces_preserve_drafts_selection_and_project_membership(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.new_project_session("/project-one".into(), cx);
            let original = s.tabs.clone();
            let selection = s.selection.clone();
            let uid = s.uid_of(original.last().unwrap());
            s.create_space("  Work  ".into(), cx);
            let work = s.spaces.active.clone();
            assert_eq!(s.spaces.current().name, "Work");
            assert!(!s.space_has_project(std::path::Path::new("/project-one")));
            assert!(s.closed_terminals.is_empty());
            assert!(original.iter().all(|t| s.all_open_tabs().contains(t)));
            s.new_project_session("/project-two".into(), cx);
            let work_tabs = s.tabs.clone();
            s.switch_space("default", cx);
            assert_eq!(s.tabs, original);
            assert_eq!(s.selection, selection);
            assert_eq!(s.uid_of(original.last().unwrap()), uid);
            assert!(!s.space_has_project(std::path::Path::new("/project-two")));
            s.switch_space(&work, cx);
            assert_eq!(s.tabs, work_tabs);
            s.rename_space(&work, "Studio".into(), cx);
            assert_eq!(s.spaces.current().name, "Studio");
            s.rename_space(&work, "   ".into(), cx);
            assert_eq!(s.spaces.current().name, "Studio");
        });
    }

    #[gpui::test]
    fn global_tabs_switch_without_changing_selection_and_closed_tabs_stay_closed(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            let first = s.tabs[0].clone();
            s.create_space("Work".into(), cx);
            let work = s.spaces.active.clone();
            let selected = s.selection.clone();
            s.set_global_space_tabs(true, cx);
            assert!(s.tabs.contains(&first));
            let global = s.tabs.clone();
            s.switch_space("default", cx);
            assert_eq!(s.tabs, global);
            assert_eq!(s.selection, selected);
            s.close_tab(&first, cx);
            assert!(!s.all_open_tabs().contains(&first));
            s.switch_space(&work, cx);
            assert!(!s.tabs.contains(&first));
            s.set_global_space_tabs(false, cx);
            assert!(!s.spaces.global_tabs);
        });
    }

    #[gpui::test]
    fn changing_global_tabs_keeps_settings_open(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.create_space("Work".into(), cx);
            s.panel = crate::state::Panel::Providers;
            s.set_global_space_tabs(true, cx);
            assert_eq!(s.panel, crate::state::Panel::Providers);
            s.set_global_space_tabs(false, cx);
            assert_eq!(s.panel, crate::state::Panel::Providers);
            assert!(s.settings_section == crate::signin::SettingsSection::Appearance);
        });
    }

    #[gpui::test]
    fn pins_are_independent_and_spaces_round_trip(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            let id = SessionId("example".into());
            assert!(s.space_pinned(&id, true));
            s.create_space("Work".into(), cx);
            assert!(!s.space_pinned(&id, true));
            s.toggle_space_pin(&id, false, cx);
            assert!(s.space_pinned(&id, false));
            s.switch_space("default", cx);
            s.toggle_space_pin(&id, true, cx);
            assert!(!s.space_pinned(&id, true));
            s.checkpoint_space();
            let mut restored: Spaces =
                serde_json::from_slice(&serde_json::to_vec(&s.spaces).unwrap()).unwrap();
            restored.normalize();
            assert_eq!(restored.active, "default");
            assert_eq!(restored.current().deck.tabs, s.tabs);
            assert_eq!(restored.items[1].pins.get("example"), Some(&true));
        });
    }

    #[gpui::test]
    fn background_start_replaces_inactive_draft_without_switching_space(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            let draft = s.tabs[0].clone();
            let uid = s.uid_of(&draft);
            s.create_space("Work".into(), cx);
            let selected = s.selection.clone();
            let session = Tab::Session(SessionId("started-in-background".into()));
            assert!(s.replace_space_tab(&draft, session.clone()));
            assert_eq!(s.spaces.items[0].deck.tabs, vec![session.clone()]);
            assert_eq!(s.uid_of(&session), uid);
            assert_eq!(s.selection, selected);
            s.open_background_tab(&session, cx);
            assert!(!s.tabs.contains(&session));
        });
    }

    #[gpui::test]
    fn deleting_a_space_keeps_projects_pins_and_open_tabs(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.delete_space("default", cx);
            assert_eq!(s.spaces.items.len(), 1);
            s.new_project_session("/one".into(), cx);
            s.create_space("Work".into(), cx);
            let work = s.spaces.active.clone();
            s.new_project_session("/two".into(), cx);
            let tabs = s.all_open_tabs();
            let pinned = SessionId("keep-pin".into());
            s.toggle_space_pin(&pinned, false, cx);
            s.switch_space("default", cx);
            s.delete_space(&work, cx);
            assert_eq!(s.spaces.items.len(), 1);
            assert!(tabs.iter().all(|t| s.tabs.contains(t)));
            assert!(s.space_has_project(std::path::Path::new("/two")));
            assert!(s.space_pinned(&pinned, false));
            s.create_space("Replacement".into(), cx);
            let replacement = s.spaces.active.clone();
            s.delete_space("default", cx);
            assert_eq!(s.spaces.active, replacement);
            assert!(tabs.iter().all(|t| s.all_open_tabs().contains(t)));
            assert!(s.space_has_project(std::path::Path::new("/unassigned")));
            assert!(s.closed_terminals.is_empty());
            let restored: Spaces =
                serde_json::from_str(&serde_json::to_string(&s.spaces).unwrap()).unwrap();
            assert_eq!(restored.current().name, "Replacement");
        });
    }

    #[gpui::test]
    fn moving_project_merges_tabs_pins_and_preserves_live_sessions(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            let root = std::path::PathBuf::from("/shared-project");
            s.new_project_session(root.clone(), cx);
            let draft = s.tabs.last().unwrap().clone();
            let uid = s.uid_of(&draft);
            let terminal = SessionId("running-terminal".into());
            s.terminal_rows.push(crate::sessions::TerminalRow {
                id: terminal.clone(),
                title: "Shell".into(),
                cwd: root.clone(),
                account_id: None,
                provider: None,
                active: true,
                summary: String::new(),
                pinned: true,
            });
            s.tabs.push(Tab::Terminal(terminal.clone()));
            s.spaces.current_mut().order.push(terminal.clone());
            s.create_space("Work".into(), cx);
            let destination = s.spaces.active.clone();
            s.spaces.current_mut().projects.insert(root.clone());
            // The destination already contains the project and one of its tabs.
            s.tabs.push(draft.clone());
            s.spaces
                .current_mut()
                .pins
                .insert(terminal.0.clone(), false);
            s.switch_space("default", cx);
            s.move_project_to_space(&root, &destination, cx);
            assert!(!s.space_has_project(&root));
            assert!(!s.tabs.contains(&draft));
            assert!(s.space_rows().iter().all(|r| r.id != terminal));
            assert_eq!(s.uid_of(&draft), uid);
            assert!(s.closed_terminals.is_empty());
            assert!(s.terminal_rows[0].active);
            s.switch_space(&destination, cx);
            assert_eq!(s.tabs.iter().filter(|t| **t == draft).count(), 1);
            assert!(s.tabs.contains(&Tab::Terminal(terminal.clone())));
            assert!(s.space_pinned(&terminal, false));
            assert_eq!(s.spaces.current().order, vec![terminal]);
            assert_eq!(
                s.spaces
                    .current()
                    .projects
                    .iter()
                    .filter(|p| **p == root)
                    .count(),
                1
            );
            s.checkpoint_space();
            let saved: Spaces =
                serde_json::from_str(&serde_json::to_string(&s.spaces).unwrap()).unwrap();
            assert!(saved.current().deck.tabs.contains(&draft));
        });
    }

    #[gpui::test]
    fn merging_project_preserves_destination_legacy_pins(cx: &mut gpui::TestAppContext) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            let root = std::path::PathBuf::from("/shared-project");
            let id = SessionId("pinned-in-default".into());
            s.terminal_rows.push(crate::sessions::TerminalRow {
                id: id.clone(),
                title: "Shell".into(),
                cwd: root.clone(),
                account_id: None,
                provider: None,
                active: true,
                summary: String::new(),
                pinned: true,
            });
            s.create_space("Work".into(), cx);
            s.spaces.current_mut().projects.insert(root.clone());
            s.spaces.current_mut().pins.insert(id.0.clone(), false);
            s.move_project_to_space(&root, "default", cx);
            s.switch_space("default", cx);
            assert!(s.space_pinned(&id, false));
        });
    }

    #[gpui::test]
    fn moving_project_with_global_tabs_keeps_them_open_and_assigns_new_tabs(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.create_space("Work".into(), cx);
            let destination = s.spaces.active.clone();
            s.switch_space("default", cx);
            s.set_global_space_tabs(true, cx);
            let root = std::path::PathBuf::from("/new-global-project");
            s.new_project_session(root.clone(), cx);
            let draft = s.tabs.last().unwrap().clone();
            let tabs = s.tabs.clone();
            let selected = s.selection.clone();
            s.move_project_to_space(&root, "missing", cx);
            assert!(s.space_has_project(&root));
            s.move_project_to_space(&root, &destination, cx);
            assert_eq!(s.tabs, tabs);
            assert_eq!(s.selection, selected);
            assert!(!s.space_has_project(&root));
            s.set_global_space_tabs(false, cx);
            assert!(!s.tabs.contains(&draft));
            s.switch_space(&destination, cx);
            assert!(s.tabs.contains(&draft));
            s.move_project_to_space(&root, &destination, cx);
            assert_eq!(s.tabs.iter().filter(|t| **t == draft).count(), 1);
        });
    }

    #[test]
    fn swipe_tracks_fingers_then_settles_or_cancels() {
        use gpui::TouchPhase::*;
        let now = std::time::Instant::now();
        let mut swipe = Swipe::default();
        assert!(swipe.update((-90., 0.), Started, now, 300., None, Some("work".into())));
        assert_eq!(swipe.offset, -90.);
        assert!(!swipe.settling());
        swipe.update((0., 0.), Ended, now, 300., None, Some("work".into()));
        assert_eq!(
            swipe.advance(now + std::time::Duration::from_millis(100), false),
            None
        );
        assert!(swipe.offset < -90. && swipe.offset > -300.);
        assert_eq!(
            swipe.advance(now + std::time::Duration::from_millis(230), false),
            Some("work".into())
        );
        swipe.update((-30., 0.), Started, now, 300., None, Some("work".into()));
        swipe.update((0., 0.), Ended, now, 300., None, Some("work".into()));
        assert_eq!(swipe.advance(now, true), None);
        assert_eq!(swipe.offset, 0.);
        swipe.update((-150., 0.), Started, now, 300., None, Some("work".into()));
        swipe.update((0., 0.), Cancelled, now, 300., None, Some("work".into()));
        assert_eq!(swipe.advance(now, true), None);
        assert!(!swipe.update((0., -90.), Started, now, 300., None, Some("work".into())));
        assert_eq!(swipe.offset, 0.);
        swipe.update((100., 0.), Started, now, 300., None, Some("work".into()));
        assert!(swipe.offset > 0. && swipe.offset < 30.);
        assert!(swipe.target.is_none());
    }
    #[test]
    fn invalid_saved_spaces_recover_to_a_valid_collection() {
        let mut spaces: Spaces =
            serde_json::from_str(r#"{"items":[],"active":"missing"}"#).unwrap();
        spaces.normalize();
        assert_eq!(spaces.current().name, "Default");
        assert!(!spaces.global_tabs);
    }
}
