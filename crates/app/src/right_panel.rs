//! The right sidebar: files, a workspace PTY, and an embedded browser.

use crate::animated_icons::Glyph;
use crate::appearance::{ActionTone, rgba};
use crate::keyboard_control::KeyboardControl as _;
use crate::micro_motion::MotionIconExt as _;
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::button::{Button, ButtonVariants as _};
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

#[path = "commit_review.rs"]
mod commit_review;
use commit_review::{CommitReview, render_review};

/// Repeats a tab close once its unsaved edits are saved or discarded.
pub(crate) type CloseTab = Rc<dyn Fn(&mut Window, &mut App)>;

pub const PANEL_MIN_WIDTH: f32 = 320.0;
pub const PANEL_WIDTH: f32 = 360.0;
const MONO: &str = del_core::typography::MONO_FONT;
/// Largest text file loaded into the editor.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Changed files whose diffs a refresh loads; the rest are only counted.
const MAX_REVIEW_FILES: usize = 500;
/// Diff rows rendered for one expanded file.
const MAX_REVIEW_LINES: usize = 2000;

/// Text-free loading rows; the highlight sweeps only across the placeholder shapes.
fn history_skeleton_bar(key: u32, width: f32, height: f32, animate: bool) -> AnyElement {
    div()
        .relative()
        .w(relative(width))
        .h(zpx(height))
        .rounded(zpx(4.))
        .overflow_hidden()
        .bg(rgba(0xffffff10))
        .when(animate, |bar| {
            bar.child(
                h_flex()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.6))
                    .child(div().h_full().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(rgba(0xffffff00), 0.),
                        linear_color_stop(rgba(0xffffff14), 1.),
                    )))
                    .child(div().h_full().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(rgba(0xffffff14), 0.),
                        linear_color_stop(rgba(0xffffff00), 1.),
                    )))
                    .with_animation(
                        ("side-history-shimmer", key),
                        Animation::new(crate::ui::SHIMMER_PERIOD).repeat(),
                        |highlight, t| highlight.left(relative(-0.6 + 1.6 * t)),
                    ),
            )
        })
        .into_any_element()
}

/// A file's diff parsed once per load, not on every render.
struct ChangeReview {
    files: Vec<gpui_kit::component::diff::DiffFile>,
    viewer: std::cell::RefCell<Option<Entity<gpui_kit::component::diff::DiffState>>>,
    scroll: ScrollHandle,
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
            files: gpui_kit::component::diff::DiffFile::parse(text).unwrap_or_default(),
            viewer: Default::default(),
            scroll: ScrollHandle::new(),
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
    SideHistory,
}

impl Pane {
    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::Files => IconName::Folder.into(),
            Self::Changes => Icon::default().path("icons/file-diff.svg"),
            Self::Terminal => Icon::default().path("icons/terminal.svg"),
            Self::Browser => IconName::Globe.into(),
            Self::SideHistory => Icon::default().path("icons/history.svg"),
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
            Self::SideHistory => ("tab-side-history", "Side history"),
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
                    .px(zpx(crate::ui::SPACE_SM))
                    .gap(zpx(crate::ui::SPACE_SM))
                    .items_center()
                    // The same held look as a dragged top-level session tab.
                    .rounded(zpx(8.))
                    .shadow_md()
                    .bg(crate::appearance::dragged_tab_surface())
                    .text_color(crate::ui::primary_text())
                    .text_size(zpx(TEXT_MD))
                    .child(
                        crate::icons::icon(self.pane.icon())
                            .with_size(zpx(crate::icons::MARK_SIZE)),
                    )
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
        self.open
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
            open: false,
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
fn read_directory(dir: &Path, show_hidden: bool) -> Listing {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // macOS privacy protection (Desktop, Documents, Downloads) reports EPERM,
        // which Rust maps to PermissionDenied along with EACCES.
        Err(error) => {
            return Listing::Unreadable {
                denied: error.kind() == std::io::ErrorKind::PermissionDenied,
            };
        }
    };
    let mut hidden = false;
    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let visible = visible_name(&entry.file_name().to_string_lossy(), show_hidden);
            hidden |= !visible;
            visible
        })
        .map(|entry| {
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            (entry.path(), is_dir)
        })
        .collect();
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
    });
    Listing::Entries { entries, hidden }
}

/// One directory read: its visible entries, or why it could not be read.
#[derive(Clone, Debug, PartialEq)]
enum Listing {
    /// `hidden` records whether anything was filtered out of `entries`.
    Entries {
        entries: Vec<(PathBuf, bool)>,
        hidden: bool,
    },
    Unreadable {
        denied: bool,
    },
}

/// A quiet row in place of a folder's children.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TreeNote {
    /// Shown only once a read has been pending for `ui::LOADING_DELAY`.
    Loading,
    Empty,
    OnlyHidden,
    Unreadable {
        denied: bool,
    },
}

impl TreeNote {
    fn text(self) -> &'static str {
        match self {
            Self::Loading => "Loading…",
            Self::Empty => "Empty folder",
            Self::OnlyHidden => "Only hidden files",
            Self::Unreadable { denied: true } => {
                "Could not read this folder — allow access in System Settings › Privacy & Security"
            }
            Self::Unreadable { denied: false } => "Could not read this folder",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum TreeRow {
    Entry {
        path: PathBuf,
        depth: usize,
        is_dir: bool,
    },
    Note {
        parent: PathBuf,
        depth: usize,
        note: TreeNote,
    },
}

/// Flatten only cached directories; an unavailable child never blocks siblings.
/// `slow` holds folders whose first read has outlasted the loading delay.
fn cached_tree_rows(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    cache: &HashMap<PathBuf, Listing>,
    slow: &HashSet<PathBuf>,
) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0, true)];
    while let Some((path, depth, is_dir)) = pending.pop() {
        if depth > 0 {
            rows.push(TreeRow::Entry {
                path: path.clone(),
                depth: depth - 1,
                is_dir,
            });
            if !is_dir || !expanded.contains(&path) {
                continue;
            }
        }
        let note = match cache.get(&path) {
            Some(Listing::Entries { entries, hidden }) if entries.is_empty() => Some(if *hidden {
                TreeNote::OnlyHidden
            } else {
                TreeNote::Empty
            }),
            Some(Listing::Entries { entries, .. }) => {
                pending.extend(
                    entries
                        .iter()
                        .rev()
                        .map(|(child, is_dir)| (child.clone(), depth + 1, *is_dir)),
                );
                None
            }
            Some(Listing::Unreadable { denied }) => Some(TreeNote::Unreadable { denied: *denied }),
            None => slow.contains(&path).then_some(TreeNote::Loading),
        };
        if let Some(note) = note {
            rows.push(TreeRow::Note {
                parent: path,
                depth,
                note,
            });
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
    side_history: Option<Result<Vec<crate::side_chat::HistoryEntry>, String>>,
    side_history_request: u64,
    side_history_owner: Option<Option<del_agent::SessionId>>,
    side_chat_id: Option<String>,
    workspace: bool,
    tabs_in_titlebar: bool,
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
    /// The file being opened outlasted `ui::LOADING_DELAY`, so it may say so.
    slow_file_load: bool,
    change_files: Vec<crate::workspace_files::ChangedFile>,
    change_commits: Vec<crate::workspace_files::Commit>,
    commit_reviews: HashMap<String, CommitReview>,
    change_root: PathBuf,
    change_diffs: HashMap<PathBuf, String>,
    change_reviews: HashMap<PathBuf, Rc<ChangeReview>>,
    /// Changed files beyond MAX_REVIEW_FILES.
    change_omitted: usize,
    change_expanded: HashSet<PathBuf>,
    change_error: Option<String>,
    change_request: u64,
    change_date_minute: i64,
    changes_loading: bool,
    /// The first Changes load outlasted `ui::LOADING_DELAY`, so it may say so.
    slow_changes_load: bool,
    /// Git's own words behind the current Changes error are showing.
    change_error_details: bool,
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
    directory_entries: HashMap<PathBuf, Listing>,
    /// Folders whose first read has been pending longer than `ui::LOADING_DELAY`.
    slow_directories: HashSet<PathBuf>,
    split_origin: Rc<Cell<f32>>,
    split_start_height: Rc<Cell<f32>>,
    rows: Vec<TreeRow>,
    markdown_preview: bool,
    markdown_scroll: ScrollHandle,
}

impl EventEmitter<crate::side_chat::SendToMain> for RightPanel {}
impl EventEmitter<crate::chat::OpenLink> for RightPanel {}
impl EventEmitter<crate::chat::OpenPeer> for RightPanel {}

impl RightPanel {
    pub(crate) fn process_roots(&self, cx: &App) -> Vec<u32> {
        let mut roots = self
            .terminals
            .values()
            .map(|host| host.read(cx).snapshot().pid)
            .collect::<Vec<_>>();
        for tab in &self.hosted {
            roots.extend(tab.panel.read(cx).process_roots(cx));
        }
        roots
    }

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
            let panel = self.hosted[index].panel.clone();
            let (title, saving) = panel.read(cx).close_guard();
            let this = cx.entity().downgrade();
            let tab = id.to_owned();
            let close: CloseTab = Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.close_hosted(&tab, window, cx));
            });
            if RightPanel::confirm_close_with_edits(&panel, title, saving, close, window, cx) {
                return;
            }
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
        if pane != Pane::SideHistory {
            self.side_history_owner = None;
        }
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
        let position = if matches!(pane, Pane::Terminal | Pane::SideChat)
            || !self.open_panes.contains(&pane)
        {
            self.consume_blank(pane)
        } else {
            None
        };
        if pane == Pane::Terminal {
            self.new_terminal_tab(window, cx);
        } else if pane == Pane::SideChat {
            // Each invocation snapshots the current main conversation through
            // the normal fork path. Existing tabs keep their own live resources.
            let context = self.side_context.clone();
            let panel = cx.new(|cx| {
                let mut panel = RightPanel::new(self.root.clone(), window, cx);
                if let Some(context) = context {
                    panel.set_side_context(context);
                }
                panel.open_panes = vec![Pane::SideChat];
                panel.blank_tabs.clear();
                panel.show_workspace(Pane::SideChat, window, cx);
                panel
            });
            self.accept_tab(panel, window, cx);
        } else {
            self.open_tool(pane, window, cx);
        }
        self.place_active_tab(position);
    }

    fn place_active_tab(&mut self, position: Option<usize>) {
        let Some(position) = position else {
            return;
        };
        let key = self.active_hosted.as_ref().map_or_else(
            || self.tab_key(self.pane, self.pane_instance(self.pane)),
            |id| SidebarTab::Hosted(id.clone()),
        );
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
            let (title, saving) = self.close_guard();
            let this = cx.entity().downgrade();
            let close: CloseTab = Rc::new(move |window, cx| {
                let _ = this.update(cx, |this, cx| this.close_tool(Pane::Files, window, cx));
            });
            let panel = cx.entity();
            if RightPanel::confirm_close_with_edits(&panel, title, saving, close, window, cx) {
                return;
            }
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
            Pane::SideHistory => {
                self.side_history_owner = None;
                self.side_history_request += 1;
                self.side_history = None;
            }
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

    /// What closing this tab must ask about: the unsaved-edits question, and
    /// whether a save is still in flight.
    pub(crate) fn close_guard(&self) -> (Option<String>, bool) {
        (self.unsaved_close_title(), !self.saving.is_empty())
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

    /// Free everything a panel for a closed tab still holds: shells, web views,
    /// and the polling tasks. The panel's saved state is the caller's business.
    pub fn forget_side_chat(&mut self, cx: &mut Context<Self>) {
        self.side_chat_id = None;
        if let Some(side) = self.side_chat.take() {
            side.update(cx, |side, cx| side.forget(cx));
        }
    }

    pub(crate) fn select_saved_side_chat(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(tab) = self.hosted.iter().find(|tab| {
            let panel = tab.panel.read(cx);
            panel.side_chat_id.as_deref() == Some(id)
                || panel
                    .side_chat
                    .as_ref()
                    .is_some_and(|side| side.read(cx).storage_id() == id)
        }) {
            let key = tab.id.clone();
            self.select_hosted(key, window, cx);
            return true;
        }
        if self.side_chat_id.as_deref() == Some(id)
            || self
                .side_chat
                .as_ref()
                .is_some_and(|side| side.read(cx).storage_id() == id)
        {
            self.open_tool(Pane::SideChat, window, cx);
            self.focus_side_chat(window, cx);
            return true;
        }
        false
    }

    pub(crate) fn reopen_side_chat(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.select_saved_side_chat(&id, window, cx) {
            return;
        }
        let context = self.side_context.clone();
        let panel = cx.new(|cx| {
            let mut panel = Self::new(self.root.clone(), window, cx);
            if let Some(context) = context {
                panel.set_side_context(context);
            }
            panel.open_panes = vec![Pane::SideChat];
            panel.blank_tabs.clear();
            panel.restore_side_chat(id, window, cx);
            panel
        });
        self.accept_tab(panel, window, cx);
    }

    pub(crate) fn contains_saved_side_chat(&self, id: &str, cx: &App) -> bool {
        self.side_chat_id.as_deref() == Some(id)
            || self
                .side_chat
                .as_ref()
                .is_some_and(|side| side.read(cx).storage_id() == id)
            || self
                .hosted
                .iter()
                .any(|tab| tab.panel.read(cx).contains_saved_side_chat(id, cx))
    }

    pub(crate) fn side_history_parent(&self, cx: &App) -> Option<del_agent::SessionId> {
        self.side_context.as_ref().and_then(|context| {
            context
                .activity_owner
                .as_ref()
                .and_then(|(state, owner)| {
                    state
                        .upgrade()
                        .and_then(|state| match state.read(cx).side_chat_owner(*owner) {
                            Some(
                                crate::state::Tab::Session(id) | crate::state::Tab::Terminal(id),
                            ) => Some(id.clone()),
                            _ => None,
                        })
                })
                .or_else(|| match &context.source {
                    del_agent::ForkSource::Session(id) => Some(id.clone()),
                    _ => None,
                })
        })
    }

    pub(crate) fn show_side_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.side_history_owner = None;
        self.open_tool(Pane::SideHistory, window, cx);
    }

    fn refresh_side_history(&mut self, cx: &mut Context<Self>) {
        self.save_side_chat(cx);
        self.side_history_request += 1;
        let request = self.side_history_request;
        let parent = self.side_history_parent(cx);
        let requested_parent = parent.clone();
        self.side_history_owner = Some(parent.clone());
        self.side_history = None;
        let result = cx.background_executor().spawn(async move {
            match (del_agent::data_dir(), parent) {
                (Some(dir), Some(parent)) => {
                    crate::side_chat::history(&dir.join("side-chats"), &parent)
                }
                _ => Ok(Vec::new()),
            }
            .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = result.await;
            let _ = this.update(cx, |this, cx| {
                if this.side_history_request == request
                    && this.side_history_parent(cx) == requested_parent
                {
                    this.side_history = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_side_history(&self, window: &Window, cx: &App) -> AnyElement {
        let parent = self.side_history_parent(cx);
        let mut list = v_flex()
            .id("side-history-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(zpx(crate::ui::SPACE_SM));
        match &self.side_history {
            None => {
                let animate = window.is_window_active() && !cx.reduce_motion();
                for row in 0..3u32 {
                    list = list.child(
                        h_flex()
                            .p(zpx(crate::ui::SPACE_MD))
                            .gap(zpx(crate::ui::SPACE_SM))
                            .items_start()
                            .child(div().w(zpx(18.)).flex_none().child(history_skeleton_bar(
                                row * 3,
                                1.,
                                18.,
                                animate,
                            )))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(zpx(crate::ui::SPACE_SM))
                                    .child(history_skeleton_bar(
                                        row * 3 + 1,
                                        0.92 - row as f32 * 0.08,
                                        14.,
                                        animate,
                                    ))
                                    .child(history_skeleton_bar(row * 3 + 2, 0.38, 10., animate)),
                            ),
                    );
                }
            }
            Some(Err(error)) => {
                list = list.child(
                    div()
                        .id("side-history-error")
                        .tooltip({
                            let details = error.clone();
                            move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(details.clone())
                                    .build(window, cx)
                            }
                        })
                        .p(zpx(crate::ui::SPACE_MD))
                        .whitespace_normal()
                        .child(crate::ui::inline_error("Could not load Side history.")),
                )
            }
            Some(Ok(entries)) if entries.is_empty() => {
                list = list.child(
                    crate::ui::empty_note("No earlier Side chats for this session")
                        .p(zpx(crate::ui::SPACE_MD))
                        .whitespace_normal(),
                )
            }
            Some(Ok(entries)) => {
                for entry in entries {
                    let id = entry.id.clone();
                    let parent = parent.clone();
                    let recency = side_history_recency(
                        crate::sidebar::relative_time(entry.last_prompt_at),
                        entry.estimated_time,
                    );

                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("side-history-{}", entry.id)))
                            .keyboard_control()
                            .role(gpui::Role::Button)
                            .aria_label(format!("Reopen {}. {}", entry.title, recency))
                            .items_start()
                            .min_w_0()
                            .p(zpx(crate::ui::SPACE_MD))
                            .gap(zpx(crate::ui::SPACE_SM))
                            .rounded(zpx(crate::ui::RADIUS_CARD))
                            .cursor_pointer()
                            .hover(|s| s.bg(crate::ui::row_hover()))
                            .child(
                                h_flex()
                                    .h(zpx(22.))
                                    .flex_none()
                                    .items_center()
                                    .child(crate::composer_ui::provider_logo(entry.provider)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(zpx(crate::ui::SPACE_XS))
                                    .child(
                                        div()
                                            .w_full()
                                            .line_height(zpx(22.))
                                            .overflow_hidden()
                                            .child(entry.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(zpx(TEXT_SM))
                                            .text_color(crate::appearance::secondary_text())
                                            .child(recency),
                                    ),
                            )
                            .on_click(move |_, window, cx| {
                                if let Some(parent) = parent.clone() {
                                    window.dispatch_action(
                                        Box::new(crate::ReopenSideChat {
                                            id: id.clone(),
                                            parent,
                                        }),
                                        cx,
                                    );
                                }
                            }),
                    );
                }
            }
        }
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .p(zpx(crate::ui::SPACE_LG))
            .gap(zpx(crate::ui::SPACE_MD))
            .child(group_title("Side history"))
            .child(list)
            .into_any_element()
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
        if self.pane == Pane::SideHistory
            && self.side_history_owner.as_ref() != Some(&self.side_history_parent(cx))
        {
            self.refresh_side_history(cx);
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
            tabs_in_titlebar: false,
            compact_tools: true,
            watched_hosted: Default::default(),
            transfer_bounds: Default::default(),
            side_context: None,
            side_chat: None,
            side_history: None,
            side_history_request: 0,
            side_history_owner: None,
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
            slow_file_load: false,
            change_files: Vec::new(),
            change_commits: Vec::new(),
            commit_reviews: HashMap::new(),
            change_root: PathBuf::new(),
            change_diffs: HashMap::new(),
            change_reviews: HashMap::new(),
            change_omitted: 0,
            change_expanded: HashSet::new(),
            change_error: None,
            change_request: 0,
            change_date_minute: 0,
            changes_loading: false,
            slow_changes_load: false,
            change_error_details: false,
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
            slow_directories: HashSet::new(),
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
            self.change_commits.clear();
            self.commit_reviews.clear();
            self.change_diffs.clear();
            self.change_reviews.clear();
            self.change_omitted = 0;
            self.change_expanded.clear();
            self.changes_loading = false;
            self.root = root;
            self.forget_tree();
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
                // The Terminal tab says so in place, with Retry.
                self.terminal_error = Some(error.to_string());
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
        self.slow_file_load = false;
        let delay = cx.background_executor().timer(crate::ui::LOADING_DELAY);
        cx.spawn(async move |this, cx| {
            delay.await;
            let _ = this.update(cx, |this, cx| {
                if this.file_request == request {
                    this.slow_file_load = true;
                    cx.notify();
                }
            });
        })
        .detach();
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
            // A first read that outlasts the loading delay shows a quiet Loading row;
            // fast reads never flash one. Re-reads keep the cached rows meanwhile.
            if !self.directory_entries.contains_key(&path) {
                let pending = path.clone();
                let delay = cx.background_executor().timer(crate::ui::LOADING_DELAY);
                cx.spawn(async move |this, cx| {
                    delay.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.tree_request == request
                            && !this.directory_entries.contains_key(&pending)
                        {
                            this.slow_directories.insert(pending);
                            this.rebuild_tree();
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
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
                    this.slow_directories.remove(&path);
                    this.directory_entries.insert(path, entries);
                    this.rebuild_tree();
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn forget_tree(&mut self) {
        self.directory_entries.clear();
        self.slow_directories.clear();
    }

    fn rebuild_tree(&mut self) {
        self.rows = cached_tree_rows(
            &self.root,
            &self.expanded,
            &self.directory_entries,
            &self.slow_directories,
        );
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
            .keyboard_control()
            .debug_selector(move || selector.clone())
            .group(id.clone())
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .px(zpx(crate::ui::SPACE_SM))
            .w(zpx(crate::icons::TOOL_TAB_WIDTH))
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .flex()
            .flex_none()
            .whitespace_nowrap()
            .items_center()
            // The same corner as top-level session tabs.
            .rounded(zpx(8.))
            .cursor_pointer()
            .text_size(zpx(TEXT_MD))
            .text_color(if active {
                crate::ui::primary_text()
            } else {
                crate::ui::secondary_text()
            })
            .when(active, |t| t.bg(crate::ui::row_selected()))
            .when(!active, |t| t.hover(|t| t.bg(crate::ui::row_hover())))
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
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // Enter on a Browser tab continues into its address field, which the
                // Tab order cannot reach by itself. It needs a frame to be on screen.
                if pane == Pane::Browser && matches!(event, ClickEvent::Keyboard(_)) {
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                        let _ = this.update(cx, |this, _| {
                            if let Some(browser) = &this.browser {
                                browser.focus_address();
                            }
                        });
                    })
                    .detach();
                }
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
                    .mr(zpx(crate::ui::SPACE_SM))
                    .flex_none()
                    .child(crate::icons::icon(pane.icon()).with_size(zpx(crate::icons::MARK_SIZE))),
            )
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .when(working, |tab| tab.child(crate::icons::working_spinner()))
            .when(unread && !working, |tab| {
                tab.child(crate::icons::unread_mark())
            })
            .child(
                div()
                    .id(SharedString::from(close_id))
                    .keyboard_control()
                    .focus_visible(|button| button.opacity(1.))
                    .debug_selector(move || close_selector.clone())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Close {label} sidebar tab"))
                    .flex_none()
                    .ml(zpx(crate::ui::SPACE_XS))
                    .size(zpx(crate::icons::TAB_CLOSE_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(zpx(crate::ui::RADIUS_XS))
                    .text_color(crate::appearance::secondary_text())
                    .opacity(0.)
                    .group_hover(id, |button| button.opacity(1.))
                    .hover(|b| b.bg(crate::ui::row_selected()))
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
                    .motion_child(true, move |feedback| {
                        feedback
                            .glyph(Glyph::Close)
                            .with_size(zpx(crate::icons::TAB_CLOSE_GLYPH))
                    }),
            )
    }

    fn render_dotfiles_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The Settings switch: a stable id keeps its spring state, so opening
        // Files never replays the slide, and Reduce Motion settles it at once.
        let panel = cx.entity().downgrade();
        div()
            .debug_selector(|| "files-dotfiles-toggle".into())
            .ml_auto()
            .flex_none()
            .text_size(zpx(TEXT_SM))
            .text_color(crate::ui::secondary_text())
            .child(
                gpui_kit::component::switch::Switch::new("show-hidden-files")
                    .small()
                    .checked(self.show_hidden)
                    .label("Dotfiles")
                    .accessibility_label("Show dotfiles")
                    .on_click(move |checked, _, cx| {
                        let checked = *checked;
                        let _ = panel.update(cx, |this, cx| {
                            this.show_hidden = checked;
                            // Re-read in place: the current rows stay until each
                            // folder's new listing arrives, so nothing blinks.
                            this.refresh_tree(cx);
                            cx.notify();
                        });
                    }),
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
        let muted = crate::ui::faint_text();

        let empty_default_split =
            selected.is_none() && self.file_tree_height == DEFAULT_TREE_HEIGHT;

        // Match the tab strip + tab padding, less the row's own inset.
        let tree_inset = zpx(crate::ui::SPACE_MD);
        let indent = move |depth: usize| tree_inset + zpx(depth as f32 * 12.0);
        let tree = v_flex()
            .id("file-tree")
            .relative()
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .child(self.file_hover.layer(None, rgba(0)))
            .children(rows.into_iter().map(|row| {
                let (path, depth, is_dir) = match row {
                    TreeRow::Entry {
                        path,
                        depth,
                        is_dir,
                    } => (path, depth, is_dir),
                    TreeRow::Note {
                        parent,
                        depth,
                        note,
                    } => {
                        // Aligned with the names of the folder's would-be children.
                        let text_inset = zpx(crate::ui::SPACE_XS
                            + crate::icons::MARK_SIZE
                            + crate::ui::SPACE_SM);
                        return h_flex()
                            .id(SharedString::from(format!(
                                "tree-note-{}",
                                parent.display()
                            )))
                            .debug_selector(move || format!("tree-note-{note:?}"))
                            .w_full()
                            .min_h(zpx(crate::ui::ROW_COMPACT))
                            .flex_none()
                            .pl(indent(depth) + text_inset)
                            .pr(zpx(crate::ui::SPACE_MD))
                            .py(zpx(crate::ui::SPACE_XS))
                            .items_center()
                            .whitespace_normal()
                            .text_size(zpx(TEXT_SM))
                            .text_color(match note {
                                TreeNote::Unreadable { .. } => crate::ui::secondary_text(),
                                _ => crate::ui::faint_text(),
                            })
                            .child(note.text())
                            .into_any_element();
                    }
                };
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
                    .h(zpx(crate::ui::ROW_COMPACT))
                    .flex_none()
                    .pl(indent(depth))
                    .pr(zpx(crate::ui::SPACE_XS))
                    .child(
                        h_flex()
                            .id(SharedString::from(path.display().to_string()))
                            .relative()
                            .flex_1()
                            .h(zpx(crate::ui::ROW_COMPACT))
                            .min_w_0()
                            .px(zpx(crate::ui::SPACE_XS))
                            .gap(zpx(crate::ui::SPACE_SM))
                            .items_center()
                            .rounded(zpx(crate::ui::RADIUS_ROW))
                            .cursor_pointer()
                            .text_size(zpx(TEXT_MD))
                            .text_color(crate::ui::primary_text())
                            .on_hover(move |entered, window, cx| {
                                hover.hover(&hover_key, *entered, window, cx);
                            })
                            // The moving hover highlight paints over the selected fill.
                            .child(self.file_hover.marker(
                                marker_key,
                                if is_selected {
                                    crate::ui::row_selected()
                                } else {
                                    rgba(0)
                                },
                                crate::ui::row_hover(),
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
                                .with_size(zpx(crate::icons::MARK_SIZE))
                                .text_color(if is_dir && open {
                                    crate::ui::primary_text()
                                } else if is_dir {
                                    crate::ui::secondary_text()
                                } else {
                                    crate::ui::faint_text()
                                }),
                            )
                            .child(div().min_w_0().truncate().child(name)),
                    )
                    .into_any_element()
            }));

        let viewer = match &self.viewer {
            Viewer::Document { .. } => self
                .document_preview
                .as_ref()
                .filter(|_| self.preview_visible)
                .map(|preview| preview.element().into_any_element())
                .unwrap_or_else(|| {
                    loading_note("Loading document…", self.slow_file_load).into_any_element()
                }),
            Viewer::None => crate::ui::empty_note("Select a file to view it")
                .p(zpx(crate::ui::SPACE_MD))
                .whitespace_nowrap()
                .into_any_element(),
            Viewer::Loading { .. } => {
                loading_note("Loading file…", self.slow_file_load).into_any_element()
            }
            Viewer::Note { message, .. } => crate::ui::empty_note(message.clone())
                .p(zpx(crate::ui::SPACE_MD))
                .whitespace_normal()
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
                        .p(zpx(crate::ui::SPACE_MD))
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
                        .px(zpx(crate::ui::SPACE_SM))
                        .gap(zpx(crate::ui::SPACE_SM))
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
                            header.child(
                                h_flex()
                                    .gap(zpx(crate::ui::SPACE_2XS))
                                    .flex_none()
                                    .children(
                                        [(false, "Markdown"), (true, "Preview")].into_iter().map(
                                            |(preview, label)| {
                                                div()
                                                    .id(label)
                                                    .keyboard_control()
                                                    .role(gpui::Role::Button)
                                                    .aria_label(label)
                                                    .h(zpx(22.0))
                                                    .px(zpx(crate::ui::SPACE_SM))
                                                    .flex()
                                                    .items_center()
                                                    .rounded(zpx(crate::ui::RADIUS_ROW))
                                                    .text_size(zpx(TEXT_SM))
                                                    .cursor_pointer()
                                                    .text_color(crate::appearance::secondary_text())
                                                    .when(
                                                        self.markdown_preview == preview,
                                                        |button| {
                                                            button
                                                                .bg(crate::ui::row_selected())
                                                                .text_color(
                                                                    crate::ui::primary_text(),
                                                                )
                                                        },
                                                    )
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.markdown_preview = preview;
                                                        cx.notify();
                                                    }))
                                                    .child(label)
                                            },
                                        ),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .id("close-file-preview")
                                .keyboard_control()
                                .role(gpui::Role::Button)
                                .aria_label("Close file preview")
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Close file preview")
                                        .build(window, cx)
                                })
                                .size(crate::icons::button_size())
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(zpx(crate::ui::RADIUS_ROW))
                                .cursor_pointer()
                                .hover(|button| button.bg(crate::ui::row_selected()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.file_request += 1;
                                    this.document_preview = None;
                                    this.viewer = Viewer::None;
                                    cx.notify();
                                }))
                                .motion_child(true, move |feedback| {
                                    feedback.glyph(Glyph::Close).text_color(muted)
                                }),
                        ),
                )
            })
            .when(matches!(self.viewer, Viewer::Text { .. }), |preview| {
                // Save and Revert only have something to do once there are edits, so
                // they stay quiet until then and colour never decorates a clean file.
                let dirty = self.selected_dirty();
                let saving = self
                    .selected_path()
                    .is_some_and(|path| self.saving.contains(&path));
                preview.child(
                    h_flex()
                        .flex_none()
                        .gap(zpx(crate::ui::SPACE_XS))
                        .px(zpx(crate::ui::SPACE_SM))
                        .py(zpx(crate::ui::SPACE_XS))
                        .child(
                            file_action(
                                "save-file",
                                if saving { "Saving…" } else { "Save" },
                                dirty && !saving,
                                dirty,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_selected(cx))),
                        )
                        .child(
                            file_action("revert-file", "Revert", dirty && !saving, false).on_click(
                                cx.listener(|this, _, window, cx| this.confirm_revert(window, cx)),
                            ),
                        )
                        .child(
                            file_action("reload-file", "Reload", true, false)
                                .on_click(cx.listener(|this, _, _, cx| this.reload_selected(cx))),
                        ),
                )
            })
            .when_some(self.file_error.clone(), |preview, error| {
                preview.child(
                    crate::ui::inline_error(error)
                        .p(zpx(crate::ui::SPACE_SM))
                        .whitespace_normal(),
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
        let layout = v_flex()
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
                    .px(zpx(crate::ui::SPACE_SM))
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
                            .bg(crate::appearance::separator())
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
            );
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .debug_selector(|| "files-toolbar".into())
                    .w_full()
                    .flex_none()
                    .px(zpx(crate::ui::SPACE_MD))
                    .py(zpx(crate::ui::SPACE_SM))
                    .items_center()
                    .child(self.render_dotfiles_toggle(cx)),
            )
            .child(div().flex_1().min_h_0().child(layout))
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
        if let Some(path) = self.selected_path() {
            self.save_draft(path, cx).detach();
        }
    }
    /// Resolves to whether the draft is now saved.
    fn save_draft(&mut self, path: PathBuf, cx: &mut Context<Self>) -> Task<bool> {
        let Some(draft) = self.file_drafts.get(&path).cloned() else {
            return Task::ready(true);
        };
        if draft.text == draft.original {
            return Task::ready(true);
        }
        if !self.saving.insert(path.clone()) {
            // Already saving: wait for that save rather than starting another.
            return cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                    let Ok(done) = this.read_with(cx, |this, _| {
                        (!this.saving.contains(&path)).then(|| {
                            this.file_drafts
                                .get(&path)
                                .is_none_or(|draft| draft.text == draft.original)
                        })
                    }) else {
                        return false;
                    };
                    if let Some(saved) = done {
                        return saved;
                    }
                }
            });
        }
        self.file_error = None;
        let target = path.clone();
        let text = draft.text.clone();
        let save = cx.background_executor().spawn(async move {
            crate::workspace_files::save_file(&target, &draft.original, &draft.text)
        });
        let task = cx.spawn(async move |this, cx| {
            let result = save.await;
            this.update(cx, |this, cx| {
                this.saving.remove(&path);
                let saved = match result {
                    Ok(()) => {
                        if let Some(draft) = this.file_drafts.get_mut(&path) {
                            draft.original = text;
                        }
                        this.file_error = None;
                        this.refresh_changes(cx);
                        true
                    }
                    Err(reason) => {
                        this.file_error = Some(save_error(&path, &reason));
                        false
                    }
                };
                cx.notify();
                saved
            })
            .unwrap_or(false)
        });
        cx.notify();
        task
    }
    /// Saves every edited file; resolves to whether all of them saved.
    fn save_all_drafts(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let paths = self.dirty_drafts();
        let saves = paths
            .into_iter()
            .map(|path| self.save_draft(path, cx))
            .collect::<Vec<_>>();
        cx.spawn(async move |_, _| {
            let mut saved = true;
            for save in saves {
                saved &= save.await;
            }
            saved
        })
    }
    fn dirty_drafts(&self) -> Vec<PathBuf> {
        let mut paths = self
            .file_drafts
            .iter()
            .filter(|(_, draft)| draft.text != draft.original)
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
    /// Drops every unsaved edit, as "Don't save" does before a tab closes.
    fn discard_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for path in self.dirty_drafts() {
            let Some(draft) = self.file_drafts.get_mut(&path) else {
                continue;
            };
            draft.text = draft.original.clone();
            if let Some(editor) = self.editors.get(&path) {
                editor.update(cx, |editor, cx| {
                    editor.set_value(draft.text.clone(), window, cx)
                });
            }
        }
        if let Viewer::Text {
            path,
            markdown: Some(markdown),
            ..
        } = &mut self.viewer
            && let Some(draft) = self.file_drafts.get(path)
        {
            *markdown = draft.text.clone().into();
        }
        self.file_error = None;
        cx.notify();
    }
    /// The question a close asks when this tab holds unsaved file edits.
    fn unsaved_close_title(&self) -> Option<String> {
        let dirty = self.dirty_drafts();
        match dirty.as_slice() {
            [] => None,
            [path] => Some(format!("Save changes to {}?", file_label(path))),
            _ => Some(format!(
                "Save changes to {}?",
                crate::ui::plural(dirty.len(), "file", "files")
            )),
        }
    }
    /// Asks before closing a tab with unsaved edits: Save, Don't save or Cancel.
    /// `close` runs once the edits are saved or discarded, and repeats the close.
    /// Returns false when the tab may close right away.
    pub(crate) fn confirm_close_with_edits(
        panel: &Entity<RightPanel>,
        title: Option<String>,
        saving: bool,
        close: CloseTab,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(title) = title else {
            if saving {
                window.push_notification(
                    gpui_kit::component::notification::Notification::info(
                        "Wait for the file to finish saving, then close this tab.",
                    ),
                    cx,
                );
            }
            return saving;
        };
        let panel = panel.downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (save_panel, discard_panel) = (panel.clone(), panel.clone());
            let (save_close, discard_close) = (close.clone(), close.clone());
            dialog
                .title(title.clone())
                .w(zpx(360.))
                .close_button(false)
                .bg(crate::appearance::popup_surface())
                .entrance(gpui_kit::component::dialog::DialogEntrance::FadeSlide)
                .child(
                    div()
                        .text_color(crate::ui::secondary_text())
                        .child("Your edits will be lost if you don't save them."),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap(zpx(crate::ui::SPACE_SM))
                        .child(
                            Button::new("unsaved-discard")
                                .ghost()
                                .bg(rgba(crate::appearance::QUIET_FILL))
                                .label("Don't save")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let discarded = discard_panel
                                        .update(cx, |panel, cx| panel.discard_drafts(window, cx))
                                        .is_ok();
                                    if discarded {
                                        discard_close(window, cx);
                                    }
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("unsaved-cancel")
                                .ghost()
                                .bg(rgba(crate::appearance::QUIET_FILL))
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("unsaved-save")
                                .primary()
                                .label("Save")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let Ok(saved) = save_panel
                                        .update(cx, |panel, cx| panel.save_all_drafts(cx))
                                    else {
                                        return;
                                    };
                                    let close = save_close.clone();
                                    // A failed save keeps the tab open with its error showing.
                                    window
                                        .spawn(cx, async move |cx| {
                                            if saved.await {
                                                let _ = cx.update(|window, cx| close(window, cx));
                                            }
                                        })
                                        .detach();
                                }),
                        ),
                )
        });
        true
    }
    /// Revert drops every edit to the file, so it asks first.
    fn confirm_revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path() else {
            return;
        };
        if !self.selected_dirty() || self.saving.contains(&path) {
            return;
        }
        let panel = cx.entity().downgrade();
        let title = format!("Revert {}?", file_label(&path));
        window.open_dialog(cx, move |dialog, _, _| {
            let panel = panel.clone();
            let reverted = path.clone();
            dialog
                .title(title.clone())
                .w(zpx(360.))
                .close_button(false)
                .bg(crate::appearance::popup_surface())
                .entrance(gpui_kit::component::dialog::DialogEntrance::FadeSlide)
                .child(
                    div()
                        .text_color(crate::ui::secondary_text())
                        .child("This discards your unsaved edits to this file."),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap(zpx(crate::ui::SPACE_SM))
                        .child(
                            Button::new("revert-cancel")
                                .ghost()
                                .bg(rgba(crate::appearance::QUIET_FILL))
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("revert-confirm")
                                .danger()
                                .label("Revert")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(gpui_kit::component::dialog::Confirm {
                                            secondary: false,
                                        }),
                                        cx,
                                    )
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let path = reverted.clone();
                    let _ = panel.update(cx, |panel, cx| {
                        // Only the file that was asked about, if it is still showing.
                        if panel.selected_path() == Some(path) {
                            panel.revert_selected(window, cx);
                        }
                    });
                    true
                })
        });
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
        // Polls that finish quickly never flash "Loading changes…".
        self.slow_changes_load = false;
        let delay = cx.background_executor().timer(crate::ui::LOADING_DELAY);
        cx.spawn(async move |this, cx| {
            delay.await;
            let _ = this.update(cx, |this, cx| {
                if this.change_request == request && this.changes_loading {
                    this.slow_changes_load = true;
                    cx.notify();
                }
            });
        })
        .detach();
        let root = self.root.clone();
        let previous_root = self.change_root.clone();
        let previous_commits = self.change_commits.clone();
        let work = cx.background_executor().spawn(async move {
            let (root, mut files) = crate::workspace_files::changes(&root)?;
            let commits = if files.is_empty() {
                if root == previous_root
                    && previous_commits.first().is_some_and(|commit| {
                        crate::workspace_files::head_id(&root).as_deref()
                            == Some(commit.id.as_str())
                    })
                {
                    previous_commits
                } else {
                    crate::workspace_files::recent_commits(&root)?
                }
            } else {
                Vec::new()
            };
            let omitted = files.len().saturating_sub(MAX_REVIEW_FILES);
            files.truncate(MAX_REVIEW_FILES);
            let head = crate::workspace_files::has_head(&root);
            let diffs = files
                .iter()
                .map(|file| {
                    (
                        file.path.clone(),
                        crate::workspace_files::diff(&root, file, head)
                            .unwrap_or_else(|_| "Could not read this diff.".to_owned()),
                    )
                })
                .collect::<HashMap<_, _>>();
            Ok::<_, String>((root, files, omitted, diffs, commits))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if request != this.change_request {
                    return;
                }
                this.changes_loading = false;
                // "Loading changes…" shows only while the list is empty.
                let minute = chrono::Local::now().timestamp() / 60;
                let mut changed = (this.change_files.is_empty() && this.change_commits.is_empty())
                    || (!this.change_commits.is_empty() && this.change_date_minute != minute);
                this.change_date_minute = minute;
                match result {
                    Ok((root, files, omitted, diffs, commits)) => {
                        if this.change_error.is_none()
                            && this.change_root == root
                            && this.change_files == files
                            && this.change_commits == commits
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
                            if this.change_root != root {
                                this.commit_reviews.clear();
                            } else if !commits.is_empty() {
                                this.commit_reviews
                                    .retain(|id, _| commits.iter().any(|c| &c.id == id));
                            }
                            this.change_root = root;
                            this.change_error = None;
                            this.change_expanded.retain(|path| diffs.contains_key(path));
                            this.change_files = files;
                            this.change_commits = commits;
                            this.change_omitted = omitted;
                            this.change_diffs = diffs;
                        }
                    }
                    Err(error) => {
                        if this.change_error.as_ref() != Some(&error) {
                            changed = true;
                            this.change_error_details = false;
                        }
                        this.change_error = Some(error);
                        this.change_files.clear();
                        this.change_commits.clear();
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
        if self.change_files.is_empty() && self.change_commits.is_empty() {
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
            .when_some(self.change_error.as_deref(), |view, error| {
                view.child(self.render_change_error(error, cx))
            })
            .when(
                self.change_files.is_empty()
                    && self.change_commits.is_empty()
                    && self.change_error.is_none(),
                |view| {
                    view.child(if self.changes_loading {
                        loading_note("Loading changes…", self.slow_changes_load)
                    } else {
                        crate::ui::empty_note("No commits yet").p(zpx(crate::ui::SPACE_MD))
                    })
                },
            )
            .when(!self.change_commits.is_empty(), |view| {
                view.child(
                    v_flex()
                        .debug_selector(|| "changes-history".into())
                        .w_full()
                        .min_w_0()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .px(zpx(crate::ui::SPACE_MD))
                                .py(zpx(crate::ui::SPACE_SM))
                                .gap(zpx(crate::ui::SPACE_SM))
                                .text_size(zpx(TEXT_SM))
                                .text_color(crate::appearance::secondary_text())
                                .child(crate::ui::section_label("Recent commits"))
                                .child(format!("{} total", self.change_commits.len())),
                        )
                        .children(
                            self.change_commits
                                .iter()
                                .enumerate()
                                .map(|(index, commit)| self.render_commit(commit, index, cx)),
                        ),
                )
            })
            .when(!self.change_files.is_empty(), |view| {
                view.child(
                    h_flex()
                        .debug_selector(|| "changes-files".into())
                        .px(zpx(crate::ui::SPACE_MD))
                        .py(zpx(crate::ui::SPACE_SM))
                        .gap(zpx(crate::ui::SPACE_SM))
                        .text_size(zpx(TEXT_SM))
                        .text_color(crate::appearance::secondary_text())
                        .child(crate::ui::plural(total_files, "file", "files"))
                        .child(
                            div()
                                .text_color(ActionTone::Success.foreground())
                                .child(format!("+{}", crate::ui::group_digits(total_added as u64))),
                        )
                        .child(
                            div()
                                .text_color(ActionTone::Danger.foreground())
                                .child(format!(
                                    "−{}",
                                    crate::ui::group_digits(total_removed as u64)
                                )),
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
                            .px(zpx(crate::ui::SPACE_MD))
                            .py(zpx(crate::ui::SPACE_SM))
                            .gap(zpx(crate::ui::SPACE_SM))
                            .border_t_1()
                            .border_color(crate::appearance::separator())
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
                            .hover(|row| row.bg(crate::ui::row_hover()))
                            .child(
                                crate::icons::icon(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .with_size(zpx(11.))
                                .text_color(crate::ui::faint_text()),
                            )
                            .child(commit_review::file_icon(&path))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(path.display().to_string()),
                            )
                            // A zero side says nothing, as in the status bar and the transcript.
                            .when(added > 0, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(ActionTone::Success.foreground())
                                        .child(format!("+{added}")),
                                )
                            })
                            .when(removed > 0, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(ActionTone::Danger.foreground())
                                        .child(format!("−{removed}")),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.change_expanded.remove(&path) {
                                    this.change_expanded.insert(path.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when_some(review.filter(|_| expanded), |view, review| {
                        view.child(render_review(("file-diff", index).into(), review))
                    })
            }))
            .when(self.change_omitted > 0, |view| {
                view.child(
                    div()
                        .px(zpx(crate::ui::SPACE_MD))
                        .py(zpx(crate::ui::SPACE_SM))
                        .border_t_1()
                        .border_color(crate::appearance::separator())
                        .text_size(zpx(TEXT_SM))
                        .text_color(crate::appearance::secondary_text())
                        .child(format!(
                            "{} not shown",
                            crate::ui::plural(
                                self.change_omitted,
                                "more changed file",
                                "more changed files"
                            )
                        )),
                )
            })
            .into_any_element()
    }

    /// A plain sentence for a Git failure; Git's own words stay behind Details.
    fn render_change_error(&self, raw: &str, cx: &mut Context<Self>) -> AnyElement {
        let problem = crate::workspace_files::explain_git_error(raw);
        if problem.quiet {
            return crate::ui::empty_note(problem.summary)
                .p(zpx(crate::ui::SPACE_MD))
                .whitespace_normal()
                .into_any_element();
        }
        let open = self.change_error_details;
        v_flex()
            .debug_selector(|| "changes-error".into())
            .p(zpx(crate::ui::SPACE_MD))
            .gap(zpx(crate::ui::SPACE_XS))
            .whitespace_normal()
            .child(crate::ui::inline_error(problem.summary))
            .when_some(problem.details, |view, details| {
                view.child(
                    h_flex()
                        .id("changes-error-details")
                        .keyboard_control()
                        .role(gpui::Role::Button)
                        .aria_label(if open { "Hide details" } else { "Show details" })
                        .gap(zpx(crate::ui::SPACE_XS))
                        .items_center()
                        .cursor_pointer()
                        .text_size(zpx(TEXT_SM))
                        .text_color(crate::ui::faint_text())
                        .hover(|row| row.text_color(crate::ui::secondary_text()))
                        .child(
                            crate::icons::icon(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .with_size(zpx(11.)),
                        )
                        .child("Details")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.change_error_details = !this.change_error_details;
                            cx.notify();
                        })),
                )
                .when(open, |view| {
                    view.child(
                        div()
                            .font_family(MONO)
                            .text_size(zpx(TEXT_SM))
                            .text_color(crate::ui::secondary_text())
                            .child(details),
                    )
                })
            })
            .into_any_element()
    }

    fn terminal_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(terminal) = self
            .terminals
            .get(&self.terminal_tabs[self.terminal_active])
        {
            let view = terminal.read(cx).terminal_view();
            crate::terminal_host::apply_shell_theme(&view, cx);
            return div().size_full().min_h_0().child(view).into_any_element();
        }
        let Some(error) = self.terminal_error.clone() else {
            return crate::ui::empty_note("Opening terminal…")
                .p(zpx(crate::ui::SPACE_MD))
                .into_any_element();
        };
        v_flex()
            .debug_selector(|| "terminal-error".into())
            .p(zpx(crate::ui::SPACE_MD))
            .gap(zpx(crate::ui::SPACE_SM))
            .items_start()
            .whitespace_normal()
            .child(crate::ui::inline_error("Could not open a shell"))
            .child(
                div()
                    .text_size(zpx(TEXT_SM))
                    .text_color(crate::ui::faint_text())
                    .child(error),
            )
            .child(
                Button::new("retry-terminal")
                    .small()
                    .ghost()
                    .bg(rgba(crate::appearance::QUIET_FILL))
                    .label("Retry")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.ensure_terminal(window, cx);
                        this.focus_terminal(window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

impl RightPanel {
    /// The same tab controls can be mounted in the shell title bar without
    /// duplicating the tool content or changing tab ownership.
    pub fn set_tabs_in_titlebar(&mut self, detached: bool) {
        self.tabs_in_titlebar = detached;
    }

    pub fn render_tab_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.tab_drag.is_some() && !cx.has_active_drag() {
            self.tab_drag = None;
        }
        let transfer_bounds = self.transfer_bounds.clone();
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
            .px(zpx(crate::ui::SPACE_SM))
            .gap(zpx(crate::ui::SPACE_2XS))
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
                            .on_drop(cx.listener(|this, drag: &DraggedSidebarTab, _, cx| {
                                if drag.owner == cx.entity_id() {
                                    this.finish_tab_drag(cx);
                                }
                            }))
                            .track_scroll(&self.tool_tab_scroll)
                            .flex_shrink_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .gap(zpx(crate::ui::SPACE_2XS))
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
                    .keyboard_control()
                    .ml(zpx(crate::ui::SPACE_SM))
                    .debug_selector(|| "new-sidebar-tab".into())
                    .role(gpui::Role::Button)
                    .aria_label("New sidebar tab")
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("New tab")
                            .action(&crate::NewWorkspaceTab, None)
                            .build(window, cx)
                    })
                    .size(zpx(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .rounded(zpx(crate::ui::RADIUS_ROW))
                    .cursor_pointer()
                    .text_color(crate::ui::secondary_text())
                    .hover(|button| button.bg(crate::ui::row_selected()))
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_tab(window, cx)))
                    .motion_child(true, move |feedback| feedback.glyph(Glyph::Add)),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                Pane::SideHistory => self.render_side_history(window, cx),
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
            .when(!self.tabs_in_titlebar, |panel| {
                panel.child(self.render_tab_strip(cx))
            })
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

/// A group title above a list (13 px Medium, primary). Smaller list headings
/// such as "Recent commits" use `ui::section_label` instead.
fn group_title(label: &'static str) -> Div {
    div()
        .text_size(zpx(TEXT_MD))
        .font_weight(FontWeight::MEDIUM)
        .text_color(crate::ui::primary_text())
        .child(label)
}

/// "Last prompt 4m ago", "Last prompt just now"; saved-time fallbacks for older
/// snapshots read "Saved 4m ago", "Saved just now".
fn side_history_recency(age: Option<String>, estimated: bool) -> String {
    let label = if estimated { "Saved" } else { "Last prompt" };
    match age.as_deref() {
        None => format!("{label} earlier"),
        Some("now") => format!("{label} just now"),
        Some(age) => format!("{label} {age} ago"),
    }
}

/// A file's name for people: no directories, no OS error codes.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::ui::display_path(&path.to_string_lossy()))
}

/// "Could not save notes.txt: the file no longer exists."
fn save_error(path: &Path, reason: &str) -> String {
    format!("Could not save {}: {reason}", file_label(path))
}

/// A quiet loading line that appears only once a load outlasts `ui::LOADING_DELAY`.
fn loading_note(text: &'static str, revealed: bool) -> Div {
    if revealed {
        crate::ui::empty_note(text).p(zpx(crate::ui::SPACE_MD))
    } else {
        div()
    }
}

/// A file action: a quiet neutral button, blue (the action accent) only for a Save with work to keep.
fn file_action(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    primary: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .when(enabled, |button| button.keyboard_control())
        .role(gpui::Role::Button)
        .aria_label(label)
        .px(zpx(crate::ui::SPACE_SM))
        .h(zpx(24.))
        .flex()
        .items_center()
        .flex_none()
        .rounded(zpx(crate::ui::RADIUS_ROW))
        .text_size(zpx(TEXT_SM))
        .map(|button| {
            if primary {
                button
                    .bg(ActionTone::Accent.surface(false))
                    .text_color(ActionTone::Accent.foreground())
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(|button| button.bg(ActionTone::Accent.surface(true)))
                    })
            } else if enabled {
                button
                    .bg(rgba(crate::appearance::QUIET_FILL))
                    .text_color(crate::appearance::secondary_text())
                    .cursor_pointer()
                    .hover(|button| button.bg(crate::ui::row_selected()))
            } else {
                button
                    .text_color(crate::appearance::faint_text())
                    .opacity(0.7)
            }
        })
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
            message: format!(
                "{} KB: too large to show",
                crate::ui::group_digits(m.len() / 1024)
            ),
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

    #[::gpui::test]
    fn detached_header_controls_the_same_panel_without_an_extra_content_row(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture(Entity<RightPanel>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let header = self.0.update(cx, |panel, cx| panel.render_tab_strip(cx));
                v_flex()
                    .w(zpx(640.))
                    .h(zpx(500.))
                    .child(div().h(zpx(42.)).flex_none().child(header))
                    .child(div().flex_1().min_h_0().child(self.0.clone()))
            }
        }
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = RightPanel::new(dir.path().into(), window, cx);
                panel.set_tabs_in_titlebar(true);
                panel
            });
            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
            Fixture(panel)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let plus = cx.debug_bounds("new-sidebar-tab").unwrap();
        assert!(plus.center().y < zpx(42.));
        let panel = root.read_with(cx, |root, _| root.0.clone());
        cx.simulate_mouse_move(plus.center(), None, Modifiers::default());
        cx.simulate_mouse_down(plus.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(plus.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.sidebar_keys().len()),
            2
        );
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.close_active_tab(window, cx));
        });
        cx.run_until_parked();
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.sidebar_keys().len()),
            1
        );
        // Only the detached header owns the tab-strip bounds, above the body.
        panel.read_with(cx, |panel, _| {
            assert!(panel.transfer_bounds.get().bottom() <= zpx(42.));
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
    fn new_side_chats_are_independent_and_capture_latest_parent(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let manager = del_agent::SessionManager::new(
            dir.path().join("index"),
            [
                std::sync::Arc::new(del_agent::MockBackend::new(del_agent::Provider::Claude))
                    as std::sync::Arc<dyn del_agent::AgentBackend>,
            ],
        )
        .unwrap();
        let parent = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider: del_agent::Provider::Claude,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: del_agent::PermissionMode::FullAuto,
            }))
            .unwrap();
        let context = crate::side_chat::Context {
            activity_owner: None,
            accounts: vec![],
            backend: crate::chat::Backend {
                runtime: runtime.handle().clone(),
                manager: manager.clone(),
            },
            source: del_agent::ForkSource::Session(parent.id.clone()),
        };
        let (right, cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
            panel.set_side_context(context);
            panel.choose_new_tool(Pane::SideChat, window, cx);
            panel
        });
        let mut ids = Vec::new();
        for round in 0..2 {
            for _ in 0..300 {
                cx.run_until_parked();
                let ready = right.read_with(cx, |p, cx| {
                    p.hosted
                        .last()
                        .and_then(|t| t.panel.read(cx).side_chat.as_ref())
                        .or(p.side_chat.as_ref())
                        .and_then(|s| s.read(cx).session_id())
                        .cloned()
                });
                if let Some(id) = ready {
                    ids.push(id);
                    break;
                }
                runtime.block_on(async {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                });
            }
            assert_eq!(ids.len(), round + 1, "new side chat must own a new session");
            if round == 0 {
                runtime
                    .block_on(manager.send_prompt(
                        &parent.id,
                        del_agent::UserInput::text("LATEST_PARENT_MARKER"),
                    ))
                    .unwrap();
                runtime.block_on(async {
                    for _ in 0..300 {
                        if manager.status(&parent.id) == Some(del_agent::SessionStatus::Idle) {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                });
                cx.update(|window, cx| {
                    right.update(cx, |p, cx| p.choose_new_tool(Pane::SideChat, window, cx))
                });
            }
        }
        assert_ne!(ids[0], ids[1]);
        let snapshots: Vec<_> = ids
            .iter()
            .map(|id| serde_json::to_string(&manager.export_fork(id).unwrap()).unwrap())
            .collect();
        assert!(!snapshots[0].contains("LATEST_PARENT_MARKER"));
        assert!(snapshots[1].contains("LATEST_PARENT_MARKER"));
        cx.update(|window, cx| {
            right.update(cx, |p, cx| {
                let first = p.hosted[0].panel.read(cx).side_chat_id.clone().unwrap();
                let second = p.hosted[1].panel.read(cx).side_chat_id.clone().unwrap();
                p.reopen_side_chat(first, window, cx);
                assert_eq!(
                    p.hosted.len(),
                    2,
                    "history selects an existing tab without forking again"
                );
                assert_eq!(p.active_hosted.as_ref(), Some(&p.hosted[0].id));
                p.reopen_side_chat(second, window, cx);
                assert_eq!(p.hosted.len(), 2);
                assert_eq!(p.active_hosted.as_ref(), Some(&p.hosted[1].id));
            })
        });
        right.read_with(cx, |p, cx| {
            assert_eq!(p.hosted.len(), 2);
            let saved = p.snapshot(true, cx);
            assert_eq!(saved.hosted.len(), 2);
            assert_ne!(
                saved.hosted[0].panel.side_chat_id,
                saved.hosted[1].panel.side_chat_id
            );
            let encoded = serde_json::to_vec(&saved).unwrap();
            let restored: SavedPanel = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(restored.active_hosted, p.active_hosted);
            for tab in restored.hosted {
                assert_eq!(tab.panel.open_panes, vec![Pane::SideChat]);
            }
        });
        cx.update(|window, cx| right.update(cx, |p, cx| p.close_active_tab(window, cx)));
        assert_eq!(right.read_with(cx, |p, _| p.hosted.len()), 1);
        assert!(manager.is_live(&ids[0]));
        right.update(cx, |p, cx| p.release(cx));
        runtime.block_on(manager.shutdown());
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
    fn side_history_is_a_single_closable_saved_sidebar_tab(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (_panel, _cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
            panel.show_side_history(window, cx);
            panel.show_side_history(window, cx);
            assert_eq!(panel.pane, Pane::SideHistory);
            let request = panel.side_history_request;
            panel.prepare_to_show(window, cx);
            panel.prepare_to_show(window, cx);
            assert_eq!(
                panel.side_history_request, request,
                "ordinary sidebar updates must not restart the read"
            );
            assert_eq!(
                panel
                    .open_panes
                    .iter()
                    .filter(|p| **p == Pane::SideHistory)
                    .count(),
                1
            );
            assert!(
                !panel.blank_tabs.is_empty(),
                "history preserves the launcher tab"
            );
            let saved: SavedPanel =
                serde_json::from_slice(&serde_json::to_vec(&panel.snapshot(true, cx)).unwrap())
                    .unwrap();
            panel.restore(saved, window, cx);
            assert_eq!(panel.pane, Pane::SideHistory);
            panel.close_active_tab(window, cx);
            assert!(!panel.open_panes.contains(&Pane::SideHistory));
            assert_eq!(panel.pane, Pane::NewTab);
            panel
        });
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
        cx.update(|cx| {
            gpui_kit::init(cx);
            // This checks settled geometry, independently of tab entrance timing.
            cx.set_reduce_motion(true);
        });
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
        let gap = plus.left() - tabs.right();
        assert!(
            gap >= zpx(8.) && gap <= zpx(16.),
            "plus should follow the tabs with a small breathing gap"
        );
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
            });
            // The standard dialog asks instead of a buttonless toast.
            assert!(window.has_active_dialog(cx));
        });
    }

    #[::gpui::test]
    fn unsaved_close_offers_save_and_dont_save(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let [first, second] = ["notes.txt", "todo.md"].map(|name| dir.path().join(name));
        std::fs::write(&first, "original").unwrap();
        std::fs::write(&second, "list").unwrap();
        let mut panel = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| legacy_panel(dir.path().into(), window, cx));
            panel = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let panel = panel.unwrap();
        let draft = |original: &str, text: &str| crate::workspace_files::FileDraft {
            original: original.into(),
            text: text.into(),
        };
        panel.update(cx, |panel, _| {
            // No live shell: closing Files must not fall through to a real PTY.
            panel.open_panes = vec![Pane::Files, Pane::Changes];
            panel
                .file_drafts
                .insert(first.clone(), draft("original", "saved"));
            assert_eq!(
                panel.close_guard(),
                (Some("Save changes to notes.txt?".into()), false)
            );
            panel
                .file_drafts
                .insert(second.clone(), draft("list", "longer list"));
            assert_eq!(
                panel.close_guard().0.as_deref(),
                Some("Save changes to 2 files?")
            );
        });
        // Save writes every edited file, then the tab may close.
        let outcome = Rc::new(Cell::new(None));
        let result = outcome.clone();
        panel.update(cx, |panel, cx| {
            let saved = panel.save_all_drafts(cx);
            cx.spawn(async move |_, _| result.set(Some(saved.await)))
                .detach();
        });
        cx.run_until_parked();
        assert_eq!(outcome.get(), Some(true));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "saved");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "longer list");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                assert!(!panel.has_unsaved_edits());
                panel.close_tool(Pane::Files, window, cx);
                assert!(!panel.open_panes.contains(&Pane::Files));
            });
            assert!(!window.has_active_dialog(cx));
        });
        // Don't save drops the edits and leaves the file on disk untouched.
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_tool(Pane::Files, window, cx);
                panel
                    .file_drafts
                    .insert(first.clone(), draft("saved", "discard me"));
                panel.discard_drafts(window, cx);
                assert!(!panel.has_unsaved_edits());
                panel.close_tool(Pane::Files, window, cx);
                assert!(!panel.open_panes.contains(&Pane::Files));
            })
        });
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "saved");
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
    fn saved_sidebar_visibility_is_respected_for_every_pane() {
        let mut saved = SavedPanel {
            open: true,
            pane: Pane::Changes,
            ..Default::default()
        };
        for pane in [
            Pane::Changes,
            Pane::Files,
            Pane::Terminal,
            Pane::SideChat,
            Pane::Browser,
            Pane::NewTab,
        ] {
            saved.pane = pane;
            saved.open = true;
            assert!(saved.restore_open());
            saved.open = false;
            assert!(!saved.restore_open());
        }
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
            assert!(panel.rows.iter().any(
                |row| matches!(row, TreeRow::Entry { path, .. } if path.ends_with(".hidden"))
            ));
        });
    }

    #[::core::prelude::v1::test]
    fn unloaded_expanded_folder_does_not_hide_available_siblings() {
        let root = PathBuf::from("/work");
        let slow = root.join("cloud");
        let file = root.join("file.txt");
        let cache = HashMap::from([(
            root.clone(),
            Listing::Entries {
                entries: vec![(slow.clone(), true), (file.clone(), false)],
                hidden: false,
            },
        )]);
        assert_eq!(
            cached_tree_rows(
                &root,
                &HashSet::from([slow.clone()]),
                &cache,
                &HashSet::new()
            ),
            vec![
                TreeRow::Entry {
                    path: slow,
                    depth: 0,
                    is_dir: true
                },
                TreeRow::Entry {
                    path: file,
                    depth: 0,
                    is_dir: false
                },
            ]
        );
    }

    #[::core::prelude::v1::test]
    fn folders_say_when_they_are_empty_unreadable_or_slow() {
        let root = PathBuf::from("/work");
        let [denied, broken, empty, dotted, slow, quick] =
            ["denied", "broken", "empty", "dotted", "slow", "quick"].map(|name| root.join(name));
        let folders = [&denied, &broken, &empty, &dotted, &slow, &quick];
        let cache = HashMap::from([
            (
                root.clone(),
                Listing::Entries {
                    entries: folders.iter().map(|path| ((*path).clone(), true)).collect(),
                    hidden: false,
                },
            ),
            (denied.clone(), Listing::Unreadable { denied: true }),
            (broken.clone(), Listing::Unreadable { denied: false }),
            (
                empty.clone(),
                Listing::Entries {
                    entries: Vec::new(),
                    hidden: false,
                },
            ),
            (
                dotted.clone(),
                Listing::Entries {
                    entries: Vec::new(),
                    hidden: true,
                },
            ),
        ]);
        let expanded = folders.iter().map(|path| (*path).clone()).collect();
        let rows = cached_tree_rows(&root, &expanded, &cache, &HashSet::from([slow.clone()]));
        let notes = rows
            .iter()
            .filter_map(|row| match row {
                TreeRow::Note {
                    parent,
                    depth,
                    note,
                } => Some((parent.clone(), *depth, *note)),
                TreeRow::Entry { .. } => None,
            })
            .collect::<Vec<_>>();
        // Each note sits directly below its folder, one level in; a read that is
        // pending but not yet slow shows nothing.
        assert_eq!(
            notes,
            vec![
                (denied, 1, TreeNote::Unreadable { denied: true }),
                (broken, 1, TreeNote::Unreadable { denied: false }),
                (empty, 1, TreeNote::Empty),
                (dotted, 1, TreeNote::OnlyHidden),
                (slow, 1, TreeNote::Loading),
            ]
        );
        assert_eq!(rows.len(), 6 + 5);
        assert!(matches!(
            &rows[1],
            TreeRow::Note {
                note: TreeNote::Unreadable { .. },
                ..
            }
        ));
        assert!(
            TreeNote::Unreadable { denied: true }
                .text()
                .contains("System Settings › Privacy & Security")
        );
    }

    #[::core::prelude::v1::test]
    fn unreadable_folders_report_why_instead_of_looking_empty() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_directory(&dir.path().join("missing"), false),
            Listing::Unreadable { denied: false }
        );
        std::fs::write(dir.path().join(".env"), "").unwrap();
        assert_eq!(
            read_directory(dir.path(), false),
            Listing::Entries {
                entries: Vec::new(),
                hidden: true
            }
        );
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let listing = read_directory(&locked, false);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root can read anything; everyone else is told why the folder is blank.
        let root = std::env::var("USER").is_ok_and(|user| user == "root");
        if !root {
            assert_eq!(listing, Listing::Unreadable { denied: true });
        }
    }

    #[::core::prelude::v1::test]
    fn side_history_recency_reads_the_same_way_for_both_sources() {
        assert_eq!(
            side_history_recency(Some("4m".into()), false),
            "Last prompt 4m ago"
        );
        assert_eq!(
            side_history_recency(Some("now".into()), false),
            "Last prompt just now"
        );
        assert_eq!(
            side_history_recency(Some("2h".into()), true),
            "Saved 2h ago"
        );
        assert_eq!(
            side_history_recency(Some("now".into()), true),
            "Saved just now"
        );
        assert_eq!(side_history_recency(None, true), "Saved earlier");
    }

    #[::core::prelude::v1::test]
    fn save_errors_name_the_file_without_its_path() {
        let path = PathBuf::from("/Users/someone/project/src/notes.txt");
        assert_eq!(
            save_error(&path, "the file no longer exists."),
            "Could not save notes.txt: the file no longer exists."
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
    fn changes_switch_between_history_and_pending_files(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new(crate::projects::git_program().unwrap())
                .arg("-C")
                .arg(root)
                .args([
                    "-c",
                    "user.name=Review Tester",
                    "-c",
                    "user.email=review@example.invalid",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        let path = root.join("example.txt");
        std::fs::write(&path, "before\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "Initial commit"]);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = legacy_panel(root.into(), window, cx);
            panel.pane = Pane::Changes;
            panel.prepare_to_show(window, cx);
            panel
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("changes-history").is_some());
        assert!(cx.debug_bounds("changes-files").is_none());
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.change_commits[0].subject, "Initial commit");
            assert_eq!(panel.change_commits[0].author, "Review Tester");
            assert_eq!(
                (
                    panel.change_commits[0].added,
                    panel.change_commits[0].removed
                ),
                (1, 0)
            );
        });
        let copy = cx.debug_bounds("history-copy-0").unwrap();
        cx.simulate_mouse_down(copy.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(copy.center(), MouseButton::Left, Default::default());
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(panel.change_commits[0].id.as_str())
            );
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("history-file-0-0").is_none(),
            "copy does not expand the commit"
        );
        // Exercise both disclosure levels through the rendered click targets.
        let commit_row = cx.debug_bounds("history-commit-0").unwrap();
        cx.simulate_mouse_down(commit_row.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(commit_row.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let file_row = cx.debug_bounds("history-file-0-0").unwrap();
        assert!(
            file_row.origin.x > commit_row.origin.x,
            "files are indented under the commit"
        );
        assert!(cx.debug_bounds("history-diff-0-0").is_none());
        cx.simulate_mouse_down(file_row.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(file_row.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        let diff = cx.debug_bounds("history-diff-0-0").unwrap();
        assert!(
            diff.origin.x > file_row.origin.x,
            "diff is nested under the file"
        );
        panel.update(cx, |panel, cx| panel.refresh_changes(cx));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("history-diff-0-0").is_some(),
            "polling retains expanded history"
        );
        cx.simulate_mouse_down(commit_row.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(commit_row.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("history-file-0-0").is_none());
        cx.simulate_mouse_down(commit_row.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(commit_row.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.draw(cx).clear(cx);
        });
        assert!(
            cx.debug_bounds("history-diff-0-0").is_some(),
            "reopening retains file disclosure"
        );
        // Untracked files, unstaged changes, and staged-only changes all hide history.
        for kind in ["untracked", "unstaged", "staged"] {
            if kind == "untracked" {
                std::fs::write(root.join("new.txt"), "new\n").unwrap();
            } else {
                std::fs::write(&path, "after\n").unwrap();
                if kind == "staged" {
                    git(&["add", "."]);
                }
            }
            panel.update(cx, |panel, cx| panel.refresh_changes(cx));
            cx.run_until_parked();
            assert!(cx.debug_bounds("changes-files").is_some(), "{kind}");
            assert!(cx.debug_bounds("changes-history").is_none(), "{kind}");
            panel.read_with(cx, |panel, _| assert!(panel.change_commits.is_empty()));
            if kind == "untracked" {
                std::fs::remove_file(root.join("new.txt")).unwrap();
            }
        }
        git(&["commit", "-qm", "Save changes café"]);
        panel.update(cx, |panel, cx| panel.refresh_changes(cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("changes-history").is_some());
        assert!(cx.debug_bounds("changes-files").is_none());
        panel.read_with(cx, |panel, _| {
            assert!(panel.change_files.is_empty());
            assert_eq!(panel.change_commits.len(), 2);
            assert_eq!(panel.change_commits[0].subject, "Save changes café");
            assert_eq!(
                (
                    panel.change_commits[0].added,
                    panel.change_commits[0].removed
                ),
                (1, 1)
            );
            assert_eq!(panel.change_commits[1].subject, "Initial commit");
        });
        // A clean-to-clean poll retains the commit list instead of showing Loading.
        panel.update(cx, |panel, cx| {
            panel.refresh_changes(cx);
            assert_eq!(panel.change_commits.len(), 2);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("changes-history").is_some());
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
    fn dotfiles_toggle_lives_in_files_content_and_still_filters(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), "hidden").unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| legacy_panel(dir.path().into(), window, cx));
        cx.run_until_parked();
        let toolbar = cx.debug_bounds("files-toolbar").unwrap();
        let toggle = cx.debug_bounds("files-dotfiles-toggle").unwrap();
        let tabs = cx.debug_bounds("sidebar-tool-tabs").unwrap();
        assert!(toolbar.origin.y >= tabs.bottom());
        assert!(toggle.origin.y >= toolbar.origin.y && toggle.bottom() <= toolbar.bottom());
        cx.simulate_mouse_down(toggle.center(), MouseButton::Left, Default::default());
        cx.simulate_mouse_up(toggle.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.show_hidden);
            assert!(panel.rows.iter().any(
                |row| matches!(row, TreeRow::Entry { path, .. } if path.ends_with(".hidden"))
            ));
        });
        panel.update(cx, |panel, cx| {
            panel.pane = Pane::Changes;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("files-dotfiles-toggle").is_none());
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
        assert!(restored.get("chat:a").restore_open());
        assert!(!restored.get("chat:b").restore_open());
        assert!(!restored.get("new-chat").restore_open());
        let legacy: SavedPanel = serde_json::from_str("{}").unwrap();
        assert!(!legacy.restore_open());
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
