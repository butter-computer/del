//! The right sidebar: files, a workspace PTY, and an embedded browser.

use crate::appearance::{ActionTone, rgb, rgba};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, InputState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const PANEL_WIDTH: f32 = 340.0;
const MONO: &str = "Menlo";
/// Largest text file loaded into the editor.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Changed files whose diffs a refresh loads; the rest are only counted.
const MAX_REVIEW_FILES: usize = 500;
/// Diff rows rendered for one expanded file.
const MAX_REVIEW_LINES: usize = 2000;

/// A file's diff parsed once per load, not on every render.
struct ChangeReview {
    lines: Vec<crate::workspace_files::ReviewLine>,
    added: usize,
    removed: usize,
}

impl ChangeReview {
    fn parse(text: &str) -> Self {
        let lines = crate::workspace_files::review_lines(text);
        let added = lines.iter().filter(|line| line.kind == '+').count();
        let removed = lines.iter().filter(|line| line.kind == '-').count();
        Self {
            lines,
            added,
            removed,
        }
    }
}
/// Generated dependency/build trees stay out of the lightweight file viewer.
const NEVER_LIST: &[&str] = &["target", "node_modules"];
const DEFAULT_TREE_HEIGHT: f32 = 220.0;
const MIN_SPLIT_HEIGHT: f32 = 72.0;

#[derive(Clone)]
struct FileSplitDrag {
    start_y: Rc<Cell<f32>>,
    start_height: Rc<Cell<f32>>,
}

struct FileSplitPreview;

impl Render for FileSplitPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_0().h_0()
    }
}

fn visible_name(name: &str, show_hidden: bool) -> bool {
    !NEVER_LIST.contains(&name) && (show_hidden || !name.starts_with('.'))
}

fn resized_tree_height(start: f32, delta: f32, available: f32) -> f32 {
    let max = (available - MIN_SPLIT_HEIGHT).max(MIN_SPLIT_HEIGHT);
    (start + delta).clamp(MIN_SPLIT_HEIGHT, max)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pane {
    #[default]
    Files,
    Terminal,
    Browser,
    Changes,
    NewTab,
    SideChat,
}

impl Pane {
    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::Files => IconName::Folder.into(),
            Self::Changes => Icon::default().path("icons/file-diff.svg"),
            Self::Terminal => Icon::default().path("icons/terminal.svg"),
            Self::Browser => IconName::Globe.into(),
            Self::SideChat => Icon::default().path("icons/side-chat.svg"),
            Self::NewTab => IconName::File.into(),
        }
    }

    fn tab_label(self) -> (&'static str, &'static str) {
        match self {
            Self::Files => ("tab-files", "Files"),
            Self::Changes => ("tab-changes", "Changes"),
            Self::Terminal => ("tab-terminal", "Terminal"),
            Self::Browser => ("tab-browser", "Browser"),
            Self::SideChat => ("tab-side-chat", "Side chat"),
            Self::NewTab => ("tab-new", "New tab"),
        }
    }
}

fn default_panes() -> Vec<Pane> {
    vec![Pane::NewTab]
}

/// Visual order is independent of the backing tool instances and selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SidebarTab {
    Tool(Pane),
    Terminal(String),
    Browser(String),
    Blank(String),
    Hosted(String),
}

#[derive(Clone)]
pub(crate) struct DraggedSidebarTab {
    owner: EntityId,
    pub key: SidebarTab,
    pane: Pane,
    origin: Rc<Cell<Point<Pixels>>>,
    offset: Point<Pixels>,
    cursor_offset: Point<Pixels>,
}

impl Render for DraggedSidebarTab {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let natural = window.mouse_position() - self.offset;
        let source_y = self.origin.get().y - self.offset.y;
        let rect = Bounds::new(
            point(natural.x, source_y),
            size(
                zpx(crate::icons::TOOL_TAB_WIDTH),
                zpx(crate::icons::TOOL_TAB_HEIGHT),
            ),
        );
        let raw = window.mouse_position() - self.cursor_offset;
        // GPUI positions the drag root at the pointer. Offset its child to
        // the pointer while keeping a full-width, horizontally constrained preview.
        div()
            .w(rect.size.width)
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .child(
                h_flex()
                    .id("held-sidebar-tab")
                    .debug_selector(|| "held-sidebar-tab".into())
                    .relative()
                    .left(rect.left() - raw.x)
                    .top(rect.top() - raw.y)
                    .w(rect.size.width)
                    .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                    .px_1p5()
                    .gap_1p5()
                    .rounded_md()
                    .bg(crate::appearance::dragged_tab_surface())
                    .border_1()
                    .border_color(crate::appearance::separator())
                    .text_color(rgb(0xf5f5f5))
                    .text_size(zpx(14.))
                    .child(crate::icons::icon(self.pane.icon()).with_size(zpx(15.)))
                    .child(self.pane.tab_label().1),
            )
    }
}

struct SidebarDrag {
    key: SidebarTab,
    order: Vec<SidebarTab>,
    centers: Vec<Pixels>,
    pointer_offset: Pixels,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct SavedHostedTab {
    id: String,
    panel: SavedPanel,
}
struct HostedTab {
    id: String,
    panel: Entity<RightPanel>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct BlankTab {
    id: String,
    address: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPanel {
    pub open: bool,
    side_chat_id: Option<String>,
    pane: Pane,
    open_panes: Vec<Pane>,
    tab_order: Vec<SidebarTab>,
    hosted: Vec<SavedHostedTab>,
    active_hosted: Option<String>,
    #[serde(default)]
    blank_tabs: Vec<BlankTab>,
    blank_active: usize,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    show_hidden: bool,
    tree_height: f32,
    markdown_preview: bool,
    browser_profile: String,
    browser_tabs: Vec<crate::browser::SavedTab>,
    browser_active: usize,
    terminal_tabs: Vec<String>,
    terminal_active: usize,
    file_drafts: HashMap<PathBuf, crate::workspace_files::FileDraft>,
}

impl SavedPanel {
    pub fn restore_open(&self) -> bool {
        self.open && (self.active_hosted.is_some() || self.pane != Pane::Changes)
    }

    pub(crate) fn split_browser_pages(mut self) -> Vec<Self> {
        if self.pane != Pane::Browser || self.browser_tabs.len() <= 1 {
            return vec![self];
        }
        let active = self.browser_active.min(self.browser_tabs.len() - 1);
        let mut pages = std::mem::take(&mut self.browser_tabs);
        self.browser_tabs = vec![pages.remove(active)];
        self.browser_active = 0;
        self.tab_order.clear();
        let mut result = vec![self.clone()];
        for page in pages {
            let mut panel = self.clone();
            panel.browser_tabs = vec![page];
            result.push(panel);
        }
        result
    }

    pub fn browser_profile(&self) -> &str {
        &self.browser_profile
    }
}
impl Default for SavedPanel {
    fn default() -> Self {
        Self {
            open: true,
            side_chat_id: None,
            pane: Pane::NewTab,
            open_panes: default_panes(),
            tab_order: Vec::new(),
            hosted: Vec::new(),
            active_hosted: None,
            blank_tabs: vec![BlankTab {
                id: uuid::Uuid::new_v4().to_string(),
                address: String::new(),
            }],
            blank_active: 0,
            expanded: HashSet::new(),
            selected: None,
            show_hidden: false,
            tree_height: DEFAULT_TREE_HEIGHT,
            markdown_preview: false,
            browser_profile: uuid::Uuid::new_v4().to_string(),
            browser_tabs: vec![Default::default()],
            browser_active: 0,
            terminal_tabs: vec![uuid::Uuid::new_v4().to_string()],
            terminal_active: 0,
            file_drafts: HashMap::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct PanelStore(std::collections::HashMap<String, SavedPanel>);

impl PanelStore {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> SavedPanel {
        self.0.get(key).cloned().unwrap_or_default()
    }

    pub fn browser_tabs(&self, key: &str) -> Vec<crate::browser::SavedTab> {
        self.0
            .get(key)
            .map(|panel| {
                fn collect(panel: &SavedPanel, tabs: &mut Vec<crate::browser::SavedTab>) {
                    if panel.open_panes.contains(&Pane::Browser) {
                        tabs.extend(panel.browser_tabs.clone());
                    }
                    for tab in &panel.hosted {
                        collect(&tab.panel, tabs);
                    }
                }
                let mut tabs = Vec::new();
                collect(panel, &mut tabs);
                tabs
            })
            .unwrap_or_default()
    }
    pub fn remove(&mut self, key: &str) -> Option<SavedPanel> {
        self.0.remove(key)
    }

    pub fn set(&mut self, key: String, panel: SavedPanel) -> bool {
        if self.0.get(&key) == Some(&panel) {
            return false;
        }
        self.0.insert(key, panel);
        true
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

/// Run only on the background executor: cloud-backed folders can block in
/// read_dir or file_type even when no file content is being opened.
fn read_directory(dir: &Path, show_hidden: bool) -> Vec<(PathBuf, bool)> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| visible_name(&entry.file_name().to_string_lossy(), show_hidden))
        .map(|entry| {
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            (entry.path(), is_dir)
        })
        .collect();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
    });
    entries
}

/// Flatten only cached directories; an unavailable child never blocks siblings.
fn cached_tree_rows(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    cache: &HashMap<PathBuf, Vec<(PathBuf, bool)>>,
) -> Vec<(PathBuf, usize, bool)> {
    let mut rows = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0, true)];
    while let Some((path, depth, is_dir)) = pending.pop() {
        if depth > 0 {
            rows.push((path.clone(), depth - 1, is_dir));
            if !is_dir || !expanded.contains(&path) {
                continue;
            }
        }
        if let Some(entries) = cache.get(&path) {
            pending.extend(
                entries
                    .iter()
                    .rev()
                    .map(|(child, is_dir)| (child.clone(), depth + 1, *is_dir)),
            );
        }
    }
    rows
}

enum Viewer {
    None,
    Document {
        path: PathBuf,
    },
    Text {
        path: PathBuf,
        source: String,
        markdown: Option<SharedString>,
    },
    Loading {
        path: PathBuf,
    },
    Note {
        path: PathBuf,
        message: String,
    },
}

pub struct RightPanel {
    focus: FocusHandle,
    side_context: Option<crate::side_chat::Context>,
    side_chat: Option<Entity<crate::side_chat::SideChat>>,
    side_chat_id: Option<String>,
    workspace: bool,
    compact_tools: bool,
    pane: Pane,
    open_panes: Vec<Pane>,
    tab_order: Vec<SidebarTab>,
    hosted: Vec<HostedTab>,
    active_hosted: Option<String>,
    watched_hosted: HashSet<EntityId>,
    transfer_bounds: Rc<Cell<Bounds<Pixels>>>,
    blank_tabs: Vec<BlankTab>,
    blank_active: usize,
    address: Entity<InputState>,
    new_tab_scroll: ScrollHandle,
    tool_tab_scroll: ScrollHandle,
    revealed_tab: Option<SidebarTab>,
    motion: crate::tab_motion::Strip<SidebarTab>,
    tab_drag: Option<SidebarDrag>,
    tab_drag_origin: Rc<Cell<Point<Pixels>>>,
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    file_hover: crate::hover_motion::HoverMotion,
    viewer: Viewer,
    document_preview: Option<crate::document_preview::DocumentPreview>,
    preview_visible: bool,
    terminals: HashMap<String, Entity<TerminalHost>>,
    terminal_tabs: Vec<String>,
    terminal_active: usize,
    file_drafts: HashMap<PathBuf, crate::workspace_files::FileDraft>,
    editors: HashMap<PathBuf, Entity<EditorState>>,
    saving: HashSet<PathBuf>,
    file_error: Option<String>,
    change_files: Vec<crate::workspace_files::ChangedFile>,
    change_root: PathBuf,
    change_diffs: HashMap<PathBuf, String>,
    change_reviews: HashMap<PathBuf, Rc<ChangeReview>>,
    /// Changed files beyond MAX_REVIEW_FILES.
    change_omitted: usize,
    change_expanded: HashSet<PathBuf>,
    change_error: Option<String>,
    change_request: u64,
    changes_loading: bool,
    changes_watch: Option<Task<()>>,

    browser: Option<crate::browser::Browser>,
    browser_profile: String,
    browser_tabs: Vec<crate::browser::SavedTab>,
    browser_active: usize,
    browsers: HashMap<String, crate::browser::Browser>,
    browser_watch: Option<Task<()>>,
    terminal_error: Option<String>,
    show_hidden: bool,
    file_tree_height: f32,
    file_request: u64,
    tree_request: u64,
    directory_entries: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    split_origin: Rc<Cell<f32>>,
    split_start_height: Rc<Cell<f32>>,
    rows: Vec<(PathBuf, usize, bool)>,
    markdown_preview: bool,
    markdown_scroll: ScrollHandle,
}

impl EventEmitter<crate::side_chat::SendToMain> for RightPanel {}
impl EventEmitter<crate::chat::OpenLink> for RightPanel {}
impl EventEmitter<crate::chat::OpenPeer> for RightPanel {}

impl RightPanel {
    pub(crate) fn set_tab_zone(&mut self, sidebar: bool) {
        self.compact_tools = sidebar;
    }
    pub(crate) fn focus_workspace(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| Some(&tab.id) == self.active_hosted.as_ref())
        {
            tab.panel
                .update(cx, |panel, cx| panel.focus_workspace(window, cx));
            return;
        }
        self.focus.focus(window, cx);
        match self.pane {
            Pane::NewTab => self.address.update(cx, |input, cx| input.focus(window, cx)),
            Pane::Terminal => self.focus_terminal(window, cx),
            Pane::SideChat => self.focus_side_chat(window, cx),
            _ => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn current_pane(&self) -> Pane {
        self.pane
    }

    pub(crate) fn restore_workspace(
        &mut self,
        saved: SavedPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = saved.pane;
        self.workspace = true;
        self.restore(saved, window, cx);
        self.pane = pane;
    }

    pub(crate) fn blank_workspace(
        &mut self,
        draft: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace = true;
        self.open_panes.clear();
        self.blank_tabs.clear();
        self.open_new_tab(window, cx);
        self.address
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        self.remember_blank(cx);
    }

    pub(crate) fn adopt_browser(&mut self, browser: crate::browser::Browser, profile: String) {
        self.workspace = true;
        self.pane = Pane::Browser;
        self.open_panes = vec![Pane::Browser];
        self.browser_profile = profile;
        self.browser_tabs[0].url = browser.url();
        self.browsers
            .insert(self.browser_tabs[0].id.clone(), browser);
        self.browser = None;
    }

    fn watch_hosted(&mut self, panel: &Entity<RightPanel>, cx: &mut Context<Self>) {
        if !self.watched_hosted.insert(panel.entity_id()) {
            return;
        }
        cx.observe(panel, |_, _, cx| cx.notify()).detach();
        let source = panel.downgrade();
        cx.subscribe(
            panel,
            move |this, _, event: &crate::side_chat::SendToMain, cx| {
                if this
                    .hosted
                    .iter()
                    .any(|tab| tab.panel.entity_id() == source.entity_id())
                {
                    cx.emit(event.clone());
                }
            },
        )
        .detach();
        let source = panel.downgrade();
        cx.subscribe(panel, move |this, _, event: &crate::chat::OpenLink, cx| {
            if this
                .hosted
                .iter()
                .any(|tab| tab.panel.entity_id() == source.entity_id())
            {
                cx.emit(crate::chat::OpenLink(event.0.clone()));
            }
        })
        .detach();
        let source = panel.downgrade();
        cx.subscribe(panel, move |this, _, event: &crate::chat::OpenPeer, cx| {
            if this
                .hosted
                .iter()
                .any(|tab| tab.panel.entity_id() == source.entity_id())
            {
                cx.emit(crate::chat::OpenPeer(event.0.clone()));
            }
        })
        .detach();
    }

    pub(crate) fn accept_tab(
        &mut self,
        panel: Entity<RightPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion.settle();
        self.remember_blank(cx);
        panel.update(cx, |panel, _| panel.set_tab_zone(true));
        self.watch_hosted(&panel, cx);
        let id = uuid::Uuid::new_v4().to_string();
        self.hosted.push(HostedTab {
            id: id.clone(),
            panel,
        });
        self.tab_order = self.sidebar_keys();
        self.select_hosted(id, window, cx);
        self.tool_tab_scroll
            .scroll_to_item(self.sidebar_keys().len() - 1);
    }

    fn select_hosted(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        self.active_hosted = Some(id);
        self.reveal_active_tab();
        self.set_preview_visible(self.preview_visible, cx);
        self.prepare_to_show(window, cx);
        if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| Some(&tab.id) == self.active_hosted.as_ref())
        {
            tab.panel
                .update(cx, |panel, cx| panel.focus_workspace(window, cx));
        }
        cx.notify();
    }

    fn close_hosted(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.hosted.iter().position(|tab| tab.id == id) else {
            return;
        };
        if self.hosted[index].panel.read(cx).has_unsaved_edits() {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Save or revert your file edits before closing this tab.",
                ),
                cx,
            );
            return;
        }
        let tab = self.hosted.remove(index);
        tab.panel.update(cx, |panel, cx| {
            panel.forget_side_chat(cx);
            panel.release(cx);
        });
        self.after_hosted_removed(id, window, cx);
    }

    fn after_hosted_removed(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_hosted.as_deref() == Some(id) {
            self.active_hosted = None;
            if let Some(pane) = self.open_panes.first().copied() {
                self.open_tool(pane, window, cx);
            } else if let Some(tab) = self.hosted.first() {
                self.select_hosted(tab.id.clone(), window, cx);
            } else {
                self.open_new_tab(window, cx);
            }
        }
        self.tab_order = self.sidebar_keys();
        self.tab_drag = None;
        cx.notify();
    }

    /// Detach live resources. A move is not a close: never kill a PTY, forget a
    /// side chat, reload a browser, or discard an editor's undo history.
    #[cfg(test)]
    pub(crate) fn take_tab(
        &mut self,
        key: &SidebarTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<RightPanel>> {
        if let SidebarTab::Hosted(id) = key {
            let index = self.hosted.iter().position(|tab| &tab.id == id)?;
            let tab = self.hosted.remove(index);
            self.after_hosted_removed(id, window, cx);
            return Some(tab.panel);
        }
        let (pane, instance) = self
            .sidebar_tabs()
            .into_iter()
            .find(|(pane, instance)| self.tab_key(*pane, *instance) == *key)?;
        if pane == Pane::Files && !self.saving.is_empty() {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Finish saving this file before moving its tab.",
                ),
                cx,
            );
            return None;
        }
        self.remember_blank(cx);
        let mut saved = self.snapshot(false, cx);
        saved.hosted.clear();
        saved.active_hosted = None;
        saved.pane = pane;
        saved.open_panes = vec![pane];
        saved.tab_order.clear();
        saved.side_chat_id = if pane == Pane::SideChat {
            self.side_chat_id.take()
        } else {
            None
        };
        saved.selected = if pane == Pane::Files {
            saved.selected
        } else {
            None
        };
        if pane != Pane::Files {
            saved.file_drafts.clear();
        }
        saved.blank_tabs = if pane == Pane::NewTab {
            vec![self.blank_tabs[instance?].clone()]
        } else {
            Vec::new()
        };
        saved.blank_active = 0;
        saved.terminal_tabs = if pane == Pane::Terminal {
            vec![self.terminal_tabs[instance?].clone()]
        } else {
            vec![uuid::Uuid::new_v4().to_string()]
        };
        saved.terminal_active = 0;
        let terminal = if pane == Pane::Terminal {
            self.terminals.remove(&saved.terminal_tabs[0])
        } else {
            None
        };
        let side = if pane == Pane::SideChat {
            self.side_chat.take()
        } else {
            None
        };
        saved.browser_tabs = if pane == Pane::Browser {
            vec![saved.browser_tabs[instance?].clone()]
        } else {
            vec![Default::default()]
        };
        saved.browser_active = 0;
        let browsers = if pane == Pane::Browser {
            if let Some(browser) = self.browser.take() {
                browser.hide();
            }
            let id = saved.browser_tabs[0].id.clone();
            self.browsers
                .remove(&id)
                .map(|browser| (id, browser))
                .into_iter()
                .collect()
        } else {
            HashMap::new()
        };
        let editors = if pane == Pane::Files {
            std::mem::take(&mut self.editors)
        } else {
            HashMap::new()
        };
        let viewer = if pane == Pane::Files {
            Some(std::mem::replace(&mut self.viewer, Viewer::None))
        } else {
            None
        };
        let preview = if pane == Pane::Files {
            self.document_preview.take()
        } else {
            None
        };
        if pane == Pane::Files {
            self.file_request += 1;
            self.file_drafts.clear();
        }
        let context = self.side_context.clone();
        let root = self.root.clone();
        let panel = cx.new(|cx| {
            let mut panel = RightPanel::new(root, window, cx);
            panel.side_context = context;
            // Restore metadata without starting a second file read or process.
            let selected = saved.selected.take();
            panel.restore_workspace(saved, window, cx);
            if let Some(terminal) = terminal {
                panel
                    .terminals
                    .insert(panel.terminal_tabs[0].clone(), terminal);
            }
            panel.side_chat = side;
            if let Some(side) = panel.side_chat.clone() {
                panel.watch_side_chat(&side, cx);
            }
            panel.browsers = browsers;
            panel.document_preview = preview;
            if let Some(viewer) = viewer {
                if matches!(viewer, Viewer::Loading { .. }) {
                    if let Some(path) = selected {
                        panel.open_file(path, cx);
                    }
                } else {
                    panel.viewer = viewer;
                }
            }
            for (path, editor) in editors {
                panel.watch_editor(path, editor, cx);
            }
            panel
        });
        match (pane, instance) {
            (Pane::Terminal, Some(index)) => self.close_terminal_tab(index, window, cx),
            (Pane::Browser, Some(index)) => self.close_browser_tab(index, window, cx),
            (Pane::NewTab, Some(index)) => self.close_blank_tab(index, window, cx),
            _ => self.close_tool(pane, window, cx),
        }
        self.tab_drag = None;
        Some(panel)
    }

    pub fn set_side_context(&mut self, context: crate::side_chat::Context) {
        self.side_context = Some(context);
    }

    pub fn open_composer_picker(
        &self,
        picker: crate::keybinds::Picker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| Some(&tab.id) == self.active_hosted.as_ref())
        {
            tab.panel.update(cx, |panel, cx| {
                panel.open_composer_picker(picker, window, cx)
            });
            return;
        }
        if self.pane == Pane::SideChat
            && let Some(side) = &self.side_chat
        {
            side.update(cx, |side, cx| side.open_composer_picker(picker, window, cx));
        }
    }

    pub fn focus_side_chat(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(side) = &self.side_chat {
            side.update(cx, |side, cx| side.focus(window, cx));
        }
    }

    fn ensure_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.side_chat.is_none()
            && let Some(context) = self.side_context.clone()
        {
            let saved = self.side_chat_id.clone();
            let side = cx.new(|cx| match saved {
                Some(id) => crate::side_chat::SideChat::restore(context, id, window, cx),
                None => crate::side_chat::SideChat::new(context, window, cx),
            });
            self.side_chat_id = Some(side.read(cx).storage_id());
            self.watch_side_chat(&side, cx);
            self.side_chat = Some(side);
        }
        if let Some(side) = &self.side_chat {
            side.update(cx, |side, cx| side.focus(window, cx));
        }
    }

    pub(crate) fn is_unread(&self, cx: &App) -> bool {
        self.is_unread_with(None, cx)
    }

    fn is_unread_with(&self, fallback: Option<&crate::side_chat::Context>, cx: &App) -> bool {
        if let Some(side) = &self.side_chat {
            return side.read(cx).is_unread(cx);
        }
        let Some(id) = &self.side_chat_id else {
            return false;
        };
        self.side_context
            .as_ref()
            .or(fallback)
            .and_then(|context| context.activity_owner.as_ref())
            .and_then(|(state, owner)| {
                state
                    .upgrade()
                    .map(|state| state.read(cx).side_chat_unread(*owner, id))
            })
            .unwrap_or(false)
    }

    pub(crate) fn is_working(&self, cx: &App) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.read(cx).is_working(cx))
    }

    fn watch_side_chat(
        &mut self,
        side: &Entity<crate::side_chat::SideChat>,
        cx: &mut Context<Self>,
    ) {
        cx.observe(side, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            side,
            |this, side, event: &crate::side_chat::SendToMain, cx| {
                if this.side_chat.as_ref() == Some(&side) {
                    cx.emit(event.clone());
                }
            },
        )
        .detach();
        cx.subscribe(side, |this, side, event: &crate::chat::OpenLink, cx| {
            if this.side_chat.as_ref() == Some(&side) {
                cx.emit(crate::chat::OpenLink(event.0.clone()));
            }
        })
        .detach();
        cx.subscribe(side, |this, side, event: &crate::chat::OpenPeer, cx| {
            if this.side_chat.as_ref() == Some(&side) {
                cx.emit(crate::chat::OpenPeer(event.0.clone()));
            }
        })
        .detach();
    }

    fn activate_pane(&mut self, pane: Pane) {
        if (pane != Pane::NewTab || !self.blank_tabs.is_empty()) && !self.open_panes.contains(&pane)
        {
            self.open_panes.push(pane);
        }
        self.active_hosted = None;
        self.pane = pane;
        self.tab_order = self.sidebar_keys();
    }

    /// Terminal and browser instances occupy the same strip as other tools.
    /// Stable IDs preserve selection and ordering across moves and restarts.
    fn sidebar_tabs(&self) -> Vec<(Pane, Option<usize>)> {
        let mut tabs: Vec<_> = self
            .open_panes
            .iter()
            .flat_map(|pane| {
                if *pane == Pane::Terminal {
                    (0..self.terminal_tabs.len())
                        .map(|index| (*pane, Some(index)))
                        .collect()
                } else if *pane == Pane::Browser {
                    (0..self.browser_tabs.len())
                        .map(|index| (*pane, Some(index)))
                        .collect()
                } else if *pane == Pane::NewTab {
                    (0..self.blank_tabs.len())
                        .map(|index| (*pane, Some(index)))
                        .collect()
                } else {
                    vec![(*pane, None)]
                }
            })
            .collect();
        tabs.sort_by_key(|(pane, instance)| {
            let key = self.tab_key(*pane, *instance);
            self.tab_order
                .iter()
                .position(|tab| *tab == key)
                .unwrap_or(usize::MAX)
        });
        tabs
    }

    fn tab_key(&self, pane: Pane, instance: Option<usize>) -> SidebarTab {
        match (pane, instance) {
            (Pane::Terminal, Some(index)) => {
                SidebarTab::Terminal(self.terminal_tabs[index].clone())
            }
            (Pane::Browser, Some(index)) => {
                SidebarTab::Browser(self.browser_tabs[index].id.clone())
            }
            (Pane::NewTab, Some(index)) => SidebarTab::Blank(self.blank_tabs[index].id.clone()),
            _ => SidebarTab::Tool(pane),
        }
    }

    fn sidebar_keys(&self) -> Vec<SidebarTab> {
        let mut keys: Vec<_> = self
            .sidebar_tabs()
            .into_iter()
            .map(|(pane, instance)| self.tab_key(pane, instance))
            .chain(
                self.hosted
                    .iter()
                    .map(|tab| SidebarTab::Hosted(tab.id.clone())),
            )
            .collect();
        keys.sort_by_key(|key| {
            self.tab_order
                .iter()
                .position(|tab| tab == key)
                .unwrap_or(usize::MAX)
        });
        keys
    }

    fn begin_tab_drag(&mut self, key: &SidebarTab, cx: &mut Context<Self>) {
        let order = self.sidebar_keys();
        let Some(source) = order.iter().position(|tab| tab == key) else {
            return;
        };
        let centers: Vec<_> = (0..order.len())
            .filter_map(|index| {
                self.tool_tab_scroll
                    .bounds_for_item(index)
                    .map(|bounds| bounds.center().x)
            })
            .collect();
        if centers.len() != order.len() {
            return;
        }
        self.tab_drag = Some(SidebarDrag {
            key: key.clone(),
            pointer_offset: self.tab_drag_origin.get().x
                - self.tool_tab_scroll.offset().x
                - centers[source],
            order,
            centers,
        });
        cx.notify();
    }

    fn move_tab_drag(
        &mut self,
        owner: EntityId,
        key: &SidebarTab,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if owner != cx.entity_id() {
            return;
        }
        if !self.transfer_bounds.get().contains(&pointer) {
            return;
        }
        let Some(drag) = &self.tab_drag else {
            return;
        };
        if &drag.key != key {
            return;
        }
        let viewport = self.tool_tab_scroll.bounds();
        let mut offset = self.tool_tab_scroll.offset();
        if pointer.x < viewport.left() + zpx(20.) {
            offset.x = (offset.x + zpx(12.)).min(Pixels::ZERO);
        } else if pointer.x > viewport.right() - zpx(20.) {
            offset.x = (offset.x - zpx(12.)).max(-self.tool_tab_scroll.max_offset().x);
        }
        self.tool_tab_scroll.set_offset(offset);
        let center = pointer.x - offset.x - drag.pointer_offset;
        let source = drag.order.iter().position(|tab| tab == key).unwrap();
        let mut target = source;
        while target + 1 < drag.centers.len() && center > drag.centers[target + 1] {
            target += 1;
        }
        while target > 0 && center < drag.centers[target - 1] {
            target -= 1;
        }
        let mut order = drag.order.clone();
        let tab = order.remove(source);
        order.insert(target, tab);
        if order != self.sidebar_keys() {
            crate::haptics::tick();
        }
        self.tab_order = order;
        cx.notify();
    }

    fn finish_tab_drag(&mut self, cx: &mut Context<Self>) {
        if self.tab_drag.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn select_relative(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys = self.sidebar_keys();
        if keys.is_empty() {
            return;
        }
        let active = self
            .active_hosted
            .as_ref()
            .map(|id| SidebarTab::Hosted(id.clone()))
            .unwrap_or_else(|| self.tab_key(self.pane, self.pane_instance(self.pane)));
        let current = keys.iter().position(|key| *key == active).unwrap_or(0);
        let next = (current as isize + direction).rem_euclid(keys.len() as isize) as usize;
        match &keys[next] {
            SidebarTab::Hosted(id) => self.select_hosted(id.clone(), window, cx),
            key => {
                if let Some((pane, instance)) = self
                    .sidebar_tabs()
                    .into_iter()
                    .find(|(pane, instance)| self.tab_key(*pane, *instance) == *key)
                {
                    self.remember_blank(cx);
                    match pane {
                        Pane::NewTab => {
                            self.blank_active = instance.unwrap_or(0);
                        }
                        Pane::Terminal => {
                            self.terminal_active = instance.unwrap_or(0);
                        }
                        Pane::Browser => {
                            self.browser_active = instance.unwrap_or(0);
                        }
                        _ => {}
                    }
                    self.activate_pane(pane);
                    if pane == Pane::NewTab {
                        self.show_blank_address(window, cx);
                    }
                    self.prepare_to_show(window, cx);
                    self.focus_workspace(window, cx);
                    self.reveal_active_tab();
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn import_legacy_tab(
        &mut self,
        id: String,
        panel: Entity<RightPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.hosted.iter().any(|tab| tab.id == id) {
            return;
        }
        self.accept_tab(panel, window, cx);
        let old = self.active_hosted.replace(id.clone()).unwrap();
        self.hosted.last_mut().unwrap().id = id.clone();
        for key in &mut self.tab_order {
            if *key == SidebarTab::Hosted(old.clone()) {
                *key = SidebarTab::Hosted(id.clone());
            }
        }
    }

    fn pane_instance(&self, pane: Pane) -> Option<usize> {
        match pane {
            Pane::Terminal => Some(self.terminal_active),
            Pane::Browser => Some(self.browser_active),
            Pane::NewTab if !self.blank_tabs.is_empty() => Some(self.blank_active),
            _ => None,
        }
    }

    fn select_blank_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        self.blank_active = index;
        self.activate_pane(Pane::NewTab);
        self.reveal_active_tab();
        self.show_blank_address(window, cx);
        cx.notify();
    }

    pub(crate) fn choose_new_tool(
        &mut self,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A new tool takes this blank's place. Selecting an already-open
        // singleton tool leaves the blank available to return to.
        let position = if pane == Pane::Terminal || !self.open_panes.contains(&pane) {
            self.consume_blank(pane)
        } else {
            None
        };
        if pane == Pane::Terminal {
            self.new_terminal_tab(window, cx);
        } else {
            self.open_tool(pane, window, cx);
        }
        self.place_active_tab(position);
    }

    fn place_active_tab(&mut self, position: Option<usize>) {
        let Some(position) = position else {
            return;
        };
        let key = self.tab_key(self.pane, self.pane_instance(self.pane));
        let mut order = self.sidebar_keys();
        order.retain(|tab| tab != &key);
        order.insert(position.min(order.len()), key);
        self.tab_order = order;
        self.tool_tab_scroll.scroll_to_item(position);
    }

    fn reveal_active_tab(&self) {
        let key = self.active_hosted.as_ref().map_or_else(
            || self.tab_key(self.pane, self.pane_instance(self.pane)),
            |id| SidebarTab::Hosted(id.clone()),
        );
        if let Some(index) = self.sidebar_keys().iter().position(|tab| *tab == key) {
            self.tool_tab_scroll.scroll_to_item(index);
        }
    }

    fn open_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        if self.pane != pane {
            self.change_expanded.clear();
        }
        self.activate_pane(pane);
        self.reveal_active_tab();
        self.prepare_to_show(window, cx);
        match pane {
            Pane::Terminal => self.focus_terminal(window, cx),
            Pane::Changes => self.refresh_changes(cx),
            Pane::Files => self.refresh_tree(cx),
            Pane::SideChat => self.ensure_side_chat(window, cx),
            Pane::NewTab => self.show_blank_address(window, cx),
            _ => {}
        }
        self.focus_workspace(window, cx);
        cx.notify();
    }

    fn remember_blank(&mut self, cx: &App) {
        if self.pane == Pane::NewTab
            && let Some(tab) = self.blank_tabs.get_mut(self.blank_active)
        {
            tab.address = self.address.read(cx).value().to_string();
        }
    }

    fn show_blank_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .blank_tabs
            .get(self.blank_active)
            .map(|tab| tab.address.clone())
            .unwrap_or_default();
        self.address.update(cx, |input, cx| {
            input.set_value(value, window, cx);
            input.focus(window, cx);
        });
    }

    pub(crate) fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        self.blank_tabs.push(BlankTab {
            id: uuid::Uuid::new_v4().to_string(),
            address: String::new(),
        });
        self.blank_active = self.blank_tabs.len() - 1;
        self.activate_pane(Pane::NewTab);
        self.tool_tab_scroll
            .scroll_to_item(self.sidebar_keys().len() - 1);
        self.show_blank_address(window, cx);
        cx.notify();
    }

    fn consume_blank(&mut self, destination: Pane) -> Option<usize> {
        if self.active_hosted.is_some() || self.pane != Pane::NewTab || self.blank_tabs.is_empty() {
            return None;
        }
        let key = self.tab_key(Pane::NewTab, Some(self.blank_active));
        let position = self.sidebar_keys().iter().position(|tab| tab == &key);
        self.blank_tabs.remove(self.blank_active);
        self.blank_active = self
            .blank_active
            .min(self.blank_tabs.len().saturating_sub(1));
        if self.blank_tabs.is_empty() {
            self.open_panes.retain(|pane| *pane != Pane::NewTab);
        }
        // The caller immediately activates the destination. Avoid copying this
        // blank's address into its neighbor while switching.
        self.pane = destination;
        position
    }

    fn close_blank_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        if self.blank_tabs.len() == 1 {
            self.close_tool(Pane::NewTab, window, cx);
            return;
        }
        self.blank_tabs.remove(index);
        self.blank_active = if index < self.blank_active {
            self.blank_active - 1
        } else {
            self.blank_active
        }
        .min(self.blank_tabs.len() - 1);
        if self.pane == Pane::NewTab {
            self.show_blank_address(window, cx);
        }
        cx.notify();
    }

    fn navigate_from_new_tab(&mut self, cx: &mut Context<Self>) {
        let Some(url) = crate::new_tab::destination(self.address.read(cx).value().as_ref()) else {
            return;
        };
        let had_browser = self.open_panes.contains(&Pane::Browser);
        let position = self.consume_blank(Pane::Browser);
        self.activate_pane(Pane::Browser);
        // Reuse an untouched initial browser page; preserve existing browsing tabs.
        if !had_browser {
            self.browser_tabs[0].url = url;
            self.ensure_browser(cx);
        } else {
            self.new_browser_tab(Some(url), cx);
        }
        self.place_active_tab(position);
        cx.notify();
    }

    fn close_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.open_panes.iter().position(|p| *p == pane) else {
            return;
        };
        if pane == Pane::Files && self.has_unsaved_edits() {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Save or revert your file edits before closing this tab.",
                ),
                cx,
            );
            return;
        }
        match pane {
            Pane::Files => {
                if let Some(preview) = self.document_preview.take() {
                    preview.hide();
                }
                self.file_request += 1;
                self.viewer = Viewer::None;
                self.editors.clear();
                self.file_drafts.clear();
            }
            Pane::Terminal => {
                for (_, terminal) in self.terminals.drain() {
                    terminal.update(cx, |terminal, cx| terminal.close(cx));
                }
                self.terminal_tabs = vec![uuid::Uuid::new_v4().to_string()];
                self.terminal_active = 0;
                self.terminal_error = None;
            }
            Pane::Browser => {
                if let Some(browser) = self.browser.take() {
                    browser.hide();
                }
                self.browsers.clear();
                self.browser_tabs = vec![Default::default()];
                self.browser_active = 0;
                self.browser_watch = None;
            }
            Pane::Changes => {
                self.changes_watch = None;
                self.change_request += 1;
                self.changes_loading = false;
                self.change_expanded.clear();
            }
            Pane::SideChat => self.forget_side_chat(cx),
            Pane::NewTab => {
                self.blank_tabs.clear();
                self.blank_active = 0;
            }
        }
        self.open_panes.remove(index);
        if self.open_panes.is_empty() && !self.hosted.is_empty() {
            self.active_hosted = Some(self.hosted[0].id.clone());
        } else if self.open_panes.is_empty() {
            self.open_new_tab(window, cx);
        } else if self.pane == pane {
            let next = self.open_panes[index.min(self.open_panes.len() - 1)];
            self.open_tool(next, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_hosted.clone() {
            self.close_hosted(&id, window, cx);
        } else if self.pane == Pane::NewTab && !self.blank_tabs.is_empty() {
            self.close_blank_tab(self.blank_active, window, cx);
        } else if self.pane == Pane::Terminal && !self.terminal_tabs.is_empty() {
            self.close_terminal_tab(self.terminal_active, window, cx);
        } else if self.pane == Pane::Browser {
            self.close_browser_tab(self.browser_active, window, cx);
        } else {
            self.close_tool(self.pane, window, cx);
        }
        self.focus_workspace(window, cx);
    }

    pub fn has_unsaved_edits(&self) -> bool {
        !self.saving.is_empty()
            || self
                .file_drafts
                .values()
                .any(|draft| draft.text != draft.original)
    }

    /// Reuse the tool body in a session tab without the sidebar's pane switcher.
    pub fn show_workspace(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace = true;
        self.compact_tools = false;
        self.pane = pane;
        if pane == Pane::SideChat {
            self.ensure_side_chat(window, cx);
        }
        self.prepare_to_show(window, cx);
        if pane == Pane::Terminal {
            self.focus_terminal(window, cx);
        }
        cx.notify();
    }

    pub fn is_changes(&self) -> bool {
        self.active_hosted.is_none() && self.pane == Pane::Changes
    }

    /// Free everything a panel for a closed tab still holds: shells, web views,
    /// and the polling tasks. The panel's saved state is the caller's business.
    pub fn forget_side_chat(&mut self, cx: &mut Context<Self>) {
        self.side_chat_id = None;
        if let Some(side) = self.side_chat.take() {
            side.update(cx, |side, cx| side.forget(cx));
        }
    }

    pub fn restore_side_chat(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.side_chat_id = Some(id);
        self.show_workspace(Pane::SideChat, window, cx);
    }

    pub fn save_side_chat(&self, cx: &mut Context<Self>) {
        for tab in &self.hosted {
            tab.panel.update(cx, |panel, cx| panel.save_side_chat(cx));
        }
        if let Some(side) = &self.side_chat {
            side.update(cx, |side, cx| side.save(cx));
        }
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
        for tab in self.hosted.drain(..) {
            tab.panel.update(cx, |panel, cx| panel.release(cx));
        }
        if let Some(side) = self.side_chat.take() {
            side.update(cx, |side, cx| side.release(cx));
        }
        for (_, terminal) in self.terminals.drain() {
            terminal.update(cx, |terminal, cx| terminal.close(cx));
        }
        if let Some(browser) = self.browser.take() {
            browser.hide();
        }
        self.browsers.clear();
        self.browser_watch = None;
        self.changes_watch = None;
        self.document_preview = None;
    }

    pub fn set_preview_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if let Some(side) = &self.side_chat {
            let shown = visible && self.active_hosted.is_none() && self.pane == Pane::SideChat;
            side.update(cx, |side, cx| side.set_visible(shown, cx));
        }
        for tab in &self.hosted {
            tab.panel.update(cx, |panel, cx| {
                panel.set_preview_visible(
                    visible && Some(&tab.id) == self.active_hosted.as_ref(),
                    cx,
                )
            });
        }
        if !visible && let Some(preview) = &self.document_preview {
            preview.hide();
        }
        if !visible && let Some(browser) = &self.browser {
            browser.hide();
        }
        if self.preview_visible != visible {
            if !visible {
                self.change_expanded.clear();
            }
            self.preview_visible = visible;
            cx.notify();
        }
    }

    pub fn prepare_to_show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| Some(&tab.id) == self.active_hosted.as_ref())
        {
            tab.panel.update(cx, |panel, cx| {
                panel.side_context = self.side_context.clone();
                panel.prepare_to_show(window, cx);
            });
            return;
        }
        if self.pane == Pane::SideChat && self.side_chat.is_none() {
            self.ensure_side_chat(window, cx);
        }
        if self.pane == Pane::Changes && self.changes_watch.is_none() {
            self.changes_watch = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            if this.preview_visible && this.pane == Pane::Changes {
                                this.refresh_changes(cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if self.pane == Pane::Terminal {
            self.ensure_terminal(window, cx);
        } else if self.pane == Pane::Browser {
            self.ensure_browser(cx);
        } else if self.pane == Pane::Changes && self.change_request == 0 {
            self.refresh_changes(cx);
        }
    }
    pub fn snapshot(&self, open: bool, cx: &App) -> SavedPanel {
        SavedPanel {
            open,
            side_chat_id: self.side_chat_id.clone(),
            pane: self.pane,
            open_panes: self.open_panes.clone(),
            tab_order: self.sidebar_keys(),
            hosted: self
                .hosted
                .iter()
                .map(|tab| SavedHostedTab {
                    id: tab.id.clone(),
                    panel: tab.panel.read(cx).snapshot(false, cx),
                })
                .collect(),
            active_hosted: self.active_hosted.clone(),
            blank_tabs: self.blank_tabs.clone(),
            blank_active: self.blank_active,
            expanded: self.expanded.clone(),
            selected: match &self.viewer {
                Viewer::None => None,
                Viewer::Document { path }
                | Viewer::Text { path, .. }
                | Viewer::Note { path, .. }
                | Viewer::Loading { path } => Some(path.clone()),
            },
            show_hidden: self.show_hidden,
            tree_height: self.file_tree_height,
            markdown_preview: self.markdown_preview,
            browser_profile: self.browser_profile.clone(),
            browser_tabs: self.browser_snapshot(),
            browser_active: self.browser_active,
            terminal_tabs: self.terminal_tabs.clone(),
            terminal_active: self.terminal_active,
            file_drafts: self
                .file_drafts
                .iter()
                .filter(|(_, draft)| draft.text != draft.original)
                .map(|(path, draft)| (path.clone(), draft.clone()))
                .collect(),
        }
    }

    pub fn restore(&mut self, mut saved: SavedPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.hosted.clear();
        let mut hosted = Vec::new();
        for tab in saved.hosted {
            let mut keys = Vec::new();
            for (index, panel) in tab.panel.split_browser_pages().into_iter().enumerate() {
                let id = if index == 0 {
                    tab.id.clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                };
                keys.push(SidebarTab::Hosted(id.clone()));
                hosted.push(SavedHostedTab { id, panel });
            }
            if let Some(index) = saved
                .tab_order
                .iter()
                .position(|key| *key == SidebarTab::Hosted(tab.id.clone()))
            {
                saved.tab_order.splice(index..=index, keys);
            }
        }
        for tab in hosted {
            let panel = cx.new(|cx| {
                let mut panel = RightPanel::new(self.root.clone(), window, cx);
                panel.side_context = self.side_context.clone();
                panel.restore_workspace(tab.panel, window, cx);
                panel
            });
            self.watch_hosted(&panel, cx);
            self.hosted.push(HostedTab { id: tab.id, panel });
        }
        self.active_hosted = saved
            .active_hosted
            .filter(|id| self.hosted.iter().any(|tab| &tab.id == id));
        self.tab_order = saved.tab_order;
        self.tab_drag = None;
        self.side_chat_id = saved.side_chat_id;
        self.blank_tabs = saved.blank_tabs;
        let mut ids = HashSet::new();
        self.blank_tabs.retain(|tab| ids.insert(tab.id.clone()));
        self.blank_active = saved
            .blank_active
            .min(self.blank_tabs.len().saturating_sub(1));
        self.pane = if (saved.pane == Pane::Changes
            || (saved.pane == Pane::NewTab && self.blank_tabs.is_empty()))
            && !saved.open_panes.is_empty()
        {
            Pane::Files
        } else {
            saved.pane
        };
        self.open_panes = saved
            .open_panes
            .into_iter()
            .filter(|p| {
                !(*p == Pane::NewTab && self.blank_tabs.is_empty())
                    && !(*p == Pane::SideChat && self.side_chat_id.is_none())
            })
            .collect();
        self.open_panes.dedup();
        if !self.blank_tabs.is_empty() && !self.open_panes.contains(&Pane::NewTab) {
            self.open_panes.push(Pane::NewTab);
        }
        if self.pane != Pane::NewTab && !self.open_panes.contains(&self.pane) {
            self.pane = self.open_panes.first().copied().unwrap_or(Pane::NewTab);
        }
        self.terminal_tabs = if saved.terminal_tabs.is_empty() {
            vec![uuid::Uuid::new_v4().to_string()]
        } else {
            saved.terminal_tabs
        };
        self.terminal_active = saved.terminal_active.min(self.terminal_tabs.len() - 1);
        self.file_drafts = saved.file_drafts;
        self.editors.clear();
        self.browser_profile = if uuid::Uuid::parse_str(&saved.browser_profile).is_ok() {
            saved.browser_profile
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        self.browser_tabs = if saved.browser_tabs.is_empty() {
            vec![Default::default()]
        } else {
            saved.browser_tabs
        };
        self.browser_active = saved.browser_active.min(self.browser_tabs.len() - 1);
        // Expand the legacy Browser group in place, preserving saved page IDs.
        self.tab_order = self
            .tab_order
            .iter()
            .flat_map(|key| {
                if *key == SidebarTab::Tool(Pane::Browser) {
                    self.browser_tabs
                        .iter()
                        .map(|tab| SidebarTab::Browser(tab.id.clone()))
                        .collect()
                } else {
                    vec![key.clone()]
                }
            })
            .collect();
        self.browsers.clear();
        self.browser = None;
        self.browser_watch = None;
        self.markdown_preview = saved.markdown_preview;
        self.expanded = saved.expanded;
        self.show_hidden = saved.show_hidden;
        self.file_tree_height = if saved.tree_height.is_finite() {
            saved.tree_height.max(MIN_SPLIT_HEIGHT)
        } else {
            DEFAULT_TREE_HEIGHT
        };
        self.refresh_tree(cx);
        if let Some(path) = saved.selected {
            self.open_file(path, cx);
        }
        if self.open_panes.is_empty() && !self.hosted.is_empty() {
            self.active_hosted = Some(self.hosted[0].id.clone());
        } else if self.open_panes.is_empty() {
            self.open_new_tab(window, cx);
        } else if self.pane == Pane::NewTab {
            self.show_blank_address(window, cx);
        }
        if saved.open && saved.pane != Pane::Changes {
            self.prepare_to_show(window, cx);
        }
    }

    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter a URL"));
        cx.subscribe_in(&address, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.navigate_from_new_tab(cx);
            } else if matches!(event, InputEvent::Change) {
                this.remember_blank(cx);
                cx.notify();
            }
        })
        .detach();
        let mut panel = Self {
            focus: cx.focus_handle(),
            workspace: false,
            compact_tools: true,
            watched_hosted: Default::default(),
            transfer_bounds: Default::default(),
            side_context: None,
            side_chat: None,
            side_chat_id: None,
            pane: Pane::NewTab,
            open_panes: default_panes(),
            tab_order: Vec::new(),
            hosted: Vec::new(),
            active_hosted: None,
            blank_tabs: vec![BlankTab {
                id: uuid::Uuid::new_v4().to_string(),
                address: String::new(),
            }],
            blank_active: 0,
            address,
            new_tab_scroll: ScrollHandle::new(),
            tool_tab_scroll: ScrollHandle::new(),
            revealed_tab: None,
            motion: Default::default(),
            tab_drag: None,
            tab_drag_origin: Rc::default(),
            root,
            expanded: HashSet::new(),
            file_hover: Default::default(),
            viewer: Viewer::None,
            document_preview: None,
            preview_visible: true,
            terminals: HashMap::new(),
            terminal_tabs: vec![uuid::Uuid::new_v4().to_string()],
            terminal_active: 0,
            file_drafts: HashMap::new(),
            editors: HashMap::new(),
            saving: HashSet::new(),
            file_error: None,
            change_files: Vec::new(),
            change_root: PathBuf::new(),
            change_diffs: HashMap::new(),
            change_reviews: HashMap::new(),
            change_omitted: 0,
            change_expanded: HashSet::new(),
            change_error: None,
            change_request: 0,
            changes_loading: false,
            changes_watch: None,
            browser: None,
            browser_profile: uuid::Uuid::new_v4().to_string(),
            browser_tabs: vec![Default::default()],
            browser_active: 0,
            browsers: HashMap::new(),
            browser_watch: None,
            terminal_error: None,
            show_hidden: false,
            file_tree_height: DEFAULT_TREE_HEIGHT,
            file_request: 0,
            tree_request: 0,
            directory_entries: HashMap::new(),
            split_origin: Default::default(),
            split_start_height: Default::default(),
            rows: Vec::new(),
            markdown_preview: false,
            markdown_scroll: ScrollHandle::new(),
        };
        panel.refresh_tree(cx);
        panel
    }

    /// Point the panel at another project folder.
    pub fn set_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root != root {
            for (_, terminal) in self.terminals.drain() {
                terminal.update(cx, |terminal, cx| terminal.close(cx));
            }
            self.terminal_tabs = vec![uuid::Uuid::new_v4().to_string()];
            self.terminal_active = 0;
            self.change_request += 1;
            self.change_files.clear();
            self.change_diffs.clear();
            self.change_reviews.clear();
            self.change_omitted = 0;
            self.change_expanded.clear();
            self.changes_loading = false;
            self.root = root;
            self.directory_entries.clear();
            self.rows.clear();
            self.expanded.clear();
            self.document_preview = None;
            self.file_request += 1;
            self.viewer = Viewer::None;
            self.refresh_tree(cx);
            self.terminal_error = None;
            if self.pane == Pane::Terminal {
                self.ensure_terminal(window, cx);
            } else if self.pane == Pane::Changes {
                self.refresh_changes(cx);
            }
            cx.notify();
        }
    }

    pub fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_pane(Pane::Terminal);
        self.ensure_terminal(window, cx);
        self.focus_terminal(window, cx);
        cx.notify();
    }

    fn ensure_browser(&mut self, cx: &mut Context<Self>) {
        if self.browser.is_some() {
            return;
        }
        let tab = &self.browser_tabs[self.browser_active];
        let browser = self
            .browsers
            .entry(tab.id.clone())
            .or_insert_with(|| crate::browser::Browser::new(&tab.url, &self.browser_profile))
            .clone();
        self.browser = Some(browser);
        if self.browser_watch.is_some() {
            return;
        }
        // Native navigation doesn't emit GPUI events. Persist URL changes through
        // the same panel observer as file selection, without repainting each tick.
        self.browser_watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let tabs = this.browser_snapshot();
                        if this.browser_tabs != tabs {
                            this.browser_tabs = tabs;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn browser_snapshot(&self) -> Vec<crate::browser::SavedTab> {
        self.browser_tabs
            .iter()
            .map(|tab| crate::browser::SavedTab {
                id: tab.id.clone(),
                url: self
                    .browsers
                    .get(&tab.id)
                    .map_or_else(|| tab.url.clone(), |browser| browser.url()),
            })
            .collect()
    }
    fn select_browser_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.take() {
            browser.hide();
        }
        self.browser_tabs = self.browser_snapshot();
        self.browser_active = index.min(self.browser_tabs.len() - 1);
        self.ensure_browser(cx);
        cx.notify();
    }
    fn new_browser_tab(&mut self, url: Option<String>, cx: &mut Context<Self>) -> String {
        let mut tab = crate::browser::SavedTab::default();
        if let Some(url) = url {
            tab.url = url;
        }
        let id = tab.id.clone();
        if !self.open_panes.contains(&Pane::Browser) {
            self.browser_tabs.clear();
            self.open_panes.push(Pane::Browser);
        }
        self.browser_tabs.push(tab);
        self.tab_order = self.sidebar_keys();
        self.select_browser_tab(self.browser_tabs.len() - 1, cx);
        id
    }
    fn close_browser_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser_tabs.len() == 1 {
            self.close_tool(Pane::Browser, window, cx);
            return;
        }
        self.browser_tabs = self.browser_snapshot();
        if let Some(browser) = self.browser.take() {
            browser.hide();
        }
        let closed = self.browser_tabs.remove(index);
        self.browsers.remove(&closed.id);
        self.browser_active = if index < self.browser_active {
            self.browser_active - 1
        } else {
            self.browser_active
        }
        .min(self.browser_tabs.len() - 1);
        self.tab_order = self.sidebar_keys();
        self.ensure_browser(cx);
        cx.notify();
    }
    pub(crate) fn contains_peer(&self, id: &del_agent::SessionId, cx: &App) -> bool {
        self.side_chat
            .as_ref()
            .is_some_and(|side| side.read(cx).session_id() == Some(id))
            || self
                .hosted
                .iter()
                .any(|tab| tab.panel.read(cx).contains_peer(id, cx))
    }

    pub(crate) fn show_peer(
        &mut self,
        id: &del_agent::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| tab.panel.read(cx).contains_peer(id, cx))
        {
            let key = tab.id.clone();
            tab.panel
                .update(cx, |panel, cx| panel.show_peer(id, window, cx));
            self.select_hosted(key, window, cx);
            return true;
        }
        if self
            .side_chat
            .as_ref()
            .is_some_and(|side| side.read(cx).session_id() == Some(id))
        {
            // Select the existing pane without converting the sidebar into
            // an embedded workspace (which would hide its tab strip).
            self.open_tool(Pane::SideChat, window, cx);
            self.focus_side_chat(window, cx);
            return true;
        }
        false
    }

    pub(crate) fn close_browser_id(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.open_panes.contains(&Pane::Browser)
            && let Some(index) = self.browser_tabs.iter().position(|tab| tab.id == id)
        {
            self.close_browser_tab(index, window, cx);
            return true;
        }
        let hosted = self
            .hosted
            .iter()
            .find(|tab| Self::find_browser_panel(&tab.panel, id, cx).is_some())
            .map(|tab| tab.id.clone());
        if let Some(key) = hosted {
            self.close_hosted(&key, window, cx);
            return true;
        }
        false
    }

    pub(crate) fn find_browser_panel(
        panel: &Entity<Self>,
        id: &str,
        cx: &App,
    ) -> Option<Entity<Self>> {
        let current = panel.read(cx);
        if current.open_panes.contains(&Pane::Browser)
            && current.browser_tabs.iter().any(|tab| tab.id == id)
        {
            return Some(panel.clone());
        }
        current
            .hosted
            .iter()
            .find_map(|tab| Self::find_browser_panel(&tab.panel, id, cx))
    }

    pub(crate) fn agent_browser_tabs(&self, cx: &App) -> Vec<crate::browser::SavedTab> {
        let mut tabs = if self.open_panes.contains(&Pane::Browser) {
            self.browser_snapshot()
        } else {
            Vec::new()
        };
        for tab in &self.hosted {
            tabs.extend(tab.panel.read(cx).agent_browser_tabs(cx));
        }
        tabs
    }

    pub(crate) fn show_browser_id(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(hosted) = self
            .hosted
            .iter()
            .find(|tab| Self::find_browser_panel(&tab.panel, id, cx).is_some())
        {
            let key = hosted.id.clone();
            hosted.panel.update(cx, |panel, cx| {
                panel.show_browser_id(id, window, cx);
            });
            self.select_hosted(key, window, cx);
            return true;
        }
        self.reveal_browser(id, window, cx)
    }

    pub(crate) fn browser_preview_source(&self, id: &str) -> Option<crate::browser::Browser> {
        if !self.open_panes.contains(&Pane::Browser) {
            return None;
        }
        self.browsers.get(id).cloned()
    }

    pub(crate) fn browser_surface_visible(&self, cx: &App) -> bool {
        if !self.preview_visible {
            return false;
        }
        if let Some(active) = &self.active_hosted {
            return self
                .hosted
                .iter()
                .find(|tab| &tab.id == active)
                .is_some_and(|tab| tab.panel.read(cx).browser_surface_visible(cx));
        }
        self.pane == Pane::Browser
    }

    pub(crate) fn active_browser_id(&self) -> String {
        self.browser_tabs[self.browser_active].id.clone()
    }

    pub(crate) fn reveal_browser(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.browser_tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.activate_pane(Pane::Browser);
        self.select_browser_tab(index, cx);
        self.prepare_to_show(window, cx);
        true
    }

    /// Run an agent browser action without stealing the user's selected pane.
    /// The browser joins the tab strip; only an explicit user reveal selects it.
    pub fn browser_command(
        &mut self,
        request: del_mcp::BrowserRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<crate::browser::BrowserReply, String> {
        request.validate()?;
        if request.action == "new_tab"
            && self.browser_tabs.len() >= del_mcp::browser::MAX_AGENT_TABS
        {
            return Err(format!(
                "This session already has {} browser tabs; close one first",
                del_mcp::browser::MAX_AGENT_TABS
            ));
        }
        if let Some(id) = &request.tab_id {
            let index = self
                .browser_tabs
                .iter()
                .position(|tab| &tab.id == id)
                .ok_or("Unknown browser tab")?;
            self.select_browser_tab(index, cx);
        }
        if !matches!(
            request.action.as_str(),
            "list_tabs" | "new_tab" | "close_tab"
        ) && !self.open_panes.contains(&Pane::Browser)
        {
            self.open_panes.push(Pane::Browser);
            self.tab_order = self.sidebar_keys();
        }
        let immediate = match request.action.as_str() {
            "new_tab" => {
                self.new_browser_tab(request.url.clone(), cx);
                true
            }
            "close_tab" => {
                self.close_browser_tab(self.browser_active, window, cx);
                true
            }
            "select_tab" | "list_tabs" => true,
            _ => false,
        };
        cx.notify();
        if immediate {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let open = self.open_panes.contains(&Pane::Browser);
            let tabs = if open {
                self.browser_snapshot()
            } else {
                Vec::new()
            };
            let active = open.then(|| self.active_browser_id());
            let _ = tx.send(Ok(serde_json::json!({"tabs":tabs,"active_tab":active})));
            return Ok(rx);
        }
        self.ensure_browser(cx);
        let mut command = serde_json::to_value(&request).map_err(|e| e.to_string())?;
        if matches!(
            request.action.as_str(),
            "snapshot" | "click" | "fill" | "press_key" | "evaluate"
        ) {
            command["script"] = crate::browser::script_for(&request)?.into();
        }
        Ok(self.browser.as_ref().unwrap().command(command, window))
    }
    fn render_browser(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.preview_visible {
            return div().into_any_element();
        }
        self.ensure_browser(cx);
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            // Native WebKit must leave the shell splitter's hit area uncovered.
            .pl(zpx(6.))
            .child(self.browser.as_ref().unwrap().element())
            .into_any_element()
    }

    fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.terminal_tabs[self.terminal_active].clone();
        if self.terminals.contains_key(&id) {
            return;
        }
        match TerminalHost::spawn(TerminalLaunchSpec::shell(self.root.clone()), window, cx) {
            Ok(terminal) => {
                cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
                self.terminals.insert(id, terminal);
                self.terminal_error = None;
            }
            Err(error) => {
                let message = format!("Could not open terminal: {error}");
                self.terminal_error = Some(message.clone());
                window.push_notification(
                    gpui_kit::component::notification::Notification::error(message)
                        .title("Terminal unavailable")
                        .autohide(false),
                    cx,
                );
            }
        }
    }

    fn focus_terminal(&self, window: &mut Window, cx: &mut App) {
        if let Some(terminal) = self
            .terminals
            .get(&self.terminal_tabs[self.terminal_active])
        {
            terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
        }
    }

    pub fn preview_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.activate_pane(Pane::Files);
        self.open_file(path, cx);
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self
            .file_drafts
            .get(&path)
            .is_some_and(|draft| draft.text == draft.original)
            && !self.saving.contains(&path)
        {
            self.file_drafts.remove(&path);
            self.editors.remove(&path);
        }
        self.file_error = None;
        self.file_request += 1;
        let request = self.file_request;
        self.viewer = Viewer::Loading { path: path.clone() };
        self.markdown_scroll = ScrollHandle::new();
        let load = cx
            .background_executor()
            .spawn(async move { read_viewer(path) });
        cx.spawn(async move |this, cx| {
            let mut viewer = load.await;
            let _ = this.update(cx, |this, cx| {
                if this.file_request == request {
                    if let Viewer::Text {
                        path,
                        markdown: Some(markdown),
                        ..
                    } = &mut viewer
                        && let Some(draft) = this.file_drafts.get(path)
                    {
                        *markdown = draft.text.clone().into();
                    }
                    this.document_preview = match &viewer {
                        Viewer::Document { path } => {
                            Some(crate::document_preview::DocumentPreview::new(path))
                        }
                        _ => None,
                    };
                    this.viewer = viewer;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        self.tree_request += 1;
        let request = self.tree_request;
        let show_hidden = self.show_hidden;
        let mut paths = self
            .expanded
            .iter()
            .filter(|path| path.starts_with(&self.root))
            .cloned()
            .collect::<HashSet<_>>();
        paths.insert(self.root.clone());
        self.rebuild_tree();
        for path in paths {
            let directory = path.clone();
            let load = cx
                .background_executor()
                .spawn(async move { read_directory(&directory, show_hidden) });
            cx.spawn(async move |this, cx| {
                let entries = load.await;
                let _ = this.update(cx, |this, cx| {
                    if this.tree_request != request {
                        return;
                    }
                    this.directory_entries.insert(path, entries);
                    this.rebuild_tree();
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn rebuild_tree(&mut self) {
        self.rows = cached_tree_rows(&self.root, &self.expanded, &self.directory_entries);
    }

    fn tab(
        &self,
        pane: Pane,
        instance: Option<usize>,
        hosted: Option<String>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (base, name) = pane.tab_label();
        let id: SharedString = if let Some(id) = &hosted {
            format!("tab-hosted-{id}").into()
        } else {
            match instance {
                Some(index) if pane == Pane::NewTab => {
                    format!("{base}-{}", self.blank_tabs[index].id).into()
                }
                Some(index) if pane == Pane::Browser => {
                    format!("{base}-{}", self.browser_tabs[index].id).into()
                }
                Some(index) => format!("{base}-{}", self.terminal_tabs[index]).into(),
                None => base.into(),
            }
        };
        let working = if let Some(id) = &hosted {
            self.hosted
                .iter()
                .find(|tab| &tab.id == id)
                .is_some_and(|tab| tab.panel.read(cx).is_working(cx))
        } else {
            pane == Pane::SideChat && self.is_working(cx)
        };
        let unread = if let Some(id) = &hosted {
            self.hosted
                .iter()
                .find(|tab| &tab.id == id)
                .is_some_and(|tab| {
                    tab.panel
                        .read(cx)
                        .is_unread_with(self.side_context.as_ref(), cx)
                })
        } else {
            pane == Pane::SideChat && self.is_unread(cx)
        };
        let label: SharedString = name.into();
        let close_id = format!("close-{id}");
        let close_selector = instance
            .map(|index| format!("close-{base}-{index}"))
            .unwrap_or_else(|| close_id.clone());
        let active = if hosted.is_some() {
            hosted == self.active_hosted
        } else {
            self.active_hosted.is_none()
                && self.pane == pane
                && instance == self.pane_instance(pane)
        };
        let key = hosted
            .clone()
            .map(SidebarTab::Hosted)
            .unwrap_or_else(|| self.tab_key(pane, instance));
        let dragging = self.tab_drag.as_ref().is_some_and(|drag| drag.key == key);
        let payload = DraggedSidebarTab {
            owner: cx.entity_id(),
            key,
            pane,
            origin: self.tab_drag_origin.clone(),
            offset: Point::default(),
            cursor_offset: Point::default(),
        };
        let origin = self.tab_drag_origin.clone();

        let hosted_click = hosted.clone();
        let panel = cx.entity();
        let selector = id.to_string();
        div()
            .id(id.clone())
            .debug_selector(move || selector.clone())
            .group(id.clone())
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .px_2()
            .w(zpx(crate::icons::TOOL_TAB_WIDTH))
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .flex()
            .flex_none()
            .whitespace_nowrap()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(zpx(14.))
            .text_color(if active {
                rgb(0xf5f5f5)
            } else {
                rgba(0xffffff8c)
            })
            .when(active, |t| t.bg(rgba(0xffffff14)))
            .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
            .when(dragging, |tab| tab.opacity(0.))
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                origin.set(event.position);

                cx.stop_propagation();
            })
            .on_drag(payload, move |drag, offset, window, cx| {
                let mut preview = drag.clone();

                let movement = window.mouse_position() - drag.origin.get();
                preview.offset = offset - movement;
                preview.cursor_offset = offset;
                panel.update(cx, |this, cx| {
                    this.begin_tab_drag(&drag.key, cx);
                    this.move_tab_drag(drag.owner, &drag.key, window.mouse_position(), cx);
                });
                cx.new(|_| preview)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(id) = &hosted_click {
                    this.select_hosted(id.clone(), window, cx);
                    return;
                }
                if let Some(index) = instance {
                    if pane == Pane::NewTab {
                        this.select_blank_tab(index, window, cx);
                        return;
                    }
                    if pane == Pane::Browser {
                        this.select_browser_tab(index, cx);
                    } else {
                        this.terminal_active = index;
                    }
                }
                this.open_tool(pane, window, cx);
            }))
            .child(
                div()
                    .mr_1p5()
                    .flex_none()
                    .child(crate::icons::icon(pane.icon()).with_size(zpx(15.))),
            )
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .when(working, |tab| tab.child(crate::icons::working_spinner()))
            .when(unread && !working, |tab| {
                tab.child(crate::icons::unread_mark())
            })
            .child(
                div()
                    .id(SharedString::from(close_id))
                    .debug_selector(move || close_selector.clone())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Close {label} sidebar tab"))
                    .flex_none()
                    .ml_1()
                    .size(zpx(crate::icons::TAB_CLOSE_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_color(rgba(0xffffff80))
                    .opacity(0.)
                    .group_hover(id, |button| button.opacity(1.))
                    .hover(|b| b.bg(rgba(0xffffff14)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if let Some(id) = &hosted {
                            this.close_hosted(id, window, cx);
                            return;
                        }
                        if let Some(index) = instance {
                            if pane == Pane::NewTab {
                                this.close_blank_tab(index, window, cx);
                            } else if pane == Pane::Browser {
                                this.close_browser_tab(index, window, cx);
                            } else {
                                this.close_terminal_tab(index, window, cx);
                            }
                        } else {
                            this.close_tool(pane, window, cx);
                        }
                    }))
                    .child(
                        crate::icons::icon(IconName::Close)
                            .with_size(zpx(crate::icons::TAB_CLOSE_GLYPH)),
                    ),
            )
    }

    fn render_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_editor(window, cx);
        let rows = self.rows.clone();
        let selected = match &self.viewer {
            Viewer::Document { path }
            | Viewer::Text { path, .. }
            | Viewer::Note { path, .. }
            | Viewer::Loading { path } => Some(path.clone()),
            Viewer::None => None,
        };
        let muted = rgba(0xffffff8c);

        let empty_default_split =
            selected.is_none() && self.file_tree_height == DEFAULT_TREE_HEIGHT;

        // Match the tab strip + tab padding, less the row's own quarter-rem inset.
        let tree_inset = window.rem_size() * 0.75;
        let tree = v_flex()
            .id("file-tree")
            .relative()
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .child(self.file_hover.layer(None, rgba(0)))
            .children(rows.into_iter().map(|(path, depth, is_dir)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let open = self.expanded.contains(&path);
                let is_selected = selected.as_ref() == Some(&path);
                let click = path.clone();
                let hover = self.file_hover.clone();
                let hover_key = path.display().to_string();
                let marker_key = hover_key.clone();
                h_flex()
                    .w_full()
                    .h(zpx(26.0))
                    .flex_none()
                    .pl(tree_inset + zpx(depth as f32 * 12.0))
                    .pr_1()
                    .child(
                        h_flex()
                            .id(SharedString::from(path.display().to_string()))
                            .relative()
                            .flex_1()
                            .h(zpx(26.0))
                            .min_w_0()
                            .px_1()
                            .gap_1p5()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(zpx(TEXT_MD))
                            .text_color(if is_dir {
                                rgb(0xf0f0f0)
                            } else {
                                rgba(0xffffffcc)
                            })
                            .when(is_selected, |r| r.text_color(rgb(0xf5f5f5)))
                            .on_hover(move |entered, window, cx| {
                                hover.hover(&hover_key, *entered, window, cx);
                            })
                            .child(self.file_hover.marker(
                                marker_key,
                                if is_selected {
                                    ActionTone::Accent.surface(false)
                                } else {
                                    rgba(0)
                                },
                                rgba(0xffffff24),
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_dir {
                                    if !this.expanded.remove(&click) {
                                        this.expanded.insert(click.clone());
                                    }
                                    this.refresh_tree(cx);
                                    cx.notify();
                                } else {
                                    this.open_file(click.clone(), cx);
                                }
                            }))
                            .child(
                                crate::icons::icon(if is_dir {
                                    if open {
                                        IconName::FolderOpen
                                    } else {
                                        IconName::Folder
                                    }
                                } else {
                                    IconName::File
                                })
                                .with_size(zpx(15.))
                                .text_color(if is_dir && open {
                                    rgb(0xf5f5f5)
                                } else if is_dir {
                                    rgba(0xffffffa6)
                                } else {
                                    muted
                                }),
                            )
                            .child(div().min_w_0().truncate().child(name)),
                    )
            }));

        let viewer = match &self.viewer {
            Viewer::Document { .. } => self
                .document_preview
                .as_ref()
                .filter(|_| self.preview_visible)
                .map(|preview| preview.element().into_any_element())
                .unwrap_or_else(|| div().child("Loading document…").into_any_element()),
            Viewer::None => div()
                .p_3()
                .whitespace_nowrap()
                .text_size(zpx(TEXT_MD))
                .text_color(muted)
                .child("Select a file to view it")
                .into_any_element(),
            Viewer::Loading { .. } => div()
                .p_3()
                .text_size(zpx(TEXT_MD))
                .text_color(muted)
                .child("Loading file…")
                .into_any_element(),
            Viewer::Note { message, .. } => div()
                .p_3()
                .whitespace_nowrap()
                .text_size(zpx(TEXT_MD))
                .text_color(muted)
                .child(message.clone())
                .into_any_element(),
            Viewer::Text {
                markdown: Some(markdown),
                ..
            } if self.markdown_preview => div()
                .id("file-markdown-preview")
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.markdown_scroll)
                .child(
                    div()
                        .p_3()
                        .w_full()
                        .whitespace_normal()
                        .text_size(zpx(TEXT_MD))
                        .child(
                            TextView::markdown(
                                ("file-markdown", self.file_request),
                                markdown.clone(),
                            )
                            .selectable(true),
                        ),
                )
                .into_any_element(),
            Viewer::Text { path, .. } => self
                .editors
                .get(path)
                .map(|editor| {
                    div()
                        .size_full()
                        .key_context("FileEditor")
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if event.keystroke.modifiers.platform && event.keystroke.key == "s" {
                                cx.stop_propagation();
                                this.save_selected(cx);
                            }
                        }))
                        .child(
                            Editor::new(editor)
                                .h(relative(1.))
                                .appearance(false)
                                .bordered(false)
                                .font_family(MONO)
                                .text_size(zpx(TEXT_MD))
                                .aria_label("File editor"),
                        )
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element()),
        };

        let is_markdown = matches!(
            &self.viewer,
            Viewer::Text {
                markdown: Some(_),
                ..
            }
        );
        let preview = v_flex()
            .size_full()
            .min_h_0()
            .rounded(zpx(10.0))
            .overflow_hidden()
            .when_some(selected, |preview, path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                preview.child(
                    h_flex()
                        .h(zpx(28.0))
                        .rounded_t(zpx(10.0))
                        .flex_none()
                        .px_2()
                        .gap_2()
                        .bg(rgba(0xffffff08))
                        .items_center()
                        .child(crate::icons::icon(IconName::File).text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(zpx(TEXT_MD))
                                .child(if self.selected_dirty() {
                                    format!("{name} •")
                                } else {
                                    name
                                }),
                        )
                        .when(is_markdown, |header| {
                            header.child(h_flex().gap_0p5().flex_none().children(
                                [(false, "Markdown"), (true, "Preview")].into_iter().map(
                                    |(preview, label)| {
                                        div()
                                            .id(label)
                                            .role(gpui::Role::Button)
                                            .aria_label(label)
                                            .h(zpx(20.0))
                                            .px_1p5()
                                            .flex()
                                            .items_center()
                                            .rounded_md()
                                            .text_size(zpx(TEXT_SM))
                                            .cursor_pointer()
                                            .when(self.markdown_preview == preview, |button| {
                                                button
                                                    .bg(ActionTone::Accent.surface(false))
                                                    .text_color(ActionTone::Accent.foreground())
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.markdown_preview = preview;
                                                cx.notify();
                                            }))
                                            .child(label)
                                    },
                                ),
                            ))
                        })
                        .child(
                            div()
                                .id("close-file-preview")
                                .role(gpui::Role::Button)
                                .aria_label("Close file preview")
                                .size(crate::icons::button_size())
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|button| button.bg(rgba(0xffffff14)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.file_request += 1;
                                    this.document_preview = None;
                                    this.viewer = Viewer::None;
                                    cx.notify();
                                }))
                                .child(crate::icons::icon(IconName::Close).text_color(muted)),
                        ),
                )
            })
            .when(matches!(self.viewer, Viewer::Text { .. }), |preview| {
                preview.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .child(
                            action(
                                "save-file",
                                if self.selected_dirty() {
                                    "Save •"
                                } else {
                                    "Save"
                                },
                                ActionTone::Success,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_selected(cx))),
                        )
                        .child(
                            action("revert-file", "Revert", ActionTone::Warning).on_click(
                                cx.listener(|this, _, window, cx| this.revert_selected(window, cx)),
                            ),
                        )
                        .child(
                            action("reload-file", "Reload", ActionTone::Accent)
                                .on_click(cx.listener(|this, _, _, cx| this.reload_selected(cx))),
                        ),
                )
            })
            .when_some(self.file_error.clone(), |preview, error| {
                preview.child(
                    div()
                        .p_2()
                        .text_size(zpx(TEXT_SM))
                        .whitespace_normal()
                        .text_color(rgb(0xf08080))
                        .child(error),
                )
            })
            .child(v_flex().flex_1().min_h_0().overflow_hidden().child(viewer));

        let measured_tree_height = Rc::new(Cell::new(self.file_tree_height));
        let split_drag = FileSplitDrag {
            start_y: self.split_origin.clone(),
            start_height: self.split_start_height.clone(),
        };

        let start_y = split_drag.start_y.clone();
        let start_height = split_drag.start_height.clone();
        let measured_start = measured_tree_height.clone();
        v_flex()
            .debug_selector(|| "files-layout".into())
            .size_full()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(tree) = bounds.first() {
                    measured_tree_height.set(del_core::zoom::unscaled(tree.size.height.as_f32()));
                }
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<FileSplitDrag>, _, cx| {
                    let drag = event.drag(cx);
                    let delta = del_core::zoom::unscaled(
                        event.event.position.y.as_f32() - drag.start_y.get(),
                    );
                    this.file_tree_height = resized_tree_height(
                        drag.start_height.get(),
                        delta,
                        del_core::zoom::unscaled(event.bounds.size.height.as_f32()),
                    );
                    cx.notify();
                }),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .h(zpx(self.file_tree_height))
                    .when(empty_default_split, |tree| tree.h(relative(0.8)))
                    .max_h(relative(0.85))
                    .overflow_hidden()
                    .child(tree),
            )
            .child(
                div()
                    .id("file-preview-resize")
                    .debug_selector(|| "file-preview-resize".into())
                    .group("file-preview-resize")
                    .h(zpx(8.0))
                    .w_full()
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .cursor_row_resize()
                    .on_mouse_down(MouseButton::Left, move |event, _, _| {
                        start_y.set(event.position.y.as_f32());
                        start_height.set(measured_start.get());
                    })
                    .on_drag(split_drag, |_, _, _, cx| cx.new(|_| FileSplitPreview))
                    .child(
                        div()
                            .h(zpx(1.0))
                            .w_full()
                            .bg(rgba(0xffffff10))
                            .group_hover("file-preview-resize", |line| line.bg(rgba(0xffffff24))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(preview),
            )
    }

    fn ensure_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Viewer::Text { path, source, .. } = &self.viewer else {
            return;
        };
        if self.editors.contains_key(path) {
            return;
        }
        let path = path.clone();
        let draft = self.file_drafts.entry(path.clone()).or_insert_with(|| {
            crate::workspace_files::FileDraft {
                original: source.clone(),
                text: source.clone(),
            }
        });
        if let Viewer::Text {
            markdown: Some(markdown),
            ..
        } = &mut self.viewer
        {
            *markdown = draft.text.clone().into();
        }
        let language = match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "rs" => "rust",
            "js" | "jsx" => "javascript",
            "ts" | "tsx" => "typescript",
            "json" => "json",
            "py" => "python",
            "md" => "markdown",
            "sh" => "bash",
            "css" => "css",
            "html" => "html",
            "toml" => "toml",
            _ => "plaintext",
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .soft_wrap(false)
                .default_value(draft.text.clone())
        });
        self.watch_editor(path, editor, cx);
    }

    fn watch_editor(&mut self, path: PathBuf, editor: Entity<EditorState>, cx: &mut Context<Self>) {
        let edited = path.clone();
        cx.subscribe(&editor, move |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.editors.get(&edited) == Some(&editor) {
                let text = editor.read(cx).text().to_string();
                if let Some(draft) = this.file_drafts.get_mut(&edited) {
                    draft.text = text.clone();
                }
                if let Viewer::Text {
                    path,
                    markdown: Some(markdown),
                    ..
                } = &mut this.viewer
                    && path == &edited
                {
                    *markdown = text.into();
                }
                cx.notify();
            }
        })
        .detach();
        self.editors.insert(path, editor);
    }

    fn selected_path(&self) -> Option<PathBuf> {
        match &self.viewer {
            Viewer::Text { path, .. } => Some(path.clone()),
            _ => None,
        }
    }
    fn selected_dirty(&self) -> bool {
        self.selected_path()
            .and_then(|path| self.file_drafts.get(&path))
            .is_some_and(|draft| draft.text != draft.original)
    }
    fn save_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path() else {
            return;
        };
        let Some(draft) = self.file_drafts.get(&path).cloned() else {
            return;
        };
        if draft.text == draft.original || !self.saving.insert(path.clone()) {
            return;
        }
        self.file_error = None;
        let target = path.clone();
        let text = draft.text.clone();
        let save = cx.background_executor().spawn(async move {
            crate::workspace_files::save_file(&target, &draft.original, &draft.text)
        });
        cx.spawn(async move |this, cx| {
            let result = save.await;
            let _ = this.update(cx, |this, cx| {
                this.saving.remove(&path);
                match result {
                    Ok(()) => {
                        if let Some(draft) = this.file_drafts.get_mut(&path) {
                            draft.original = text;
                        }
                        this.file_error = None;
                        this.refresh_changes(cx);
                    }
                    Err(error) => this.file_error = Some(format!("{}: {error}", path.display())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn revert_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path() else {
            return;
        };
        if self.saving.contains(&path) {
            return;
        }
        if let Some(draft) = self.file_drafts.get_mut(&path) {
            draft.text = draft.original.clone();
            if let Some(editor) = self.editors.get(&path) {
                editor.update(cx, |editor, cx| {
                    editor.set_value(draft.text.clone(), window, cx)
                });
            }
            if let Viewer::Text {
                markdown: Some(markdown),
                ..
            } = &mut self.viewer
            {
                *markdown = draft.text.clone().into();
            }
        }
        self.file_error = None;
        cx.notify();
    }
    fn reload_selected(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path() else {
            return;
        };
        if self.selected_dirty() {
            self.file_error =
                Some("Unsaved edits are kept. Save or Revert them before reloading.".into());
            cx.notify();
            return;
        }
        self.editors.remove(&path);
        self.file_drafts.remove(&path);
        self.open_file(path, cx);
    }
    fn new_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = uuid::Uuid::new_v4().to_string();
        if self.open_panes.contains(&Pane::Terminal) {
            self.terminal_tabs.push(id);
        } else {
            self.terminal_tabs = vec![id];
        }
        self.terminal_active = self.terminal_tabs.len() - 1;
        self.open_tool(Pane::Terminal, window, cx);
    }
    fn close_terminal_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_tabs.len() == 1 {
            self.close_tool(Pane::Terminal, window, cx);
            return;
        }
        let id = self.terminal_tabs.remove(index);
        if let Some(terminal) = self.terminals.remove(&id) {
            terminal.update(cx, |terminal, cx| terminal.close(cx));
        }
        self.terminal_active = if index < self.terminal_active {
            self.terminal_active - 1
        } else {
            self.terminal_active
        }
        .min(self.terminal_tabs.len() - 1);
        if self.pane == Pane::Terminal {
            self.ensure_terminal(window, cx);
            self.focus_terminal(window, cx);
        }
        cx.notify();
    }
    fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .when_some(self.terminal_error.clone(), |view, error| {
                view.child(
                    div()
                        .p_2()
                        .text_size(zpx(TEXT_SM))
                        .whitespace_normal()
                        .child(error),
                )
            })
            .child(div().flex_1().min_h_0().child(self.terminal_body(cx)))
            .into_any_element()
    }

    fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        if self.changes_loading {
            return;
        }
        self.changes_loading = true;
        self.change_request += 1;
        let request = self.change_request;
        let root = self.root.clone();
        let work = cx.background_executor().spawn(async move {
            let (root, mut files) = crate::workspace_files::changes(&root)?;
            let omitted = files.len().saturating_sub(MAX_REVIEW_FILES);
            files.truncate(MAX_REVIEW_FILES);
            let head = crate::workspace_files::has_head(&root);
            let diffs = files
                .iter()
                .map(|file| {
                    (
                        file.path.clone(),
                        crate::workspace_files::diff(&root, file, head)
                            .unwrap_or_else(|error| format!("Unable to read diff: {error}")),
                    )
                })
                .collect::<HashMap<_, _>>();
            Ok::<_, String>((root, files, omitted, diffs))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if request != this.change_request {
                    return;
                }
                this.changes_loading = false;
                // "Loading changes…" shows only while the list is empty.
                let mut changed = this.change_files.is_empty();
                match result {
                    Ok((root, files, omitted, diffs)) => {
                        if this.change_error.is_none()
                            && this.change_root == root
                            && this.change_files == files
                            && this.change_omitted == omitted
                            && this.change_diffs == diffs
                        {
                            // The 2-second poll found nothing new.
                        } else {
                            changed = true;
                            this.change_reviews = diffs
                                .iter()
                                .map(|(path, text)| {
                                    let review = match this.change_diffs.get(path) {
                                        Some(old) if old == text => {
                                            this.change_reviews.get(path).cloned()
                                        }
                                        _ => None,
                                    };
                                    (
                                        path.clone(),
                                        review
                                            .unwrap_or_else(|| Rc::new(ChangeReview::parse(text))),
                                    )
                                })
                                .collect();
                            this.change_root = root;
                            this.change_error = None;
                            this.change_expanded.retain(|path| diffs.contains_key(path));
                            this.change_files = files;
                            this.change_omitted = omitted;
                            this.change_diffs = diffs;
                        }
                    }
                    Err(error) => {
                        changed |= this.change_error.as_ref() != Some(&error);
                        this.change_error = Some(error);
                        this.change_files.clear();
                        this.change_diffs.clear();
                        this.change_reviews.clear();
                        this.change_omitted = 0;
                        this.change_expanded.clear();
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
        if self.change_files.is_empty() {
            cx.notify();
        }
    }
    fn render_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (total_added, total_removed) = self
            .change_reviews
            .values()
            .fold((0usize, 0usize), |(added, removed), review| {
                (added + review.added, removed + review.removed)
            });
        let shown = self.change_files.len();
        let total_files = shown + self.change_omitted;
        v_flex()
            .id("changes-review")
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .when_some(self.change_error.clone(), |view, error| {
                view.child(
                    div()
                        .p_3()
                        .whitespace_normal()
                        .text_size(zpx(TEXT_MD))
                        .child(error),
                )
            })
            .when(
                self.change_files.is_empty() && self.change_error.is_none(),
                |view| {
                    view.child(
                        div()
                            .p_3()
                            .text_size(zpx(TEXT_MD))
                            .child(if self.changes_loading {
                                "Loading changes…"
                            } else {
                                "Working tree clean"
                            }),
                    )
                },
            )
            .when(!self.change_files.is_empty(), |view| {
                view.child(
                    h_flex()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .text_size(zpx(TEXT_SM))
                        .text_color(rgba(0xffffff8c))
                        .child(format!(
                            "{} {}",
                            total_files,
                            if total_files == 1 { "file" } else { "files" }
                        ))
                        .child(
                            div()
                                .text_color(rgb(0x5fc27e))
                                .child(format!("+{total_added}")),
                        )
                        .child(
                            div()
                                .text_color(rgb(0xf0716b))
                                .child(format!("−{total_removed}")),
                        ),
                )
            })
            .children(self.change_files.iter().enumerate().map(|(index, file)| {
                let path = file.path.clone();
                let expanded = self.change_expanded.contains(&path);
                let review = self.change_reviews.get(&path).cloned();
                let (added, removed) = review.as_ref().map_or((0, 0), |r| (r.added, r.removed));
                v_flex()
                    .w_full()
                    .flex_none()
                    .min_w_0()
                    .child(
                        h_flex()
                            .id(("changed-file", index))
                            .px_3()
                            .py_2()
                            .gap_2()
                            .border_t_1()
                            .border_color(rgba(0xffffff14))
                            .cursor_pointer()
                            .text_size(zpx(TEXT_MD))
                            .role(gpui::Role::Button)
                            .aria_label(format!(
                                "{} {} +{} -{}",
                                if expanded { "Collapse" } else { "Expand" },
                                path.display(),
                                added,
                                removed
                            ))
                            .hover(|row| row.bg(rgba(0xffffff0a)))
                            .child(
                                crate::icons::icon(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .with_size(zpx(12.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(path.display().to_string()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgb(0x5fc27e))
                                    .child(format!("+{added}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgb(0xf0716b))
                                    .child(format!("-{removed}")),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.change_expanded.remove(&path) {
                                    this.change_expanded.insert(path.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when_some(review.filter(|_| expanded), |view, review| {
                        let hidden = review.lines.len().saturating_sub(MAX_REVIEW_LINES);
                        view.child(
                            v_flex()
                                .id(("file-diff", index))
                                .overflow_x_scroll()
                                .max_h(zpx(600.))
                                .overflow_y_scroll()
                                .children(review.lines.iter().take(MAX_REVIEW_LINES).map(|line| {
                                    let fill = match line.kind {
                                        '+' => 0x45cf7845,
                                        '-' => 0xff6b6b45,
                                        _ => 0x00000000,
                                    };
                                    h_flex()
                                        .px_2()
                                        .gap_2()
                                        .flex_none()
                                        .bg(rgba(fill))
                                        .font_family(MONO)
                                        .text_size(zpx(TEXT_SM))
                                        .line_height(relative(1.6))
                                        .child(
                                            div()
                                                .w(zpx(32.))
                                                .flex_none()
                                                .text_color(rgba(0xffffff70))
                                                .child(
                                                    line.old
                                                        .map(|n| n.to_string())
                                                        .unwrap_or_default(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .w(zpx(32.))
                                                .flex_none()
                                                .text_color(rgba(0xffffff70))
                                                .child(
                                                    line.new
                                                        .map(|n| n.to_string())
                                                        .unwrap_or_default(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .child(format!("{} {}", line.kind, line.text)),
                                        )
                                }))
                                .when(hidden > 0, |list| {
                                    list.child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .flex_none()
                                            .text_size(zpx(TEXT_SM))
                                            .text_color(rgba(0xffffff8c))
                                            .child(format!(
                                                "{hidden} more {} not shown",
                                                if hidden == 1 { "line" } else { "lines" }
                                            )),
                                    )
                                }),
                        )
                    })
            }))
            .when(self.change_omitted > 0, |view| {
                view.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(rgba(0xffffff14))
                        .text_size(zpx(TEXT_SM))
                        .text_color(rgba(0xffffff8c))
                        .child(format!(
                            "{} more changed {} not shown",
                            self.change_omitted,
                            if self.change_omitted == 1 {
                                "file"
                            } else {
                                "files"
                            }
                        )),
                )
            })
            .into_any_element()
    }

    fn terminal_body(&self, cx: &App) -> AnyElement {
        if let Some(terminal) = self
            .terminals
            .get(&self.terminal_tabs[self.terminal_active])
        {
            return div()
                .size_full()
                .min_h_0()
                .child(terminal.read(cx).terminal_view())
                .into_any_element();
        }
        div()
            .p_3()
            .text_size(zpx(TEXT_MD))
            .text_color(rgba(0xffffff8c))
            .child(if self.terminal_error.is_some() {
                "Terminal unavailable. Switch back to Terminal to retry."
            } else {
                "Opening terminal…"
            })
            .into_any_element()
    }
}

impl Render for RightPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.set_preview_visible(self.preview_visible, cx);
        if self.tab_drag.is_some() && !cx.has_active_drag() {
            self.tab_drag = None;
        }

        if (!self.preview_visible
            || self.pane != Pane::Files
            || !matches!(self.viewer, Viewer::Document { .. }))
            && let Some(preview) = &self.document_preview
        {
            preview.hide();
        }
        if (!self.preview_visible || self.active_hosted.is_some() || self.pane != Pane::Browser)
            && let Some(browser) = &self.browser
        {
            browser.hide();
        }
        let show_hidden = self.show_hidden;
        let transfer_bounds = self.transfer_bounds.clone();
        let body = if let Some(tab) = self
            .hosted
            .iter()
            .find(|tab| Some(&tab.id) == self.active_hosted.as_ref())
        {
            div()
                .size_full()
                .child(tab.panel.clone())
                .into_any_element()
        } else {
            match self.pane {
                Pane::Files => self.render_files(window, cx).into_any_element(),
                Pane::Terminal => self.render_terminal(cx),
                Pane::Browser => self.render_browser(cx),
                Pane::Changes => self.render_changes(window, cx),
                Pane::SideChat => div()
                    .size_full()
                    .when_some(self.side_chat.clone(), |v, side| v.child(side))
                    .into_any_element(),
                Pane::NewTab => v_flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .child(crate::new_tab::address_bar(&self.address))
                    .child(crate::new_tab::tools(
                        self.compact_tools,
                        &self.new_tab_scroll,
                        Self::choose_new_tool,
                        cx,
                    ))
                    .into_any_element(),
            }
        };
        let mut tab_elements = Vec::new();
        let keys = self.sidebar_keys();
        let visuals = keys
            .iter()
            .filter_map(|key| {
                let pane = match key {
                    SidebarTab::Hosted(id) => self
                        .hosted
                        .iter()
                        .find(|tab| &tab.id == id)
                        .map(|tab| tab.panel.read(cx).pane),
                    _ => self
                        .sidebar_tabs()
                        .into_iter()
                        .find(|(pane, instance)| self.tab_key(*pane, *instance) == *key)
                        .map(|(pane, _)| pane),
                }?;
                Some((
                    key.clone(),
                    crate::tab_motion::Visual {
                        label: pane.tab_label().1.into(),
                        icon: pane.icon(),
                        active: self
                            .active_hosted
                            .as_ref()
                            .map(|id| SidebarTab::Hosted(id.clone()))
                            .unwrap_or_else(|| {
                                self.tab_key(self.pane, self.pane_instance(self.pane))
                            })
                            == *key,
                    },
                ))
            })
            .collect();
        let was_transitioning = self.motion.transitioning();
        let slots = self.motion.sync(visuals, cx);
        let active_key = self.active_hosted.as_ref().map_or_else(
            || self.tab_key(self.pane, self.pane_instance(self.pane)),
            |id| SidebarTab::Hosted(id.clone()),
        );
        let active_changed = self.revealed_tab.as_ref() != Some(&active_key);
        self.revealed_tab = Some(active_key.clone());
        if active_changed && !self.workspace {
            // Restored panels can first render before their scroll viewport has
            // bounds. Repeat once after layout instead of leaving the active tab hidden.
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.reveal_active_tab();
                    cx.notify();
                });
            })
            .detach();
        }
        // Re-reveal after expansion as well: the initial scroll request sees
        // a nearly zero-width tab and cannot reveal its final bounds yet.
        if !cx.has_active_drag()
            && (was_transitioning || self.motion.transitioning() || active_changed)
            && let Some(index) = slots
                .iter()
                .position(|slot| !slot.closing && slot.key == active_key)
        {
            self.tool_tab_scroll.scroll_to_item(index);
        }
        for slot in &slots {
            let key = &slot.key;
            if slot.closing {
                tab_elements.push(slot.ghost(format!("sidebar-close-{key:?}").into()));
                continue;
            }
            let tab = if let SidebarTab::Hosted(id) = key {
                self.hosted
                    .iter()
                    .find(|tab| &tab.id == id)
                    .map(|tab| self.tab(tab.panel.read(cx).pane, None, Some(id.clone()), cx))
            } else {
                self.sidebar_tabs()
                    .into_iter()
                    .find(|(pane, instance)| &self.tab_key(*pane, *instance) == key)
                    .map(|(pane, instance)| self.tab(pane, instance, None, cx))
            };
            if let Some(tab) = tab {
                tab_elements.push(slot.wrap(format!("sidebar-open-{key:?}").into(), tab));
            }
        }
        if self.workspace {
            return v_flex()
                .track_focus(&self.focus)
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(body)
                .into_any_element();
        }
        v_flex()
            .track_focus(&self.focus)
            .bg(crate::appearance::shell_surface())
            .size_full()
            .min_w_0()
            .overflow_hidden()
            // Nothing wraps while the panel's width animates.
            .whitespace_nowrap()
            .flex_none()
            .child(
                h_flex()
                    .relative()
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                transfer_bounds.set(bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .bg(crate::appearance::shell_surface())
                    .id("sidebar-transfer-zone")
                    .debug_selector(|| "sidebar-transfer-zone".into())
                    .h(zpx(crate::icons::TOOL_ROW_HEIGHT))
                    .flex_none()
                    .overflow_hidden()
                    .px_2()
                    .gap_0p5()
                    .items_center()
                    .child(
                        h_flex()
                            .relative()
                            .min_w_0()
                            .flex_shrink_1()
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .id("sidebar-tool-tabs")
                                    .debug_selector(|| "sidebar-tool-tabs".into())
                                    .on_drag_move(cx.listener(
                                        |this, event: &DragMoveEvent<DraggedSidebarTab>, _, cx| {
                                            let drag = event.drag(cx).clone();
                                            this.move_tab_drag(
                                                drag.owner,
                                                &drag.key,
                                                event.event.position,
                                                cx,
                                            );
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.finish_tab_drag(cx)),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.finish_tab_drag(cx)),
                                    )
                                    .on_drop(cx.listener(
                                        |this, drag: &DraggedSidebarTab, _, cx| {
                                            if drag.owner == cx.entity_id() {
                                                this.finish_tab_drag(cx);
                                            }
                                        },
                                    ))
                                    .track_scroll(&self.tool_tab_scroll)
                                    .flex_shrink_1()
                                    .min_w_0()
                                    .overflow_x_scroll()
                                    .gap_0p5()
                                    .children(tab_elements),
                            )
                            .children(crate::tab_transfer::edge_fades(
                                &self.tool_tab_scroll,
                                crate::appearance::shell_surface(),
                            )),
                    )
                    .child(
                        div()
                            .id("new-sidebar-tab")
                            .debug_selector(|| "new-sidebar-tab".into())
                            .role(gpui::Role::Button)
                            .aria_label("New sidebar tab")
                            .size(zpx(crate::icons::TOOL_TAB_HEIGHT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_none()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|button| button.bg(rgba(0xffffff14)))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_new_tab(window, cx)),
                            )
                            .child(crate::icons::icon(IconName::Plus)),
                    )
                    .when(
                        self.pane == Pane::Files && self.active_hosted.is_none(),
                        |row| {
                            row.child(
                                h_flex()
                                    .id("show-hidden-files")
                                    .ml_auto()
                                    .gap(zpx(5.0))
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|control| control.opacity(0.85))
                                    .text_size(zpx(TEXT_SM))
                                    .text_color(rgba(0xffffff8c))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_hidden = !this.show_hidden;
                                        this.directory_entries.clear();
                                        this.refresh_tree(cx);
                                        cx.notify();
                                    }))
                                    .child(".files")
                                    .child(
                                        div()
                                            .relative()
                                            .w(zpx(22.0))
                                            .h(zpx(12.0))
                                            .rounded_full()
                                            .bg(if show_hidden {
                                                ActionTone::Accent.foreground()
                                            } else {
                                                rgba(0xffffff20)
                                            })
                                            .child(
                                                div()
                                                    .absolute()
                                                    .top(zpx(2.0))
                                                    .size(zpx(8.0))
                                                    .rounded_full()
                                                    .bg(rgba(0xffffffe6))
                                                    .with_animation(
                                                        SharedString::from(format!(
                                                            "hidden-files-thumb-{}",
                                                            show_hidden
                                                        )),
                                                        Animation::new(
                                                            std::time::Duration::from_millis(120),
                                                        )
                                                        .with_easing(ease_out_quint()),
                                                        move |thumb, t| {
                                                            let (from, to) = if show_hidden {
                                                                (2.0, 12.0)
                                                            } else {
                                                                (12.0, 2.0)
                                                            };
                                                            thumb.left(zpx(from + (to - from) * t))
                                                        },
                                                    ),
                                            ),
                                    ),
                            )
                        },
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .rounded(zpx(del_core::ShellMetrics::default().page_radius))
                    .bg(crate::appearance::page_surface())
                    .child(body),
            )
            .into_any_element()
    }
}

fn action(id: &'static str, label: &'static str, tone: ActionTone) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .px_1p5()
        .h(zpx(28.))
        .flex()
        .items_center()
        .flex_none()
        .rounded_md()
        .text_size(zpx(TEXT_SM))
        .cursor_pointer()
        .bg(tone.surface(false))
        .text_color(tone.foreground())
        .hover(move |button| button.bg(tone.surface(true)))
        .child(label)
}

fn read_viewer(path: PathBuf) -> Viewer {
    if crate::document_preview::supports(&path) && path.is_file() {
        return Viewer::Document { path };
    }
    match std::fs::metadata(&path) {
        // Devices, FIFOs and sockets report no useful size and can block or
        // stream forever; only regular files are previewed.
        Ok(m) if !m.is_file() => Viewer::Note {
            path,
            message: "Not a regular file".into(),
        },
        Ok(m) if m.len() > MAX_FILE_BYTES => Viewer::Note {
            path,
            message: format!("{} KB: too large to show", m.len() / 1024),
        },
        Ok(_) => match read_capped(&path, MAX_FILE_BYTES) {
            Ok(None) => Viewer::Note {
                path,
                message: "File grew too large to show".into(),
            },
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let markdown = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                        })
                        .then(|| SharedString::from(text.clone()));
                    Viewer::Text {
                        source: text,
                        markdown,
                        path,
                    }
                }
                Err(_) => Viewer::Note {
                    path,
                    message: "Binary file".into(),
                },
            },
            Err(e) => Viewer::Note {
                path,
                message: e.to_string(),
            },
        },
        Err(e) => Viewer::Note {
            path,
            message: e.to_string(),
        },
    }
}

/// Read at most `max` bytes; `None` when the file holds more (it may have
/// grown since its size was checked).
fn read_capped(path: &Path, max: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

#[cfg(test)]
mod tests {
    fn legacy_panel(
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<RightPanel>,
    ) -> RightPanel {
        let mut panel = RightPanel::new(root, window, cx);
        panel.pane = Pane::Files;
        panel.open_panes = vec![Pane::Files, Pane::Changes, Pane::Terminal];
        panel.blank_tabs.clear();
        panel
    }

    #[::gpui::test]
    fn fresh_sidebar_has_only_one_chooser_and_cycles_without_losing_drafts(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                assert_eq!(panel.open_panes, vec![Pane::NewTab]);
                assert_eq!(panel.sidebar_keys().len(), 1);
                assert!(panel.terminals.is_empty());
                assert!(panel.browsers.is_empty());
                panel
                    .address
                    .update(cx, |input, cx| input.set_value("first draft", window, cx));
                panel.open_new_tab(window, cx);
                panel
                    .address
                    .update(cx, |input, cx| input.set_value("second draft", window, cx));
                panel.select_relative(-1, window, cx);
                assert_eq!(panel.address.read(cx).value().as_ref(), "first draft");
                panel.select_relative(1, window, cx);
                assert_eq!(panel.address.read(cx).value().as_ref(), "second draft");
                panel.close_active_tab(window, cx);
                panel.close_active_tab(window, cx);
                assert_eq!(panel.sidebar_keys().len(), 1);
                assert_eq!(panel.pane, Pane::NewTab);
                let saved = panel.snapshot(true, cx);
                panel.restore(saved, window, cx);
                assert_eq!(panel.sidebar_keys().len(), 1);
                assert_eq!(panel.open_panes, vec![Pane::NewTab]);
            })
        });
    }

    #[test]
    fn older_sidebar_state_uses_default_tool_tabs() {
        let saved: SavedPanel = serde_json::from_str(r#"{"open":true,"pane":"Files"}"#).unwrap();
        assert!(saved.open_panes == default_panes());
        let empty: SavedPanel =
            serde_json::from_str(r#"{"open":true,"pane":"NewTab","open_panes":[]}"#).unwrap();
        assert!(empty.open_panes.is_empty());
        assert!(empty.restore_open());
    }

    #[::gpui::test]
    fn sidebar_mouse_drag_keeps_instances_selection_and_saved_order(cx: &mut gpui::TestAppContext) {
        struct Fixture(Entity<RightPanel>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(zpx(1000.)).h(zpx(600.)).child(self.0.clone())
            }
        }
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut panel = legacy_panel(dir.path().into(), window, cx);
                panel.terminal_tabs.push(uuid::Uuid::new_v4().to_string());
                panel.open_new_tab(window, cx);
                panel.open_new_tab(window, cx);
                panel.address.update(cx, |input, cx| {
                    input.set_value("keep-this-draft", window, cx)
                });
                panel.remember_blank(cx);
                panel
            });
            panel = Some(view.clone());
            cx.observe(&view, |_, _, cx| cx.notify()).detach();
            Fixture(view)
        });
        let panel = panel.unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let original = panel.read_with(cx, |panel, _| panel.sidebar_keys());
        let (blank, terminal) = panel.read_with(cx, |panel, _| {
            (
                format!("tab-new-{}", panel.blank_tabs[1].id),
                format!("tab-terminal-{}", panel.terminal_tabs[1]),
            )
        });
        let from = cx
            .debug_bounds(Box::leak(blank.into_boxed_str()))
            .unwrap()
            .center();
        let to = cx.debug_bounds("tab-files").unwrap().center() - point(zpx(8.), zpx(0.));
        cx.simulate_mouse_move(from, None, Modifiers::default());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let mut expected = original.clone();
        let last = expected.pop().unwrap();
        expected.insert(0, last);
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.sidebar_keys(), expected);
            assert_eq!(panel.pane, Pane::NewTab);
            assert_eq!(panel.blank_active, 1);
            assert_eq!(panel.address.read(cx).value().as_ref(), "keep-this-draft");
        });
        // A terminal instance can cross tool kinds without moving its sibling.
        let from = cx
            .debug_bounds(Box::leak(terminal.into_boxed_str()))
            .unwrap()
            .center();
        let to = cx.debug_bounds("tab-files").unwrap().center() - point(zpx(8.), zpx(0.));
        cx.simulate_mouse_move(from, None, Modifiers::default());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            from + point(zpx(12.), zpx(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_up(
            to + point(zpx(0.), zpx(100.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        let moved = expected.remove(4);
        expected.insert(1, moved);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                assert_eq!(panel.sidebar_keys(), expected);
                assert_eq!(panel.pane, Pane::NewTab);
                assert_eq!(panel.terminal_tabs.len(), 2);
                let saved: SavedPanel =
                    serde_json::from_slice(&serde_json::to_vec(&panel.snapshot(true, cx)).unwrap())
                        .unwrap();
                panel.restore(saved, window, cx);
                assert_eq!(panel.sidebar_keys(), expected);
                assert_eq!(panel.address.read(cx).value().as_ref(), "keep-this-draft");
                panel.open_new_tab(window, cx);
                assert_eq!(&panel.sidebar_keys()[..expected.len()], expected.as_slice());
                panel.close_blank_tab(0, window, cx);
                expected.retain(|key| key != &original[4]);
                assert_eq!(&panel.sidebar_keys()[..expected.len()], expected.as_slice());
                panel.select_blank_tab(0, window, cx);
                panel.open_panes.retain(|pane| *pane != Pane::Changes);
                panel.choose_new_tool(Pane::Changes, window, cx);
                assert_eq!(panel.sidebar_keys()[0], SidebarTab::Tool(Pane::Changes));
            })
        });
    }

    #[::gpui::test]
    fn transferring_tabs_keeps_live_shells_and_editor_entities(cx: &mut gpui::TestAppContext) {
        use crate::workspace_tabs::WorkspaceTabs;
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("move.md");
        std::fs::write(&path, "original").unwrap();
        let (right, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.viewer = read_viewer(path.clone());
            panel
        });
        cx.run_until_parked();
        let editor = right.read_with(cx, |right, _| right.editors[&path].clone());
        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.replace_all("unsaved draft", window, cx)
            })
        });
        cx.run_until_parked();
        let mut file = None;
        cx.update(|window, cx| {
            let workspace = cx.new(|cx| WorkspaceTabs::new(dir.path().into(), window, cx));
            let panel = right.update(cx, |right, cx| {
                right
                    .take_tab(&SidebarTab::Tool(Pane::Files), window, cx)
                    .unwrap()
            });
            assert_eq!(panel.read(cx).editors[&path], editor);
            assert_eq!(panel.read(cx).file_drafts[&path].text, "unsaved draft");
            assert!(right.read(cx).file_drafts.is_empty());
            workspace.update(cx, |workspace, cx| {
                workspace.accept_tab(panel.clone(), window, cx)
            });
            let moved = workspace.update(cx, |workspace, cx| {
                workspace.take_tab(1, window, cx).unwrap()
            });
            assert_eq!(moved, panel);
            right.update(cx, |right, cx| right.accept_tab(moved, window, cx));
            file = Some(panel);
            right.update(cx, |right, cx| {
                right.new_terminal_tab(window, cx);
                right.new_terminal_tab(window, cx);
            });
            let (key, shell, sibling) = {
                let right = right.read(cx);
                (
                    right.tab_key(Pane::Terminal, Some(1)),
                    right.terminals[&right.terminal_tabs[1]].clone(),
                    right.terminals[&right.terminal_tabs[2]].clone(),
                )
            };
            let pid = shell.read(cx).snapshot().pid;
            let panel = right.update(cx, |right, cx| right.take_tab(&key, window, cx).unwrap());
            assert_eq!(panel.read(cx).terminals.values().next().unwrap(), &shell);
            workspace.update(cx, |workspace, cx| {
                workspace.accept_tab(panel.clone(), window, cx)
            });
            let moved = workspace.update(cx, |workspace, cx| {
                workspace.take_tab(2, window, cx).unwrap()
            });
            right.update(cx, |right, cx| right.accept_tab(moved.clone(), window, cx));
            assert_eq!(moved, panel);
            assert_eq!(shell.read(cx).snapshot().pid, pid);
            assert_eq!(
                shell.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Running
            );
            assert_eq!(
                sibling.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Running
            );
            // The original editor still emits into its current owner after moving twice.
            editor.update(cx, |editor, cx| {
                editor.replace_all("edited after transfer", window, cx)
            });
        });
        cx.run_until_parked();
        file.unwrap().read_with(cx, |file, _| {
            assert_eq!(file.file_drafts[&path].text, "edited after transfer")
        });
        right.read_with(cx, |right, cx| {
            assert!(right.file_drafts.is_empty());
            let saved: SavedPanel =
                serde_json::from_slice(&serde_json::to_vec(&right.snapshot(true, cx)).unwrap())
                    .unwrap();
            assert_eq!(saved.hosted.len(), 2);
            assert!(saved.hosted.iter().any(|tab| {
                tab.panel
                    .file_drafts
                    .get(&path)
                    .is_some_and(|draft| draft.text == "edited after transfer")
            }));
        });
        right.update(cx, |right, cx| right.release(cx));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }

    #[::gpui::test]
    fn moving_side_chats_keeps_identity_and_forwards_events_once(cx: &mut gpui::TestAppContext) {
        use crate::workspace_tabs::WorkspaceTabs;
        struct Fixture {
            right: Entity<RightPanel>,
            workspace: Entity<WorkspaceTabs>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().child(self.right.clone())
            }
        }
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let agent: std::sync::Arc<dyn del_agent::AgentBackend> =
            std::sync::Arc::new(del_agent::MockBackend::new(del_agent::Provider::Codex));
        let manager = del_agent::SessionManager::new(dir.path().join("index"), [agent]).unwrap();
        let parent = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider: del_agent::Provider::Codex,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: del_agent::PermissionMode::Plan,
            }))
            .unwrap();
        let context = crate::side_chat::Context {
            activity_owner: None,
            backend: crate::chat::Backend {
                runtime: runtime.handle().clone(),
                manager,
            },
            source: del_agent::ForkSource::Session(parent.id.clone()),
            accounts: vec![],
        };
        let right_count = Rc::new(Cell::new(0));
        let workspace_count = Rc::new(Cell::new(0));
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let right = cx.new(|cx| {
                let mut right = legacy_panel(dir.path().into(), window, cx);
                right.set_side_context(context.clone());
                right.open_tool(Pane::SideChat, window, cx);
                right
            });
            let workspace = cx.new(|cx| {
                let mut workspace = WorkspaceTabs::new(dir.path().into(), window, cx);
                workspace.set_side_context(context);
                workspace
            });
            let count = right_count.clone();
            cx.subscribe(&right, move |_, _, _: &crate::side_chat::SendToMain, _| {
                count.set(count.get() + 1)
            })
            .detach();
            let count = workspace_count.clone();
            cx.subscribe(
                &workspace,
                move |_, _, _: &crate::side_chat::SendToMain, _| count.set(count.get() + 1),
            )
            .detach();
            Fixture { right, workspace }
        });
        let (right, workspace) = fixture.read_with(cx, |fixture, _| {
            (fixture.right.clone(), fixture.workspace.clone())
        });
        let side = right.read_with(cx, |right, _| right.side_chat.clone().unwrap());
        let storage_id = side.read_with(cx, |side, _| side.storage_id());
        for _ in 0..200 {
            cx.run_until_parked();
            if side.read_with(cx, |side, _| side.session_id().is_some()) {
                break;
            }
            runtime
                .block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        }
        let peer = side.read_with(cx, |side, _| side.session_id().unwrap().clone());
        cx.update(|window, cx| {
            right.update(cx, |right, cx| {
                right.open_tool(Pane::Files, window, cx);
                assert!(right.show_peer(&peer, window, cx));
                assert_eq!(right.pane, Pane::SideChat);
                assert!(
                    !right.workspace,
                    "peer navigation must retain the sidebar tab strip"
                );
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("tab-side-chat").is_some());
        cx.update(|window, cx| {
            let panel = right.update(cx, |right, cx| {
                right
                    .take_tab(&SidebarTab::Tool(Pane::SideChat), window, cx)
                    .unwrap()
            });
            for id in 1..=3 {
                workspace.update(cx, |workspace, cx| {
                    workspace.accept_tab(panel.clone(), window, cx)
                });
                let moved = workspace.update(cx, |workspace, cx| {
                    workspace.take_tab(id, window, cx).unwrap()
                });
                assert_eq!(moved, panel);
                right.update(cx, |right, cx| right.accept_tab(moved, window, cx));
                if id < 3 {
                    right.update(cx, |right, cx| {
                        let key = SidebarTab::Hosted(right.hosted[0].id.clone());
                        assert_eq!(right.take_tab(&key, window, cx).unwrap(), panel);
                    });
                }
            }
            assert_eq!(panel.read(cx).side_chat.as_ref(), Some(&side));
            assert_eq!(side.read(cx).storage_id(), storage_id);
            side.update(cx, |_, cx| {
                cx.emit(crate::side_chat::SendToMain {
                    parent: parent.id.clone(),
                    text: "once".into(),
                })
            });
        });
        cx.run_until_parked();
        assert_eq!(right_count.get(), 1);
        assert_eq!(workspace_count.get(), 0);
        cx.update(|window, cx| {
            let panel = right.update(cx, |right, cx| {
                let key = SidebarTab::Hosted(right.hosted[0].id.clone());
                right.take_tab(&key, window, cx).unwrap()
            });
            workspace.update(cx, |workspace, cx| workspace.accept_tab(panel, window, cx));
            side.update(cx, |_, cx| {
                cx.emit(crate::side_chat::SendToMain {
                    parent: parent.id.clone(),
                    text: "once again".into(),
                })
            });
        });
        cx.run_until_parked();
        assert_eq!(right_count.get(), 1);
        assert_eq!(workspace_count.get(), 1);
        workspace.update(cx, |workspace, cx| workspace.release(cx));
        right.update(cx, |right, cx| right.release(cx));
    }

    #[::gpui::test]
    fn restoring_an_empty_sidebar_creates_one_visible_blank_tab(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            let saved =
                serde_json::from_str(r#"{"open":true,"pane":"NewTab","open_panes":[]}"#).unwrap();
            panel.restore(saved, window, cx);
            assert!(panel.pane == Pane::NewTab);
            assert!(panel.open_panes == vec![Pane::NewTab]);
            assert_eq!(panel.blank_tabs.len(), 1);
            panel.restore(panel.snapshot(true, cx), window, cx);
            assert_eq!(panel.blank_tabs.len(), 1);
            panel
        });
    }

    #[::gpui::test]
    fn blank_tabs_keep_identity_and_drafts_until_explicitly_closed(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            assert!(panel.blank_tabs.is_empty());
            for index in 0..8 {
                panel.open_new_tab(window, cx);
                panel.address.update(cx, |input, cx| {
                    input.set_value(format!("draft-{index}.example"), window, cx)
                });
            }
            panel.open_tool(Pane::Files, window, cx);
            assert_eq!(panel.blank_tabs.len(), 8);
            assert_eq!(panel.sidebar_tabs().len(), 11);
            let ids: Vec<_> = panel.blank_tabs.iter().map(|tab| tab.id.clone()).collect();
            for index in 0..8 {
                panel.select_blank_tab(index, window, cx);
                assert_eq!(
                    panel.address.read(cx).value().as_ref(),
                    format!("draft-{index}.example")
                );
            }
            let saved: SavedPanel =
                serde_json::from_slice(&serde_json::to_vec(&panel.snapshot(true, cx)).unwrap())
                    .unwrap();
            panel.restore(saved, window, cx);
            assert!(panel.pane == Pane::NewTab);
            assert_eq!(panel.blank_active, 7);
            assert_eq!(panel.address.read(cx).value().as_ref(), "draft-7.example");
            assert_eq!(
                panel
                    .blank_tabs
                    .iter()
                    .map(|tab| tab.id.clone())
                    .collect::<Vec<_>>(),
                ids
            );
            panel.open_tool(Pane::Files, window, cx);
            panel.close_blank_tab(2, window, cx);
            assert!(panel.pane == Pane::Files);
            assert_eq!(panel.blank_tabs.len(), 7);
            assert_eq!(panel.blank_tabs[2].id, ids[3]);
            panel.select_blank_tab(6, window, cx);
            panel.close_blank_tab(6, window, cx);
            assert_eq!(panel.address.read(cx).value().as_ref(), "draft-6.example");
            panel.select_blank_tab(0, window, cx);
            panel.choose_new_tool(Pane::Terminal, window, cx);
            assert_eq!(panel.blank_tabs.len(), 5);
            assert_eq!(panel.blank_tabs[0].id, ids[1]);
            panel.release(cx);
            panel
        });
    }

    #[::gpui::test]
    fn sidebar_plus_follows_tabs_and_stays_reachable_when_they_overflow(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture {
            panel: Entity<RightPanel>,
            width: f32,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(zpx(self.width))
                    .h(zpx(600.))
                    .child(self.panel.clone())
            }
        }
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let mut panel = None;
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| legacy_panel(dir.path().into(), window, cx));
            panel = Some(view.clone());
            Fixture {
                panel: view,
                width: 700.,
            }
        });
        let panel = panel.unwrap();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tabs = cx.debug_bounds("sidebar-tool-tabs").unwrap();
        let plus = cx.debug_bounds("new-sidebar-tab").unwrap();
        assert!((plus.left() - tabs.right()).abs() <= zpx(4.));
        assert!(
            plus.right() < zpx(500.),
            "plus must not fill the available width"
        );
        cx.update(|window, cx| {
            fixture.update(cx, |fixture, cx| {
                fixture.width = PANEL_WIDTH;
                cx.notify();
            });
            panel.update(cx, |panel, cx| {
                for _ in 0..8 {
                    panel.open_new_tab(window, cx);
                }
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let plus = cx.debug_bounds("new-sidebar-tab").unwrap();
        assert!(plus.right() <= zpx(PANEL_WIDTH));
        panel.read_with(cx, |panel, _| {
            assert!(panel.tool_tab_scroll.max_offset().x > zpx(100.))
        });
        cx.simulate_mouse_move(plus.center(), None, Modifiers::default());
        cx.simulate_mouse_down(plus.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(plus.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| assert_eq!(panel.blank_tabs.len(), 9));
        // The final reveal must use expanded widths, not the near-zero bounds
        // from the frame when rapid tab creation began.
        std::thread::sleep(crate::tab_motion::ENTER + std::time::Duration::from_millis(20));
        panel.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        panel.read_with(cx, |panel, _| {
            let index = panel.sidebar_keys().len() - 1;
            let tab = panel.tool_tab_scroll.bounds_for_item(index).unwrap();
            let viewport = panel.tool_tab_scroll.bounds();
            let offset = panel.tool_tab_scroll.offset().x;
            assert!(tab.right() + offset <= viewport.right() + zpx(1.));
            assert!(tab.left() + offset >= viewport.left() - zpx(1.));
        });
    }

    #[::gpui::test]
    fn plus_creates_multiple_blank_tabs_and_hover_close_removes_only_one(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        // This test measures settled hit targets; motion has separate coverage.
        cx.update(|cx| cx.set_reduce_motion(true));
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        for _ in 0..3 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let target = cx.debug_bounds("new-sidebar-tab").unwrap().center();
            cx.simulate_mouse_move(target, None, Modifiers::default());
            cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
        }
        let ids = panel.read_with(cx, |panel, _| {
            assert_eq!(panel.blank_tabs.len(), 3);
            panel
                .blank_tabs
                .iter()
                .map(|tab| tab.id.clone())
                .collect::<Vec<_>>()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let close = cx.debug_bounds("close-tab-new-1").unwrap().center();
        cx.simulate_mouse_move(close, None, Modifiers::default());
        cx.simulate_mouse_down(close, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(close, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.blank_tabs.len(), 2);
            assert_eq!(panel.blank_tabs[0].id, ids[0]);
            assert_eq!(panel.blank_tabs[1].id, ids[2]);
            assert_eq!(panel.blank_active, 1);
        });
    }

    #[::gpui::test]
    fn closing_sidebar_tools_reaches_shared_launcher_and_restores_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                // Closing a background tool must leave Files selected.
                panel.close_tool(Pane::Browser, window, cx);
                assert!(panel.pane == Pane::Files);
                panel.close_tool(Pane::Terminal, window, cx);
                panel.close_tool(Pane::Changes, window, cx);
                panel.close_tool(Pane::Files, window, cx);
                assert!(panel.open_panes == vec![Pane::NewTab]);
                assert_eq!(panel.blank_tabs.len(), 1);
                assert!(panel.pane == Pane::NewTab);
                let first_blank = panel.blank_tabs[0].id.clone();
                panel.close_blank_tab(0, window, cx);
                assert_eq!(panel.blank_tabs.len(), 1);
                assert_ne!(panel.blank_tabs[0].id, first_blank);
                let saved = serde_json::from_str(
                    &serde_json::to_string(&panel.snapshot(true, cx)).unwrap(),
                )
                .unwrap();
                panel.restore(saved, window, cx);
                assert!(panel.open_panes == vec![Pane::NewTab]);
                assert_eq!(panel.blank_tabs.len(), 1);
                assert!(panel.pane == Pane::NewTab);
                panel.choose_new_tool(Pane::Files, window, cx);
                assert!(panel.open_panes == vec![Pane::Files]);
                panel.open_new_tab(window, cx);
                panel.open_tool(Pane::Files, window, cx);
                assert_eq!(panel.open_panes.len(), 2);
                assert_eq!(panel.blank_tabs.len(), 1);
                panel.close_tool(Pane::Files, window, cx);
                panel.address.update(cx, |input, cx| {
                    input.set_value("http://localhost:8766", window, cx)
                });
                panel.navigate_from_new_tab(cx);
                assert!(panel.open_panes == vec![Pane::Browser]);
                assert_eq!(
                    panel.browser.as_ref().unwrap().url(),
                    "http://localhost:8766/"
                );
                panel.close_tool(Pane::Browser, window, cx);
                assert!(panel.browser.is_none());
                assert!(panel.browsers.is_empty());
                assert!(panel.pane == Pane::NewTab);
            })
        });
    }

    #[::gpui::test]
    fn sidebar_close_buttons_reveal_launcher_at_sidebar_width(cx: &mut gpui::TestAppContext) {
        struct Fixture(Entity<RightPanel>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(zpx(PANEL_WIDTH)).h(zpx(600.)).child(self.0.clone())
            }
        }
        cx.update(gpui_kit::init);
        // This test measures settled hit targets; motion has separate coverage.
        cx.update(|cx| cx.set_reduce_motion(true));
        let dir = tempfile::tempdir().unwrap();
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| legacy_panel(dir.path().into(), window, cx));
            panel = Some(view.clone());
            let fixture = cx.new(|_| Fixture(view));
            gpui_kit::component::Root::new(fixture, window, cx)
        });
        let panel = panel.unwrap();
        for id in [
            "close-tab-files",
            "close-tab-changes",
            "close-tab-terminal-0",
        ] {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let target = cx
                .debug_bounds(id)
                .expect("close button is laid out")
                .center();
            cx.simulate_mouse_move(target, None, Modifiers::default());
            cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
        }
        panel.read_with(cx, |panel, _| {
            assert!(panel.open_panes == vec![Pane::NewTab]);
            assert_eq!(panel.blank_tabs.len(), 1);
            assert!(panel.pane == Pane::NewTab);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("Files").expect("launcher shows Files");
        assert!(target.size.width > zpx(200.));
        assert_eq!(target.size.height, zpx(44.));
        cx.simulate_mouse_move(target.center(), None, Modifiers::default());
        cx.simulate_mouse_down(target.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.open_panes == vec![Pane::Files]);
            assert!(panel.pane == Pane::Files);
        });
    }

    #[::gpui::test]
    fn closing_sidebar_files_keeps_unsaved_edits(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        let mut panel = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| legacy_panel(dir.path().into(), window, cx));
            panel = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let panel = panel.unwrap();
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.file_drafts.insert(
                    path.clone(),
                    crate::workspace_files::FileDraft {
                        original: "original".into(),
                        text: "unsaved".into(),
                    },
                );
                panel.close_tool(Pane::Files, window, cx);
                assert!(panel.open_panes.contains(&Pane::Files));
                assert!(panel.pane == Pane::Files);
                assert_eq!(panel.file_drafts[&path].text, "unsaved");
            })
        });
    }

    #[::gpui::test]
    fn closing_sidebar_terminal_releases_shells_and_reopening_starts_fresh(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.open_tool(Pane::Terminal, window, cx);
            let first_id = panel.terminal_tabs[0].clone();
            let first = panel.terminals[&first_id].clone();
            panel.new_terminal_tab(window, cx);
            let second = panel.terminals[&panel.terminal_tabs[1]].clone();
            panel.close_tool(Pane::Terminal, window, cx);
            assert!(panel.terminals.is_empty());
            assert_eq!(
                first.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Closed
            );
            assert_eq!(
                second.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Closed
            );
            panel.open_tool(Pane::Terminal, window, cx);
            assert_ne!(panel.terminal_tabs[0], first_id);
            assert_eq!(panel.terminals.len(), 1);
            panel.release(cx);
            panel
        });
    }

    #[test]
    fn saved_changes_requires_an_explicit_click() {
        let mut saved = SavedPanel {
            open: true,
            pane: Pane::Changes,
            ..Default::default()
        };
        assert!(!saved.restore_open());
        saved.pane = Pane::Files;
        assert!(saved.restore_open());
    }

    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn large_documents_bypass_the_text_limit_and_missing_documents_still_report_errors() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["large.pdf", "formatted.docx", "legacy.doc"] {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0; MAX_FILE_BYTES as usize + 1]).unwrap();
            assert!(
                matches!(read_viewer(path.clone()), Viewer::Document { path: selected } if selected == path)
            );
        }
        assert!(matches!(
            read_viewer(dir.path().join("missing.pdf")),
            Viewer::Note { .. }
        ));
        let text = dir.path().join("large.txt");
        std::fs::write(&text, vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(matches!(read_viewer(text), Viewer::Note { .. }));
    }

    #[test]
    fn special_files_are_not_read_and_reads_are_capped() {
        // Character devices report size 0 but stream forever.
        assert!(matches!(
            read_viewer(PathBuf::from("/dev/zero")),
            Viewer::Note { message, .. } if message == "Not a regular file"
        ));
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(read_viewer(fifo), Viewer::Note { .. }));
        let path = dir.path().join("grown.txt");
        std::fs::write(&path, b"12345").unwrap();
        assert!(read_capped(&path, 4).unwrap().is_none());
        assert_eq!(read_capped(&path, 5).unwrap().unwrap(), b"12345");
    }

    #[::gpui::test]
    fn folder_loading_is_deferred_and_obsolete_requests_cannot_replace_new_state(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            assert!(
                panel.rows.is_empty(),
                "construction must never read a directory on the UI thread"
            );
            panel.show_hidden = true;
            panel.refresh_tree(cx);
            panel
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.rows.len(), 2);
            assert!(
                panel
                    .rows
                    .iter()
                    .any(|(path, _, _)| path.ends_with(".hidden"))
            );
        });
    }

    #[::core::prelude::v1::test]
    fn unloaded_expanded_folder_does_not_hide_available_siblings() {
        let root = PathBuf::from("/work");
        let slow = root.join("cloud");
        let file = root.join("file.txt");
        let cache = HashMap::from([(
            root.clone(),
            vec![(slow.clone(), true), (file.clone(), false)],
        )]);
        assert_eq!(
            cached_tree_rows(&root, &HashSet::from([slow.clone()]), &cache),
            vec![(slow, 0, true), (file, 0, false)]
        );
    }

    use ::core::prelude::v1::test;

    #[::gpui::test]
    fn markdown_preview_scrolls_and_retains_the_selected_source(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.MD");
        let source = "# Heading\n\nA **formatted** paragraph.\n\n".repeat(100);
        std::fs::write(&path, &source).unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.viewer = read_viewer(path.clone());
            panel.markdown_preview = true;
            panel
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(panel.markdown_scroll.max_offset().y > px(1000.0));
            assert!(panel.snapshot(true, cx).markdown_preview);
            match &panel.viewer {
                Viewer::Text {
                    path: selected,
                    markdown: Some(markdown),
                    ..
                } => {
                    assert_eq!(selected, &path);
                    assert_eq!(markdown.as_ref(), source);
                }
                _ => panic!("Markdown must retain its original source"),
            }
        });
        panel.update(cx, |panel, cx| {
            panel.markdown_preview = false;
            cx.notify();
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(panel.editors.contains_key(&path));
            assert_eq!(panel.snapshot(true, cx).selected, Some(path.clone()));
        });
        assert_eq!(std::fs::read_to_string(path).unwrap(), source);
    }

    #[::gpui::test]
    fn changes_refresh_preserves_disclosures_and_loads_external_edits(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new(crate::projects::git_program().expect("git installed"))
                .args(["init", "-q"])
                .arg(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let path = dir.path().join("example.txt");
        std::fs::write(&path, "before\n").unwrap();
        std::fs::write(dir.path().join("other.txt"), "another file\n").unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.pane = Pane::Changes;
            panel.prepare_to_show(window, cx);
            panel
        });
        cx.run_until_parked();
        panel.update(cx, |panel, _| {
            assert!(panel.change_expanded.is_empty());
            assert_eq!(panel.change_diffs.len(), 2);
            panel.change_expanded.insert(PathBuf::from("example.txt"));
        });
        panel.update(cx, |panel, cx| panel.refresh_changes(cx));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.change_expanded.contains(Path::new("example.txt")));
        });
        std::fs::write(&path, "after\n").unwrap();
        panel.update(cx, |panel, cx| panel.refresh_changes(cx));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.change_diffs[Path::new("example.txt")].contains("+after"));
        });
        std::fs::remove_file(&path).unwrap();
        panel.update(cx, |panel, cx| panel.refresh_changes(cx));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(!panel.change_expanded.contains(Path::new("example.txt")));
            assert_eq!(panel.change_diffs.len(), 1);
        });
    }

    #[::gpui::test]
    fn preparing_visible_changes_does_not_restart_completed_loads(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.pane = Pane::Changes;
            panel.prepare_to_show(window, cx);
            panel
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                assert!(!panel.changes_loading);
                let request = panel.change_request;
                panel.prepare_to_show(window, cx);
                panel.prepare_to_show(window, cx);
                assert_eq!(panel.change_request, request);
                assert!(!panel.changes_loading);
            })
        });
    }

    #[::gpui::test]
    fn launcher_creates_a_fresh_terminal_in_the_main_strip(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.open_tool(Pane::Terminal, window, cx);
            panel.open_new_tab(window, cx);
            panel
        });
        let first = panel.read_with(cx, |panel, _| {
            panel.terminals[&panel.terminal_tabs[0]].clone()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("Terminal").unwrap().center();
        cx.simulate_mouse_move(target, None, Modifiers::default());
        cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(panel.pane == Pane::Terminal);
            assert_eq!(panel.terminal_active, 1);
            assert_eq!(panel.terminal_tabs.len(), 2);
            let second = &panel.terminals[&panel.terminal_tabs[1]];
            assert_ne!(
                first.read(cx).snapshot().pid,
                second.read(cx).snapshot().pid
            );
            assert_eq!(panel.sidebar_tabs().len(), 4);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        for id in ["close-tab-terminal-0", "close-tab-terminal-1"] {
            let close = cx.debug_bounds(id).unwrap();
            assert!(close.center().y < zpx(crate::icons::TOOL_ROW_HEIGHT));
        }
        assert!(cx.debug_bounds("terminal-tabs").is_none());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.terminal_active = 0;
                panel.open_tool(Pane::Terminal, window, cx);
                assert_eq!(panel.terminals[&panel.terminal_tabs[0]], first);
                panel.open_tool(Pane::Files, window, cx);
                panel.close_terminal_tab(0, window, cx);
                assert!(panel.pane == Pane::Files);
                assert_eq!(panel.terminals.len(), 1);
                panel.close_terminal_tab(0, window, cx);
                assert!(panel.terminals.is_empty());
                assert!(!panel.open_panes.contains(&Pane::Terminal));
                panel.open_new_tab(window, cx);
                panel.choose_new_tool(Pane::Terminal, window, cx);
                assert_eq!(panel.terminal_tabs.len(), 1);
                assert_eq!(panel.terminals.len(), 1);
                panel.release(cx);
            })
        });
    }

    #[::gpui::test]
    fn terminal_tabs_keep_independent_shells_and_close_only_the_target(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.ensure_terminal(window, cx);
            let first_id = panel.terminal_tabs[0].clone();
            let first = panel.terminals[&first_id].clone();
            panel.new_terminal_tab(window, cx);
            let second_id = panel.terminal_tabs[1].clone();
            let second = panel.terminals[&second_id].clone();
            let first_view = first.read(cx).terminal_view();
            let second_view = second.read(cx).terminal_view();
            assert_ne!(
                first_view.read(cx).session().pid(),
                second_view.read(cx).session().pid()
            );
            let saved = panel.snapshot(true, cx);
            let restored: SavedPanel =
                serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            assert_eq!(
                restored.terminal_tabs,
                vec![first_id.clone(), second_id.clone()]
            );
            assert_eq!(restored.terminal_active, 1);
            panel.close_terminal_tab(0, window, cx);
            assert_eq!(
                first.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Closed
            );
            assert_eq!(
                second.read(cx).snapshot().status,
                crate::terminal_host::TerminalHostStatus::Running
            );
            assert_eq!(panel.terminal_tabs, vec![second_id]);
            second.update(cx, |terminal, cx| terminal.close(cx));
            panel
        });
    }

    #[::gpui::test]
    fn editing_persists_and_saves_drafts(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "original\n").unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.viewer = read_viewer(path.clone());
            panel
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let editor = panel.read(cx).editors[&path].clone();
            editor.update(cx, |editor, cx| {
                editor.replace_all("changed\nsecond\n", window, cx)
            });
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            assert!(panel.selected_dirty());
            assert_eq!(
                panel.snapshot(true, cx).file_drafts[&path].text,
                "changed\nsecond\n"
            );
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                let saved: SavedPanel = serde_json::from_str(
                    &serde_json::to_string(&panel.snapshot(true, cx)).unwrap(),
                )
                .unwrap();
                panel.restore(saved, window, cx);
            })
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.editors[&path].read(cx).text().to_string(),
                "changed\nsecond\n"
            );
            assert!(panel.selected_dirty());
        });
        panel.update(cx, |panel, cx| panel.save_selected(cx));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed\nsecond\n");
        panel.read_with(cx, |panel, _| assert!(!panel.selected_dirty()));
    }

    #[::gpui::test]
    fn file_split_drag_keeps_the_original_pointer_anchor(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        cx.run_until_parked();
        let layout = cx.debug_bounds("files-layout").unwrap();
        let handle = cx.debug_bounds("file-preview-resize").unwrap();
        let initial_height = (handle.top() - layout.top()).as_f32();
        assert!((initial_height / layout.size.height.as_f32() - 0.8).abs() < 0.01);
        let handle_y = handle.center().y.as_f32();
        cx.simulate_mouse_down(
            point(px(100.0), px(handle_y)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(100.0), px(handle_y - 20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(px(100.0), px(handle_y - 40.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(100.0), px(handle_y - 40.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(
                (panel.file_tree_height - (initial_height - 40.0)).abs() < 1.0,
                "height={} initial={initial_height}",
                panel.file_tree_height
            )
        });
    }

    #[::gpui::test]
    fn long_file_has_bounded_scrollable_preview_on_both_axes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let source = ("long line ".repeat(40) + "\n").repeat(1000);
        std::fs::write(&path, &source).unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.viewer = read_viewer(path.clone());
            panel
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            panel.editors[&path].update(cx, |editor, cx| {
                editor.set_scroll_offset(point(px(-120.), px(-400.)), cx)
            });
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            let editor = panel.editors[&path].read(cx);
            assert_eq!(editor.text().to_string(), source);
            assert!(editor.scroll_offset().y < px(0.));
        });
    }

    #[::gpui::test]
    fn browser_pages_are_independent_outer_tabs(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_panes = vec![Pane::Files];
                let original_pane = panel.pane;
                for url in ["https://example.com/one", "https://example.com/two"] {
                    panel
                        .browser_command(
                            serde_json::from_value(serde_json::json!({
                                "action": "new_tab", "url": url
                            }))
                            .unwrap(),
                            window,
                            cx,
                        )
                        .unwrap();
                }
                assert!(
                    panel.pane == original_pane,
                    "agent pages must not steal focus"
                );
                assert_eq!(panel.browser_tabs.len(), 2, "no extra initial blank page");
                let first = panel.browser_tabs[0].id.clone();
                let second = panel.browser_tabs[1].id.clone();
                assert!(
                    panel
                        .sidebar_keys()
                        .contains(&SidebarTab::Browser(first.clone()))
                );
                assert!(
                    panel
                        .sidebar_keys()
                        .contains(&SidebarTab::Browser(second.clone()))
                );
                panel.reveal_browser(&first, window, cx);
                assert_eq!(panel.active_browser_id(), first);
                let moved = panel
                    .take_tab(&SidebarTab::Browser(first.clone()), window, cx)
                    .unwrap();
                assert_eq!(panel.agent_browser_tabs(cx).len(), 1);
                assert_eq!(panel.active_browser_id(), second);
                assert!(!panel.browsers.contains_key(&first));
                assert_eq!(moved.read(cx).agent_browser_tabs(cx).len(), 1);
                assert_eq!(moved.read(cx).active_browser_id(), first);
                assert!(
                    moved.read(cx).browsers.contains_key(&first),
                    "move keeps the existing web view"
                );
                assert_eq!(moved.read(cx).browser_profile, panel.browser_profile);
                panel.accept_tab(moved, window, cx);
                assert_eq!(panel.agent_browser_tabs(cx).len(), 2);
                assert!(panel.close_browser_id(&first, window, cx));
                assert_eq!(panel.agent_browser_tabs(cx).len(), 1);
                assert!(panel.close_browser_id(&second, window, cx));
                assert!(panel.agent_browser_tabs(cx).is_empty());
                assert!(!panel.open_panes.contains(&Pane::Browser));
            });
        });
    }

    #[::gpui::test]
    fn browser_launcher_replaces_only_its_blank_tab(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_panes = vec![Pane::Files];
                for url in ["https://example.com/one", "https://example.com/two"] {
                    panel.open_new_tab(window, cx);
                    panel
                        .address
                        .update(cx, |input, cx| input.set_value(url, window, cx));
                    panel.navigate_from_new_tab(cx);
                }
                assert_eq!(panel.browser_tabs.len(), 2);
                assert!(panel.blank_tabs.is_empty());
                assert_eq!(panel.browser_active, 1);
                panel.close_active_tab(window, cx);
                assert_eq!(panel.browser_tabs.len(), 1);
                assert_eq!(panel.browser_tabs[0].url, "https://example.com/one");
                panel.close_active_tab(window, cx);
                assert!(!panel.open_panes.contains(&Pane::Browser));
            });
        });
    }

    #[::gpui::test]
    fn legacy_browser_group_restores_as_outer_tabs(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let pages = vec![
            crate::browser::SavedTab::default(),
            crate::browser::SavedTab {
                url: "https://example.com".into(),
                ..Default::default()
            },
        ];
        let first = pages[0].id.clone();
        let second = pages[1].id.clone();
        let saved = SavedPanel {
            pane: Pane::Browser,
            open_panes: vec![Pane::Files, Pane::Browser, Pane::Changes],
            tab_order: vec![
                SidebarTab::Tool(Pane::Files),
                SidebarTab::Tool(Pane::Browser),
                SidebarTab::Tool(Pane::Changes),
            ],
            browser_tabs: pages,
            blank_tabs: Vec::new(),
            browser_active: 1,
            ..Default::default()
        };
        let split = saved.clone().split_browser_pages();
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].browser_tabs[0].id, second);
        assert_eq!(split[1].browser_tabs[0].id, first);
        assert_eq!(split[0].browser_profile, split[1].browser_profile);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(dir.path().into(), window, cx);
            panel.restore(saved, window, cx);
            panel
        });
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.sidebar_keys(),
                vec![
                    SidebarTab::Tool(Pane::Files),
                    SidebarTab::Browser(first),
                    SidebarTab::Browser(second.clone()),
                    SidebarTab::Tool(Pane::Changes)
                ]
            );
            assert_eq!(panel.active_browser_id(), second);
            let saved = panel.snapshot(false, cx);
            assert_eq!(
                saved.browser_tabs.len(),
                2,
                "including an intentionally blank browser page"
            );
        });
    }

    #[::core::prelude::v1::test]
    fn browser_profiles_and_tabs_are_independent_and_persisted() {
        let a = SavedPanel::default();
        let b = SavedPanel::default();
        assert_ne!(a.browser_profile, b.browser_profile);
        assert_ne!(a.browser_tabs[0].id, b.browser_tabs[0].id);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("panels.json");
        let mut store = PanelStore::default();
        store.set("chat:a".into(), a.clone());
        store.set("chat:b".into(), b.clone());
        store.save(&path).unwrap();
        let mut restored = PanelStore::load(&path);
        assert_eq!(restored.get("chat:a").browser_profile, a.browser_profile);
        assert_eq!(
            restored.get("chat:a").browser_tabs[0].id,
            a.browser_tabs[0].id
        );
        restored.remove("chat:a");
        assert_eq!(restored.get("chat:b").browser_profile, b.browser_profile);
    }

    #[::core::prelude::v1::test]
    fn panel_state_is_independent_per_chat_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panels.json");
        let mut store = PanelStore::default();
        let chat_a = SavedPanel {
            open: true,
            pane: Pane::Terminal,
            tree_height: 180.0,
            show_hidden: true,
            ..Default::default()
        };
        let chat_b = SavedPanel {
            selected: Some("/work/README.md".into()),
            expanded: HashSet::from([PathBuf::from("/work/src")]),
            ..Default::default()
        };
        store.set("chat:a".into(), chat_a.clone());
        store.set("chat:b".into(), chat_b.clone());
        store.save(&path).unwrap();
        let restored = PanelStore::load(&path);
        assert!(restored.get("chat:a") == chat_a);
        assert!(restored.get("chat:b") == chat_b);
        assert!(restored.get("chat:b").open);
        assert!(restored.get("new-chat").open);
    }

    #[::core::prelude::v1::test]
    fn dotfiles_are_opt_in_and_generated_trees_stay_hidden() {
        assert!(visible_name("src", false));
        assert!(!visible_name(".env", false));
        assert!(visible_name(".env", true));
        assert!(visible_name(".git", true));
        assert!(!visible_name("node_modules", true));
        assert!(!visible_name("target", true));
    }

    #[::core::prelude::v1::test]
    fn file_split_stays_usable_at_both_extremes() {
        assert_eq!(resized_tree_height(220.0, -500.0, 600.0), 72.0);
        assert_eq!(resized_tree_height(220.0, 500.0, 600.0), 528.0);
        assert_eq!(resized_tree_height(220.0, 30.0, 600.0), 250.0);
    }
}
