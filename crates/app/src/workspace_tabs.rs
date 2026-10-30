//! Compatibility loader for tools saved in the former main-area tab strip.
//! The shell migrates these live panels into the right sidebar before rendering.
use crate::new_tab::destination;
use crate::right_panel::{Pane, RightPanel};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Kind {
    Blank,
    Files,
    Changes,
    Terminal,
    Browser,
    SideChat,
}
impl Kind {
    #[cfg(test)]
    fn from_pane(pane: Pane) -> Self {
        match pane {
            Pane::Files => Self::Files,
            Pane::Changes => Self::Changes,
            Pane::Terminal => Self::Terminal,
            Pane::Browser => Self::Browser,
            Pane::SideChat => Self::SideChat,
            Pane::NewTab => Self::Blank,
        }
    }
}
struct ToolTab {
    id: u64,
    kind: Kind,
    panel: Option<Entity<RightPanel>>,
    browser: Option<crate::browser::Browser>,
    draft: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedSideTabs {
    #[serde(default)]
    tabs: Option<Vec<SavedToolTab>>,
    #[serde(default)]
    ids: Vec<String>,
    active: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedToolTab {
    kind: Kind,
    panel: Option<crate::right_panel::SavedPanel>,
    url: Option<String>,
    profile: String,
    draft: String,
}

pub struct WorkspaceTabs {
    watched_panels: std::collections::HashSet<EntityId>,
    side_context: Option<crate::side_chat::Context>,
    storage_key: Option<String>,
    root: PathBuf,
    tabs: Vec<ToolTab>,
    active: Option<u64>,
    next_id: u64,
    visible: bool,
    address: Entity<InputState>,
    profile: String,
    scroll: ScrollHandle,
}
impl EventEmitter<crate::side_chat::SendToMain> for WorkspaceTabs {}
impl EventEmitter<crate::chat::OpenLink> for WorkspaceTabs {}
impl EventEmitter<crate::chat::OpenPeer> for WorkspaceTabs {}
impl WorkspaceTabs {
    pub fn set_side_context(&mut self, context: crate::side_chat::Context) {
        self.side_context = Some(context);
    }

    fn storage_path(key: &str) -> Option<PathBuf> {
        #[cfg(test)]
        {
            let _ = key;
            None
        }
        #[cfg(not(test))]
        {
            let name: String = key.bytes().map(|b| format!("{b:02x}")).collect();
            Some(
                del_agent::data_dir()?
                    .join("side-chat-tabs")
                    .join(format!("{name}.json")),
            )
        }
    }
    pub fn save_side_chats(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |panel, cx| panel.save_side_chat(cx));
            }
        }
        self.persist(cx);
    }

    pub fn persist(&self, cx: &App) {
        if let Some(path) = self.storage_key.as_deref().and_then(Self::storage_path) {
            let saved = self.snapshot(cx);
            if let Ok(bytes) = serde_json::to_vec(&saved)
                && let Err(error) =
                    crate::sessions::write_atomic(&path, &bytes, crate::sessions::PRIVATE)
            {
                eprintln!("Could not save side chat tabs: {error}");
            }
        }
    }
    pub fn set_storage_key(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_key.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = self.storage_key.replace(key.clone()) {
            // Draft -> saved session: move the existing strip, don't reopen it.
            self.persist(cx);
            if let Some(path) = Self::storage_path(&old) {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
        if let Some(path) = Self::storage_path(&key) {
            let saved: SavedSideTabs = match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(saved) => saved,
                    Err(error) => {
                        eprintln!("Could not read side chat tabs: {error}");
                        return;
                    }
                },
                Err(_) => return,
            };
            self.restore_tabs(saved, window, cx);
        }
    }

    pub(crate) fn snapshot(&self, cx: &App) -> SavedSideTabs {
        SavedSideTabs {
            ids: Vec::new(),
            active: self.tabs.iter().position(|tab| Some(tab.id) == self.active),
            tabs: Some(
                self.tabs
                    .iter()
                    .map(|tab| SavedToolTab {
                        kind: tab.kind,
                        panel: tab
                            .panel
                            .as_ref()
                            .map(|panel| panel.read(cx).snapshot(false, cx)),
                        url: tab.browser.as_ref().map(|browser| browser.url()),
                        profile: self.profile.clone(),
                        draft: if Some(tab.id) == self.active {
                            self.address.read(cx).value().to_string()
                        } else {
                            tab.draft.clone()
                        },
                    })
                    .collect(),
            ),
        }
    }

    fn restore_tabs(&mut self, saved: SavedSideTabs, window: &mut Window, cx: &mut Context<Self>) {
        let mut tabs = Vec::new();
        let mut active_index = saved.active;
        if let Some(saved_tabs) = saved.tabs {
            let mut expanded = Vec::new();
            for (index, mut tab) in saved_tabs.into_iter().enumerate() {
                if saved.active == Some(index) {
                    active_index = Some(expanded.len());
                }
                if let Some(panel) = tab.panel.take() {
                    for panel in panel.split_browser_pages() {
                        expanded.push(SavedToolTab {
                            kind: tab.kind,
                            panel: Some(panel),
                            url: None,
                            profile: tab.profile.clone(),
                            draft: tab.draft.clone(),
                        });
                    }
                } else {
                    expanded.push(tab);
                }
            }
            for saved in expanded {
                self.profile = saved.profile.clone();
                let panel = saved.panel.map(|saved| {
                    cx.new(|cx| {
                        let mut panel = RightPanel::new(self.root.clone(), window, cx);
                        if let Some(context) = self.side_context.clone() {
                            panel.set_side_context(context);
                        }
                        panel.restore_workspace(saved, window, cx);
                        panel.set_tab_zone(false);
                        panel
                    })
                });
                if let Some(panel) = &panel {
                    self.watch_panel(panel, cx);
                }
                let browser = saved
                    .url
                    .map(|url| crate::browser::Browser::new(&url, &saved.profile));
                let id = self.next_id;
                self.next_id += 1;
                tabs.push(ToolTab {
                    id,
                    kind: saved.kind,
                    panel,
                    browser,
                    draft: saved.draft,
                });
            }
        } else {
            // Migrate the earlier side-chat-only manifest without losing any chats.
            for id in saved.ids {
                let Some(context) = self.side_context.clone() else {
                    break;
                };
                let panel = cx.new(|cx| {
                    let mut panel = RightPanel::new(self.root.clone(), window, cx);
                    panel.set_side_context(context);
                    panel.restore_side_chat(id, window, cx);
                    panel
                });
                self.watch_panel(&panel, cx);
                let id = self.next_id;
                self.next_id += 1;
                tabs.push(ToolTab {
                    id,
                    kind: Kind::SideChat,
                    panel: Some(panel),
                    browser: None,
                    draft: String::new(),
                });
            }
        }
        let active = active_index
            .and_then(|index| tabs.get(index))
            .map(|tab| tab.id);
        self.tabs.extend(tabs);
        self.select(active, window, cx);
    }

    fn watch_panel(&mut self, panel: &Entity<RightPanel>, cx: &mut Context<Self>) {
        if !self.watched_panels.insert(panel.entity_id()) {
            return;
        }
        cx.observe(panel, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            panel,
            |this, panel, event: &crate::side_chat::SendToMain, cx| {
                if this
                    .tabs
                    .iter()
                    .any(|tab| tab.panel.as_ref() == Some(&panel))
                {
                    cx.emit(event.clone());
                }
            },
        )
        .detach();
        cx.subscribe(panel, |this, panel, event: &crate::chat::OpenLink, cx| {
            if this
                .tabs
                .iter()
                .any(|tab| tab.panel.as_ref() == Some(&panel))
            {
                cx.emit(crate::chat::OpenLink(event.0.clone()));
            }
        })
        .detach();
        cx.subscribe(panel, |this, panel, event: &crate::chat::OpenPeer, cx| {
            if this
                .tabs
                .iter()
                .any(|tab| tab.panel.as_ref() == Some(&panel))
            {
                cx.emit(crate::chat::OpenPeer(event.0.clone()));
            }
        })
        .detach();
    }

    /// One-time migration of the old main-area tools into the sidebar. Stable
    /// import IDs make retrying after an interrupted save safe.
    pub(crate) fn migrate_to_sidebar(
        &mut self,
        right: &Entity<RightPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        self.set_visible(false, cx);
        let ids: Vec<_> = self.tabs.iter().map(|tab| tab.id).collect();
        for id in ids {
            let key = format!("legacy:{}:{id}", self.storage_key.as_deref().unwrap_or(""));
            if let Some(panel) = self.take_tab(id, window, cx) {
                right.update(cx, |right, cx| {
                    right.import_legacy_tab(key, panel, window, cx)
                });
            }
        }
        self.active = None;
        true
    }

    pub(crate) fn take_tab(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<RightPanel>> {
        let index = self.tabs.iter().position(|tab| tab.id == id)?;
        let mut tab = self.tabs.remove(index);
        if self.active == Some(id) {
            tab.draft = self.address.read(cx).value().to_string();
        }
        let panel = tab.panel.unwrap_or_else(|| {
            cx.new(|cx| {
                let mut panel = RightPanel::new(self.root.clone(), window, cx);
                if let Some(context) = self.side_context.clone() {
                    panel.set_side_context(context);
                }
                if let Some(browser) = tab.browser {
                    panel.adopt_browser(browser, self.profile.clone());
                } else {
                    panel.blank_workspace(tab.draft, window, cx);
                }
                panel
            })
        });
        if self.active == Some(id) {
            self.active = None;
            self.select(
                index
                    .checked_sub(1)
                    .and_then(|index| self.tabs.get(index))
                    .map(|tab| tab.id),
                window,
                cx,
            );
        }
        cx.notify();
        Some(panel)
    }

    #[cfg(test)]
    pub(crate) fn accept_tab(
        &mut self,
        panel: Entity<RightPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        panel.update(cx, |panel, _| panel.set_tab_zone(false));
        self.watch_panel(&panel, cx);
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(ToolTab {
            id,
            kind: Kind::from_pane(panel.read(cx).current_pane()),
            panel: Some(panel),
            browser: None,
            draft: String::new(),
        });
        self.select(Some(id), window, cx);
    }

    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter a URL"));
        cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.navigate(window, cx);
            } else if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| Some(tab.id) == this.active) {
                    tab.draft = this.address.read(cx).value().to_string();
                }
                cx.notify();
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.persist(cx);
            async {}
        })
        .detach();
        Self {
            watched_panels: Default::default(),
            side_context: None,
            storage_key: None,
            root,
            tabs: vec![],
            active: None,
            next_id: 1,
            visible: true,
            address,
            profile: uuid::Uuid::new_v4().to_string(),
            scroll: ScrollHandle::new(),
        }
    }
    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.persist(cx);
        for tab in self.tabs.drain(..) {
            if let Some(panel) = tab.panel {
                panel.update(cx, |p, cx| p.release(cx));
            }
            if let Some(browser) = tab.browser {
                browser.hide();
            }
        }
        self.active = None;
    }

    pub(crate) fn browser_surface_visible(&self, cx: &App) -> bool {
        self.visible
            && self
                .tabs
                .iter()
                .find(|tab| self.active == Some(tab.id))
                .is_some_and(|tab| {
                    (tab.kind == Kind::Browser && tab.browser.is_some())
                        || tab
                            .panel
                            .as_ref()
                            .is_some_and(|panel| panel.read(cx).browser_surface_visible(cx))
                })
    }

    pub(crate) fn contains_peer(&self, id: &del_agent::SessionId, cx: &App) -> bool {
        self.tabs
            .iter()
            .filter_map(|t| t.panel.as_ref())
            .any(|panel| panel.read(cx).contains_peer(id, cx))
    }

    pub(crate) fn show_peer(
        &mut self,
        id: &del_agent::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.iter().find(|tab| {
            tab.panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).contains_peer(id, cx))
        }) else {
            return false;
        };
        let key = tab.id;
        if let Some(panel) = &tab.panel {
            panel.update(cx, |panel, cx| panel.show_peer(id, window, cx));
        }
        self.select(Some(key), window, cx);
        true
    }

    pub(crate) fn find_browser_panel(&self, id: &str, cx: &App) -> Option<Entity<RightPanel>> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.panel.as_ref())
            .find_map(|panel| RightPanel::find_browser_panel(panel, id, cx))
    }

    pub(crate) fn agent_browser_tabs(&self, cx: &App) -> Vec<crate::browser::SavedTab> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.panel.as_ref())
            .flat_map(|panel| panel.read(cx).agent_browser_tabs(cx))
            .collect()
    }

    pub(crate) fn close_browser_id(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab = self
            .tabs
            .iter()
            .find(|tab| {
                tab.panel
                    .as_ref()
                    .is_some_and(|panel| RightPanel::find_browser_panel(panel, id, cx).is_some())
            })
            .map(|tab| tab.id);
        if let Some(tab) = tab {
            self.close(tab, window, cx);
            return true;
        }
        false
    }

    pub(crate) fn take_browser_tab(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<RightPanel>> {
        let tab = self.tabs.iter().find(|tab| {
            tab.panel
                .as_ref()
                .is_some_and(|panel| RightPanel::find_browser_panel(panel, id, cx).is_some())
        })?;
        self.take_tab(tab.id, window, cx)
    }

    pub fn is_chat(&self) -> bool {
        self.active.is_none()
    }
    pub fn set_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root.clone();
        for tab in &self.tabs {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |p, cx| p.set_root(root.clone(), window, cx));
            }
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        for tab in &self.tabs {
            let shown = visible && self.active == Some(tab.id);
            if let Some(panel) = &tab.panel {
                panel.update(cx, |p, cx| p.set_preview_visible(shown, cx));
            }
            if !shown && let Some(browser) = &tab.browser {
                browser.hide();
            }
        }
    }
    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(ToolTab {
            id,
            kind: Kind::Blank,
            panel: None,
            browser: None,
            draft: String::new(),
        });
        self.select(Some(id), window, cx);
    }
    pub fn select_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == 0 {
            self.select(None, window, cx);
        } else if let Some(tab) = self.tabs.get(index - 1) {
            self.select(Some(tab.id), window, cx);
        }
    }
    #[cfg(test)]
    pub fn select_relative(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .map_or(0, |i| i + 1);
        let next =
            (current as isize + direction).rem_euclid((self.tabs.len() + 1) as isize) as usize;
        self.select_index(next, window, cx);
    }
    fn select(&mut self, id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| Some(tab.id) == self.active) {
            tab.draft = self.address.read(cx).value().to_string();
        }
        self.active = id;
        self.set_visible(self.visible, cx);
        let draft = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == id)
            .map(|tab| tab.draft.clone())
            .unwrap_or_default();
        self.address
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        if let Some(panel) = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == id)
            .and_then(|tab| tab.panel.clone())
        {
            panel.update(cx, |panel, cx| panel.prepare_to_show(window, cx));
        }
        self.focus(window, cx);
        cx.notify();
    }
    pub fn open_composer_picker(
        &self,
        picker: crate::keybinds::Picker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active)
            .and_then(|tab| tab.panel.as_ref())
        {
            panel.update(cx, |panel, cx| {
                panel.open_composer_picker(picker, window, cx)
            });
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|tab| Some(tab.id) == self.active) {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |panel, cx| panel.focus_workspace(window, cx));
                return;
            }
            if tab.kind != Kind::Browser {
                self.address.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }
    #[cfg(test)]
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.active else {
            return false;
        };
        self.close(id, window, cx);
        true
    }
    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        if self.tabs[index]
            .panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).has_unsaved_edits())
        {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Save or revert your file edits before closing this tab.",
                ),
                cx,
            );
            return;
        }
        let tab = self.tabs.remove(index);
        if let Some(panel) = tab.panel {
            panel.update(cx, |p, cx| {
                p.forget_side_chat(cx);
                p.release(cx);
            });
        }
        if let Some(browser) = tab.browser {
            browser.hide();
        }
        if self.active == Some(id) {
            let next = index
                .checked_sub(1)
                .and_then(|i| self.tabs.get(i))
                .map(|t| t.id);
            self.select(next, window, cx);
        }
        cx.notify();
    }
    fn choose(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == self.active) else {
            return;
        };
        let pane = match kind {
            Kind::Files => Pane::Files,
            Kind::Changes => Pane::Changes,
            Kind::Terminal => Pane::Terminal,
            Kind::SideChat => Pane::SideChat,
            _ => return,
        };
        let panel = cx.new(|cx| {
            let mut panel = RightPanel::new(self.root.clone(), window, cx);
            if let Some(context) = self.side_context.clone() {
                panel.set_side_context(context);
            }
            panel.show_workspace(pane, window, cx);
            panel
        });
        // Event forwarding checks current ownership after a cross-zone move.
        let tab_id = tab.id;
        self.watch_panel(&panel, cx);
        let tab = self.tabs.iter_mut().find(|tab| tab.id == tab_id).unwrap();
        tab.kind = kind;
        tab.panel = Some(panel);
        self.focus(window, cx);
        cx.notify();
    }
    fn navigate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = destination(self.address.read(cx).value().as_ref()) else {
            return;
        };
        // Keep a file editor or terminal alive when its address bar opens a site.
        if self
            .tabs
            .iter()
            .any(|t| Some(t.id) == self.active && t.kind != Kind::Blank)
        {
            self.new_tab(window, cx);
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == self.active) else {
            return;
        };
        tab.kind = Kind::Browser;
        tab.browser = Some(crate::browser::Browser::new(&url, &self.profile));
        cx.notify();
    }
    fn choose_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match pane {
            Pane::Files => Kind::Files,
            Pane::Changes => Kind::Changes,
            Pane::Terminal => Kind::Terminal,
            Pane::SideChat => Kind::SideChat,
            _ => return,
        };
        self.choose(kind, window, cx);
    }
}
impl Render for WorkspaceTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tabs.iter().find(|t| Some(t.id) == self.active);
        if let Some(browser) = tab.and_then(|t| t.browser.as_ref()) {
            return div()
                .size_full()
                .when(self.visible, |view| view.child(browser.element()))
                .into_any_element();
        }
        if let Some(panel) = tab.and_then(|tab| tab.panel.clone()) {
            return div().size_full().child(panel).into_any_element();
        }
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(crate::new_tab::address_bar(&self.address))
            .child(crate::new_tab::tools(
                false,
                &self.scroll,
                Self::choose_tool,
                cx,
            ))
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    #[::gpui::test]
    fn legacy_tools_migrate_once_with_identity_drafts_and_restart_state(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            let right = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
            workspace.update(cx, |workspace, cx| {
                workspace.storage_key = Some("chat:migration-test".into());
                workspace.new_tab(window, cx);
                workspace.choose(Kind::Files, window, cx);
                let files = workspace.tabs[0].panel.clone().unwrap();
                workspace.new_tab(window, cx);
                workspace.address.update(cx, |input, cx| input.set_value("saved draft", window, cx));
                let legacy = serde_json::to_vec(&workspace.snapshot(cx)).unwrap();
                assert!(workspace.migrate_to_sidebar(&right, window, cx));
                assert!(workspace.is_chat());
                assert!(workspace.tabs.is_empty());
                assert!(!workspace.migrate_to_sidebar(&right, window, cx));
                let saved = right.read(cx).snapshot(true, cx);
                let json = serde_json::to_value(&saved).unwrap();
                assert_eq!(json["hosted"].as_array().unwrap().len(), 2);
                assert_eq!(json["hosted"][1]["panel"]["blank_tabs"][0]["address"], "saved draft");
                // Retry a source manifest left behind by an interrupted migration.
                workspace.next_id = 1;
                workspace.restore_tabs(serde_json::from_slice(&legacy).unwrap(), window, cx);
                assert!(workspace.migrate_to_sidebar(&right, window, cx));
                assert_eq!(serde_json::to_value(right.read(cx).snapshot(true, cx)).unwrap()["hosted"].as_array().unwrap().len(), 2);
                right.update(cx, |right, cx| right.restore(saved, window, cx));
                assert_eq!(serde_json::to_value(right.read(cx).snapshot(true, cx)).unwrap()["hosted"][1]["panel"]["blank_tabs"][0]["address"], "saved draft");
                assert_eq!(files.read(cx).current_pane(), Pane::Files);
            });
        });
    }

    #[::core::prelude::v1::test]
    fn address_accepts_urls_localhost_and_searches() {
        assert_eq!(
            destination(" example.com ").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            destination("localhost:8766").as_deref(),
            Some("http://localhost:8766/")
        );
        assert_eq!(
            destination("http://127.0.0.1:8766/a").as_deref(),
            Some("http://127.0.0.1:8766/a")
        );
        assert!(destination("two words").unwrap().contains("q=two+words"));
        assert!(destination("file:///tmp/private").is_none());
        assert!(destination("  ").is_none());
    }

    #[::gpui::test]
    fn session_tabs_keep_their_tools_and_close_back_to_chat(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            let other = cx.new(|cx| WorkspaceTabs::new(dir.path().into(), window, cx));
            workspace.update(cx, |w, cx| {
                assert!(w.is_chat());
                w.new_tab(window, cx);
                let first = w.active;
                w.choose(Kind::Files, window, cx);
                let files = w.tabs[0].panel.clone().unwrap();
                w.new_tab(window, cx);
                assert_eq!(w.tabs.len(), 2);
                w.choose(Kind::Changes, window, cx);
                w.select_index(1, window, cx);
                assert_eq!(w.active, first);
                assert_eq!(w.tabs[0].panel.as_ref(), Some(&files));
                w.set_visible(false, cx);
                w.set_visible(true, cx);
                assert_eq!(w.active, first);
                assert!(other.read(cx).is_chat());
                assert!(other.read(cx).tabs.is_empty());
                w.select_relative(-1, window, cx);
                assert!(w.is_chat());
                w.select_relative(-1, window, cx);
                assert_eq!(w.active, Some(w.tabs[1].id));
                assert!(w.close_active(window, cx));
                assert_eq!(w.active, first);
                assert!(w.close_active(window, cx));
                assert!(w.is_chat());
                assert!(!w.close_active(window, cx));
            });
        });
    }

    #[::gpui::test]
    fn url_from_a_tool_keeps_the_original_tool_open(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.new_tab(window, cx);
                w.choose(Kind::Files, window, cx);
                w.address.update(cx, |input, cx| {
                    input.set_value("http://localhost:8766", window, cx)
                });
                w.navigate(window, cx);
                assert_eq!(w.tabs.len(), 2);
                assert_eq!(w.tabs[0].kind, Kind::Files);
                assert!(w.tabs[0].panel.is_some());
                assert_eq!(w.tabs[1].kind, Kind::Browser);
                assert_eq!(
                    w.tabs[1].browser.as_ref().unwrap().url(),
                    "http://localhost:8766/"
                );
                w.close_active(window, cx);
                assert_eq!(w.active, Some(w.tabs[0].id));
            })
        });
    }
}
