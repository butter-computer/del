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
    start_height: f32,
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
    vec![Pane::Files, Pane::Changes, Pane::Terminal]
}

/// Visual order is independent of the backing tool instances and selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SidebarTab {
    Tool(Pane),
    Terminal(String),
    Blank(String),
}

#[derive(Clone)]
struct DraggedSidebarTab {
    owner: EntityId,
    key: SidebarTab,
    pane: Pane,
    scroll: ScrollHandle,
    origin: Rc<Cell<Point<Pixels>>>,
    width: Pixels,
    offset: Point<Pixels>,
}

impl Render for DraggedSidebarTab {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.scroll.bounds();
        let width = self.width.min(bounds.size.width);
        let natural = window.mouse_position().x - self.offset.x;
        let left = natural.clamp(bounds.left(), (bounds.right() - width).max(bounds.left()));
        h_flex()
            .relative()
            .left(left - natural)
            .top(self.origin.get().y - window.mouse_position().y)
            .w(width)
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .px_1p5()
            .gap_1p5()
            .rounded_md()
            .bg(crate::appearance::popup_surface())
            .text_color(rgb(0xf5f5f5))
            .text_size(zpx(14.))
            .shadow_md()
            .child(crate::icons::icon(self.pane.icon()).with_size(zpx(15.)))
            .child(self.pane.tab_label().1)
    }
}

struct SidebarDrag {
    key: SidebarTab,
    order: Vec<SidebarTab>,
    centers: Vec<Pixels>,
    pointer_offset: Pixels,
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
        self.open && self.pane != Pane::Changes
    }

    pub fn browser_profile(&self) -> &str {
        &self.browser_profile
    }
}
impl Default for SavedPanel {
    fn default() -> Self {
        Self {
            open: false,
            side_chat_id: None,
            pane: Pane::Files,
            open_panes: default_panes(),
            tab_order: Vec::new(),
            blank_tabs: Vec::new(),
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
            .map(|panel| panel.browser_tabs.clone())
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
    side_context: Option<crate::side_chat::Context>,
    side_chat: Option<Entity<crate::side_chat::SideChat>>,
    side_chat_id: Option<String>,
    workspace: bool,
    pane: Pane,
    open_panes: Vec<Pane>,
    tab_order: Vec<SidebarTab>,
    blank_tabs: Vec<BlankTab>,
    blank_active: usize,
    address: Entity<InputState>,
    new_tab_scroll: ScrollHandle,
    tool_tab_scroll: ScrollHandle,
    tab_drag: Option<SidebarDrag>,
    tab_drag_origin: Rc<Cell<Point<Pixels>>>,
    root: PathBuf,
    expanded: HashSet<PathBuf>,
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
    rows: Vec<(PathBuf, usize, bool)>,
    markdown_preview: bool,
    markdown_scroll: ScrollHandle,
}

impl EventEmitter<crate::side_chat::SendToMain> for RightPanel {}
impl EventEmitter<crate::chat::OpenLink> for RightPanel {}

impl RightPanel {
    pub fn set_side_context(&mut self, context: crate::side_chat::Context) {
        self.side_context = Some(context);
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
            cx.observe(&side, |_, _, cx| cx.notify()).detach();
            cx.subscribe(&side, |_, _, event: &crate::side_chat::SendToMain, cx| {
                cx.emit(event.clone())
            })
            .detach();
            cx.subscribe(&side, |_, _, event: &crate::chat::OpenLink, cx| {
                cx.emit(crate::chat::OpenLink(event.0.clone()))
            })
            .detach();
            self.side_chat = Some(side);
        }
        if let Some(side) = &self.side_chat {
            side.update(cx, |side, cx| side.focus(window, cx));
        }
    }

    fn activate_pane(&mut self, pane: Pane) {
        if (pane != Pane::NewTab || !self.blank_tabs.is_empty()) && !self.open_panes.contains(&pane)
        {
            self.open_panes.push(pane);
        }
        self.pane = pane;
        self.tab_order = self.sidebar_keys();
    }

    /// Terminal instances occupy the same strip as the other tools. The saved
    /// IDs also migrate older terminal subtabs without losing their order.
    fn sidebar_tabs(&self) -> Vec<(Pane, Option<usize>)> {
        let mut tabs: Vec<_> = self
            .open_panes
            .iter()
            .flat_map(|pane| {
                if *pane == Pane::Terminal {
                    (0..self.terminal_tabs.len())
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
            (Pane::NewTab, Some(index)) => SidebarTab::Blank(self.blank_tabs[index].id.clone()),
            _ => SidebarTab::Tool(pane),
        }
    }

    fn sidebar_keys(&self) -> Vec<SidebarTab> {
        self.sidebar_tabs()
            .into_iter()
            .map(|(pane, instance)| self.tab_key(pane, instance))
            .collect()
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
        if order != self.tab_order {
            self.tab_order = order;
            crate::haptics::tick();
        }
        cx.notify();
    }

    fn finish_tab_drag(&mut self, cx: &mut Context<Self>) {
        if self.tab_drag.take().is_some() {
            cx.notify();
        }
    }

    fn pane_instance(&self, pane: Pane) -> Option<usize> {
        match pane {
            Pane::Terminal => Some(self.terminal_active),
            Pane::NewTab if !self.blank_tabs.is_empty() => Some(self.blank_active),
            _ => None,
        }
    }

    fn select_blank_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        self.blank_active = index;
        self.activate_pane(Pane::NewTab);
        self.show_blank_address(window, cx);
        cx.notify();
    }

    fn choose_new_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
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

    fn open_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        if self.pane != pane {
            self.change_expanded.clear();
        }
        self.activate_pane(pane);
        if let Some(index) = self
            .sidebar_tabs()
            .iter()
            .position(|(p, instance)| *p == pane && *instance == self.pane_instance(pane))
        {
            self.tool_tab_scroll.scroll_to_item(index);
        }
        self.prepare_to_show(window, cx);
        match pane {
            Pane::Terminal => self.focus_terminal(window, cx),
            Pane::Changes => self.refresh_changes(cx),
            Pane::Files => self.refresh_tree(cx),
            Pane::SideChat => self.ensure_side_chat(window, cx),
            Pane::NewTab => self.show_blank_address(window, cx),
            _ => {}
        }
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

    fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_blank(cx);
        self.blank_tabs.push(BlankTab {
            id: uuid::Uuid::new_v4().to_string(),
            address: String::new(),
        });
        self.blank_active = self.blank_tabs.len() - 1;
        self.activate_pane(Pane::NewTab);
        self.tool_tab_scroll
            .scroll_to_item(self.sidebar_tabs().len() - 1);
        self.show_blank_address(window, cx);
        cx.notify();
    }

    fn consume_blank(&mut self, destination: Pane) -> Option<usize> {
        if self.pane != Pane::NewTab || self.blank_tabs.is_empty() {
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
        let position = self.consume_blank(Pane::Browser);
        self.activate_pane(Pane::Browser);
        self.place_active_tab(position);
        // Reuse an untouched initial browser page; preserve existing browsing tabs.
        if self.browser_tabs.len() == 1
            && self.browser_tabs[0].url == "about:blank"
            && self.browser.is_none()
        {
            self.browser_tabs[0].url = url;
            self.ensure_browser(cx);
        } else {
            self.new_browser_tab(Some(url), cx);
        }
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
        if self.open_panes.is_empty() {
            self.open_new_tab(window, cx);
        } else if self.pane == pane {
            let next = self.open_panes[index.min(self.open_panes.len() - 1)];
            self.open_tool(next, window, cx);
        }
        cx.notify();
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
        self.pane == Pane::Changes
    }

    /// Free everything a panel for a closed tab still holds: shells, web views,
    /// and the polling tasks. The panel's saved state is the caller's business.
    pub fn forget_side_chat(&mut self, cx: &mut Context<Self>) {
        self.side_chat_id = None;
        if let Some(side) = self.side_chat.take() {
            side.update(cx, |side, cx| side.forget(cx));
        }
    }

    pub fn side_chat_id(&self) -> Option<String> {
        self.side_chat_id.clone()
    }

    pub fn restore_side_chat(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.side_chat_id = Some(id);
        self.show_workspace(Pane::SideChat, window, cx);
    }

    pub fn save_side_chat(&self, cx: &mut Context<Self>) {
        if let Some(side) = &self.side_chat {
            side.update(cx, |side, cx| side.save(cx));
        }
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
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
    pub fn snapshot(&self, open: bool) -> SavedPanel {
        SavedPanel {
            open,
            side_chat_id: self.side_chat_id.clone(),
            pane: self.pane,
            open_panes: self.open_panes.clone(),
            tab_order: self.sidebar_keys(),
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

    pub fn restore(&mut self, saved: SavedPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_order = saved.tab_order;
        self.tab_drag = None;
        self.side_chat_id = saved.side_chat_id;
        self.blank_tabs = saved.blank_tabs;
        let mut ids = HashSet::new();
        self.blank_tabs.retain(|tab| ids.insert(tab.id.clone()));
        self.blank_active = saved
            .blank_active
            .min(self.blank_tabs.len().saturating_sub(1));
        let unused_browser = saved
            .browser_tabs
            .iter()
            .all(|tab| tab.url == "about:blank" || tab.url.is_empty());
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
                    && !(*p == Pane::Browser && unused_browser)
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
        if self.open_panes.is_empty() {
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
            workspace: false,
            side_context: None,
            side_chat: None,
            side_chat_id: None,
            pane: Pane::Files,
            open_panes: default_panes(),
            tab_order: Vec::new(),
            blank_tabs: Vec::new(),
            blank_active: 0,
            address,
            new_tab_scroll: ScrollHandle::new(),
            tool_tab_scroll: ScrollHandle::new(),
            tab_drag: None,
            tab_drag_origin: Rc::default(),
            root,
            expanded: HashSet::new(),
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
        self.browser_tabs.push(tab);
        self.select_browser_tab(self.browser_tabs.len() - 1, cx);
        id
    }
    fn close_browser_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.take() {
            browser.hide();
        }
        let closed = self.browser_tabs.remove(index);
        self.browsers.remove(&closed.id);
        if self.browser_tabs.is_empty() {
            self.browser_tabs.push(Default::default());
        }
        self.browser_active = if index < self.browser_active {
            self.browser_active - 1
        } else {
            self.browser_active
        }
        .min(self.browser_tabs.len() - 1);
        self.ensure_browser(cx);
        cx.notify();
    }
    /// Run an agent browser action on this panel's session. `reveal` switches
    /// the pane to Browser; background sessions run without changing layout.
    pub fn browser_command(
        &mut self,
        request: del_mcp::BrowserRequest,
        reveal: bool,
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
        if reveal {
            self.activate_pane(Pane::Browser);
        }
        if let Some(id) = &request.tab_id {
            let index = self
                .browser_tabs
                .iter()
                .position(|tab| &tab.id == id)
                .ok_or("Unknown browser tab")?;
            self.select_browser_tab(index, cx);
        }
        self.ensure_browser(cx);
        let immediate = match request.action.as_str() {
            "new_tab" => {
                self.new_browser_tab(request.url.clone(), cx);
                true
            }
            "close_tab" => {
                self.close_browser_tab(self.browser_active, cx);
                true
            }
            "select_tab" | "list_tabs" => true,
            _ => false,
        };
        cx.notify();
        if immediate {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(Ok(serde_json::json!({"tabs":self.browser_snapshot(),"active_tab":self.browser_tabs[self.browser_active].id})));
            return Ok(rx);
        }
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
        let tabs = self.browser_snapshot();
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .h(zpx(28.))
                    .flex_none()
                    .min_w_0()
                    .px_2()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("browser-tabs")
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .children(tabs.iter().enumerate().map(|(index, tab)| {
                                let label = if tab.url == "about:blank" {
                                    "New tab"
                                } else {
                                    tab.url
                                        .split("://")
                                        .last()
                                        .unwrap_or(&tab.url)
                                        .split('/')
                                        .next()
                                        .unwrap_or(&tab.url)
                                };
                                h_flex()
                                    .id(("browser-tab", index))
                                    .h(crate::icons::button_size())
                                    .max_w(zpx(150.))
                                    .px_2()
                                    .gap_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(zpx(TEXT_SM))
                                    .when(index == self.browser_active, |row| {
                                        row.bg(rgba(0xffffff14))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_browser_tab(index, cx)
                                    }))
                                    .child(div().truncate().child(label.to_owned()))
                                    .child(
                                        div()
                                            .id(("close-browser-tab", index))
                                            .size(crate::icons::button_size())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .role(gpui::Role::Button)
                                            .aria_label("Close browser tab")
                                            .child(
                                                crate::icons::icon(IconName::Close)
                                                    .with_size(zpx(14.)),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.close_browser_tab(index, cx);
                                            })),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .id("new-browser-tab")
                            .size(crate::icons::button_size())
                            .flex()
                            .items_center()
                            .justify_center()
                            .role(gpui::Role::Button)
                            .aria_label("New browser tab")
                            .cursor_pointer()
                            .child(crate::icons::icon(IconName::Plus))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_browser_tab(None, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // Leave the shell's 5px splitter above the GPUI surface.
                    // Native WebKit subviews otherwise intercept its mouse-down.
                    .pl(zpx(6.))
                    .child(self.browser.as_ref().unwrap().element()),
            )
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

    fn tab(&self, pane: Pane, instance: Option<usize>, cx: &mut Context<Self>) -> Stateful<Div> {
        let (base, name) = pane.tab_label();
        let id: SharedString = match instance {
            Some(index) if pane == Pane::NewTab => {
                format!("{base}-{}", self.blank_tabs[index].id).into()
            }
            Some(index) => format!("{base}-{}", self.terminal_tabs[index]).into(),
            None => base.into(),
        };
        let label: SharedString = name.into();
        let close_id = format!("close-{id}");
        let close_selector = instance
            .map(|index| format!("close-{base}-{index}"))
            .unwrap_or_else(|| close_id.clone());
        let active = self.pane == pane && instance == self.pane_instance(pane);
        let key = self.tab_key(pane, instance);
        let dragging = self.tab_drag.as_ref().is_some_and(|drag| drag.key == key);
        let payload = DraggedSidebarTab {
            owner: cx.entity_id(),
            key,
            pane,
            scroll: self.tool_tab_scroll.clone(),
            origin: self.tab_drag_origin.clone(),
            width: Pixels::ZERO,
            offset: Point::default(),
        };
        let origin = self.tab_drag_origin.clone();
        let panel = cx.entity();
        let selector = id.to_string();
        div()
            .id(id.clone())
            .debug_selector(move || selector.clone())
            .group(id.clone())
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .px_1p5()
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
                panel.update(cx, |this, cx| {
                    let index = this
                        .sidebar_keys()
                        .iter()
                        .position(|key| key == &drag.key)
                        .unwrap();
                    preview.width = this
                        .tool_tab_scroll
                        .bounds_for_item(index)
                        .unwrap()
                        .size
                        .width;
                    this.begin_tab_drag(&drag.key, cx);
                    this.move_tab_drag(drag.owner, &drag.key, window.mouse_position(), cx);
                });
                cx.new(|_| preview)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(index) = instance {
                    if pane == Pane::NewTab {
                        this.select_blank_tab(index, window, cx);
                        return;
                    }
                    this.terminal_active = index;
                }
                this.open_tool(pane, window, cx);
            }))
            .child(
                div()
                    .mr_1p5()
                    .flex_none()
                    .child(crate::icons::icon(pane.icon()).with_size(zpx(15.))),
            )
            .child(label.clone())
            .child(
                div()
                    .id(SharedString::from(close_id))
                    .debug_selector(move || close_selector.clone())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Close {label} sidebar tab"))
                    .ml_1()
                    .size(zpx(16.))
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
                        if let Some(index) = instance {
                            if pane == Pane::NewTab {
                                this.close_blank_tab(index, window, cx);
                            } else {
                                this.close_terminal_tab(index, window, cx);
                            }
                        } else {
                            this.close_tool(pane, window, cx);
                        }
                    }))
                    .child(crate::icons::icon(IconName::Close).with_size(zpx(11.))),
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

        let tree = v_flex()
            .id("file-tree")
            .flex_none()
            .h(zpx(self.file_tree_height))
            .overflow_y_scroll()
            .children(rows.into_iter().map(|(path, depth, is_dir)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let open = self.expanded.contains(&path);
                let is_selected = selected.as_ref() == Some(&path);
                let click = path.clone();
                h_flex()
                    .w_full()
                    .h(zpx(26.0))
                    .flex_none()
                    .pl(zpx(4.0 + depth as f32 * 12.0))
                    .pr_1()
                    .child(
                        h_flex()
                            .id(SharedString::from(path.display().to_string()))
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
                            .when(is_selected, |r| {
                                r.bg(ActionTone::Accent.surface(false))
                                    .text_color(rgb(0xf5f5f5))
                            })
                            .when(!is_selected, |r| r.hover(|r| r.bg(rgba(0xffffff0a))))
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
                                .text_color(if is_dir {
                                    ActionTone::Accent.foreground()
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

        let split_drag = FileSplitDrag {
            start_y: self.split_origin.clone(),
            start_height: self.file_tree_height,
        };

        let start_y = split_drag.start_y.clone();
        v_flex()
            .size_full()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<FileSplitDrag>, _, cx| {
                    let drag = event.drag(cx);
                    let delta = del_core::zoom::unscaled(
                        event.event.position.y.as_f32() - drag.start_y.get(),
                    );
                    this.file_tree_height = resized_tree_height(
                        drag.start_height,
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
                    .overflow_hidden()
                    .child(tree),
            )
            .child(
                div()
                    .id("file-preview-resize")
                    .group("file-preview-resize")
                    .h(zpx(8.0))
                    .w_full()
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .cursor_row_resize()
                    .on_mouse_down(MouseButton::Left, move |event, _, _| {
                        start_y.set(event.position.y.as_f32())
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
        let edited = path.clone();
        cx.subscribe(&editor, move |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
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
        if (!self.preview_visible || self.pane != Pane::Browser)
            && let Some(browser) = &self.browser
        {
            browser.hide();
        }
        let show_hidden = self.show_hidden;
        let body = match self.pane {
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
                    true,
                    &self.new_tab_scroll,
                    Self::choose_new_tool,
                    cx,
                ))
                .into_any_element(),
        };
        if self.workspace {
            return v_flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(body)
                .into_any_element();
        }
        v_flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            // Nothing wraps while the panel's width animates.
            .whitespace_nowrap()
            .flex_none()
            .child(
                h_flex()
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
                                    .children(
                                        self.sidebar_tabs()
                                            .into_iter()
                                            .map(|(pane, instance)| self.tab(pane, instance, cx)),
                                    ),
                            )
                            .children([true, false].into_iter().map(|left| {
                                let scroll = self.tool_tab_scroll.clone();
                                canvas(
                                    |bounds, _, _| bounds,
                                    move |_, bounds, window, _| {
                                        let distance = if left {
                                            -scroll.offset().x
                                        } else {
                                            scroll.max_offset().x + scroll.offset().x
                                        };
                                        let alpha = (distance / zpx(24.)).clamp(0., 1.);
                                        if alpha > 0. {
                                            let surface = crate::appearance::page_surface();
                                            window.paint_quad(fill(
                                                bounds,
                                                linear_gradient(
                                                    if left { 90. } else { 270. },
                                                    linear_color_stop(surface.opacity(alpha), 0.),
                                                    linear_color_stop(surface.opacity(0.), 1.),
                                                ),
                                            ));
                                        }
                                    },
                                )
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .w(zpx(24.))
                                .when(left, |fade| fade.left_0())
                                .when(!left, |fade| fade.right_0())
                            })),
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
                    .when(self.pane == Pane::Files, |row| {
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
                    }),
            )
            .child(v_flex().flex_1().min_h_0().overflow_hidden().child(body))
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
                let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
                    serde_json::from_slice(&serde_json::to_vec(&panel.snapshot(true)).unwrap())
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
    fn restoring_an_empty_sidebar_creates_one_visible_blank_tab(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
            let saved =
                serde_json::from_str(r#"{"open":true,"pane":"NewTab","open_panes":[]}"#).unwrap();
            panel.restore(saved, window, cx);
            assert!(panel.pane == Pane::NewTab);
            assert!(panel.open_panes == vec![Pane::NewTab]);
            assert_eq!(panel.blank_tabs.len(), 1);
            panel.restore(panel.snapshot(true), window, cx);
            assert_eq!(panel.blank_tabs.len(), 1);
            panel
        });
    }

    #[::gpui::test]
    fn blank_tabs_keep_identity_and_drafts_until_explicitly_closed(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
                serde_json::from_slice(&serde_json::to_vec(&panel.snapshot(true)).unwrap())
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
            let view = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
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
    }

    #[::gpui::test]
    fn plus_creates_multiple_blank_tabs_and_hover_close_removes_only_one(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
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
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
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
                let saved =
                    serde_json::from_str(&serde_json::to_string(&panel.snapshot(true)).unwrap())
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
        let dir = tempfile::tempdir().unwrap();
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
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
            let view = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
            panel.viewer = read_viewer(path.clone());
            panel.markdown_preview = true;
            panel
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.markdown_scroll.max_offset().y > px(1000.0));
            assert!(panel.snapshot(true).markdown_preview);
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
        panel.read_with(cx, |panel, _| {
            assert!(panel.editors.contains_key(&path));
            assert_eq!(panel.snapshot(true).selected, Some(path.clone()));
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
            let saved = panel.snapshot(true);
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
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
        panel.update(cx, |panel, _| {
            assert!(panel.selected_dirty());
            assert_eq!(
                panel.snapshot(true).file_drafts[&path].text,
                "changed\nsecond\n"
            );
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                let saved: SavedPanel =
                    serde_json::from_str(&serde_json::to_string(&panel.snapshot(true)).unwrap())
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
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        cx.run_until_parked();
        // Header + tree 220px + middle of the 8px handle.
        let handle_y = crate::icons::TOOL_ROW_HEIGHT + 220.0 + 4.0;
        cx.simulate_mouse_down(
            point(px(100.0), px(handle_y)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(100.0), px(handle_y + 20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(px(100.0), px(handle_y + 40.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(100.0), px(handle_y + 40.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| assert_eq!(panel.file_tree_height, 260.0));
    }

    #[::gpui::test]
    fn long_file_has_bounded_scrollable_preview_on_both_axes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let source = ("long line ".repeat(40) + "\n").repeat(1000);
        std::fs::write(&path, &source).unwrap();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
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
        assert!(!restored.get("chat:b").open);
        assert!(!restored.get("new-chat").open);
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
