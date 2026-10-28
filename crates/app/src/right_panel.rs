//! The right sidebar: files, a workspace PTY, and an embedded browser.

use crate::appearance::{rgb, rgba};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use del_core::typography::TEXT_SM;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
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

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Pane {
    #[default]
    Files,
    Terminal,
    Browser,
    Changes,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPanel {
    pub open: bool,
    pane: Pane,
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
            pane: Pane::Files,
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
    pane: Pane,
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

impl RightPanel {
    pub fn is_changes(&self) -> bool { self.pane == Pane::Changes }

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
        if self.changes_watch.is_none() {
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
            pane: self.pane,
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
        self.pane = if saved.pane == Pane::Changes {
            Pane::Files
        } else {
            saved.pane
        };
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
        if saved.open && saved.pane != Pane::Changes {
            self.prepare_to_show(window, cx);
        }
    }

    pub fn new(root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            pane: Pane::Files,
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
        self.pane = Pane::Terminal;
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
        if request.action == "new_tab" && self.browser_tabs.len() >= del_mcp::browser::MAX_AGENT_TABS {
            return Err(format!(
                "This session already has {} browser tabs; close one first",
                del_mcp::browser::MAX_AGENT_TABS
            ));
        }
        if reveal {
            self.pane = Pane::Browser;
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
                                    .text_size(zpx(11.))
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
        self.pane = Pane::Files;
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
                    {
                        if let Some(draft) = this.file_drafts.get(path) {
                            *markdown = draft.text.clone().into();
                        }
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
        id: &'static str,
        label: &'static str,
        pane: Pane,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.pane == pane;
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label)
            .px_1p5()
            .h(zpx(24.0))
            .flex()
            .flex_none()
            .whitespace_nowrap()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(zpx(TEXT_SM))
            .text_color(if active {
                rgb(0xf5f5f5)
            } else {
                rgba(0xffffff8c)
            })
            .when(active, |t| t.bg(rgba(0xffffff14)))
            .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.pane != pane {
                    this.change_expanded.clear();
                }
                this.pane = pane;
                if pane == Pane::Terminal {
                    this.ensure_terminal(window, cx);
                    this.focus_terminal(window, cx);
                } else if pane == Pane::Browser {
                    this.ensure_browser(cx);
                } else if pane == Pane::Changes {
                    this.refresh_changes(cx);
                } else {
                    this.refresh_tree(cx);
                }
                cx.notify();
            }))
            .child(label)
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
                    .h(zpx(22.0))
                    .flex_none()
                    .pl(zpx(4.0 + depth as f32 * 12.0))
                    .pr_1()
                    .child(
                        h_flex()
                            .id(SharedString::from(path.display().to_string()))
                            .h(zpx(22.0))
                            .min_w_0()
                            .px_1()
                            .gap_1p5()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(zpx(TEXT_SM))
                            .text_color(if is_dir {
                                rgb(0xf0f0f0)
                            } else {
                                rgba(0xffffffcc)
                            })
                            .when(is_selected, |r| {
                                r.bg(rgba(0xffffff1f)).text_color(rgb(0xf5f5f5))
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
                                .text_color(muted),
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
                .text_size(zpx(TEXT_SM))
                .text_color(muted)
                .child("Select a file to view it")
                .into_any_element(),
            Viewer::Loading { .. } => div()
                .p_3()
                .text_size(zpx(TEXT_SM))
                .text_color(muted)
                .child("Loading file…")
                .into_any_element(),
            Viewer::Note { message, .. } => div()
                .p_3()
                .whitespace_nowrap()
                .text_size(zpx(TEXT_SM))
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
                        .text_size(zpx(TEXT_SM))
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
                                .text_size(zpx(TEXT_SM))
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
                                .text_size(zpx(TEXT_SM))
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
                                            .text_size(zpx(11.0))
                                            .cursor_pointer()
                                            .when(self.markdown_preview == preview, |button| {
                                                button.bg(rgba(0xffffff20))
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
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_selected(cx))),
                        )
                        .child(action("revert-file", "Revert").on_click(
                            cx.listener(|this, _, window, cx| this.revert_selected(window, cx)),
                        ))
                        .child(
                            action("reload-file", "Reload")
                                .on_click(cx.listener(|this, _, _, cx| this.reload_selected(cx))),
                        ),
                )
            })
            .when_some(self.file_error.clone(), |preview, error| {
                preview.child(
                    div()
                        .p_2()
                        .text_size(zpx(11.))
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
                {
                    if path == &edited {
                        *markdown = text.into();
                    }
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
        self.terminal_tabs.push(uuid::Uuid::new_v4().to_string());
        self.terminal_active = self.terminal_tabs.len() - 1;
        self.ensure_terminal(window, cx);
        self.focus_terminal(window, cx);
        cx.notify();
    }
    fn close_terminal_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.terminal_tabs.remove(index);
        if let Some(terminal) = self.terminals.remove(&id) {
            terminal.update(cx, |terminal, cx| terminal.close(cx));
        }
        if self.terminal_tabs.is_empty() {
            self.terminal_tabs.push(uuid::Uuid::new_v4().to_string());
        }
        self.terminal_active = if index < self.terminal_active {
            self.terminal_active - 1
        } else {
            self.terminal_active
        }
        .min(self.terminal_tabs.len() - 1);
        self.ensure_terminal(window, cx);
        self.focus_terminal(window, cx);
        cx.notify();
    }
    fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                h_flex()
                    .h(zpx(28.))
                    .flex_none()
                    .px_2()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("terminal-tabs")
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .children(self.terminal_tabs.iter().enumerate().map(|(index, id)| {
                                let title = self
                                    .terminals
                                    .get(id)
                                    .map(|terminal| terminal.read(cx).snapshot().title)
                                    .filter(|title| !title.is_empty())
                                    .unwrap_or_else(|| format!("Terminal {}", index + 1));
                                h_flex()
                                    .id(("terminal-tab", index))
                                    .h(crate::icons::button_size())
                                    .max_w(zpx(150.))
                                    .px_2()
                                    .gap_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(zpx(11.))
                                    .when(index == self.terminal_active, |row| {
                                        row.bg(rgba(0xffffff14))
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.terminal_active = index;
                                        this.ensure_terminal(window, cx);
                                        this.focus_terminal(window, cx);
                                        cx.notify();
                                    }))
                                    .child(div().truncate().child(title))
                                    .child(
                                        div()
                                            .id(("close-terminal-tab", index))
                                            .size(crate::icons::button_size())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .role(gpui::Role::Button)
                                            .aria_label("Close terminal tab")
                                            .child(
                                                crate::icons::icon(IconName::Close)
                                                    .with_size(zpx(14.)),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.close_terminal_tab(index, window, cx);
                                            })),
                                    )
                            })),
                    )
                    .child(
                        action("new-terminal-tab", "+")
                            .aria_label("New terminal tab")
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.new_terminal_tab(window, cx)
                                }),
                            ),
                    ),
            )
            .when_some(self.file_error.clone(), |view, error| {
                view.child(
                    div()
                        .p_2()
                        .text_size(zpx(11.))
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
                                        review.unwrap_or_else(|| Rc::new(ChangeReview::parse(text))),
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
        let (total_added, total_removed) = self.change_reviews.values()
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
                        .text_size(zpx(TEXT_SM))
                        .child(error),
                )
            })
            .when(
                self.change_files.is_empty() && self.change_error.is_none(),
                |view| {
                    view.child(
                        div()
                            .p_3()
                            .text_size(zpx(TEXT_SM))
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
                        .text_size(zpx(11.))
                        .text_color(rgba(0xffffff8c))
                        .child(format!("{} {}", total_files, if total_files == 1 { "file" } else { "files" }))
                        .child(div().text_color(rgb(0x5fc27e)).child(format!("+{total_added}")))
                        .child(div().text_color(rgb(0xf0716b)).child(format!("−{total_removed}"))),
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
                            .text_size(zpx(TEXT_SM))
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
                                        .text_size(zpx(11.))
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
                                            .text_size(zpx(11.))
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
                        .text_size(zpx(11.))
                        .text_color(rgba(0xffffff8c))
                        .child(format!(
                            "{} more changed {} not shown",
                            self.change_omitted,
                            if self.change_omitted == 1 { "file" } else { "files" }
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
            .text_size(zpx(TEXT_SM))
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
        };
        v_flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            // Nothing wraps while the panel's width animates.
            .whitespace_nowrap()
            .flex_none()
            .child(
                h_flex()
                    .h(zpx(32.0))
                    .flex_none()
                    .overflow_hidden()
                    .px_2()
                    .gap_0p5()
                    .items_center()
                    .child(self.tab("tab-files", "Files", Pane::Files, cx))
                    .child(self.tab("tab-changes", "Changes", Pane::Changes, cx))
                    .child(self.tab("tab-terminal", "Terminal", Pane::Terminal, cx))
                    .child(self.tab("tab-browser", "Browser", Pane::Browser, cx))
                    .when(self.pane == Pane::Files, |row| {
                        row.child(div().flex_1()).child(
                            h_flex()
                                .id("show-hidden-files")
                                .gap(zpx(5.0))
                                .items_center()
                                .cursor_pointer()
                                .opacity(0.55)
                                .hover(|control| control.opacity(0.82))
                                .text_size(zpx(11.0))
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
                                            rgba(0x448aff80)
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
    }
}

fn action(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .px_1p5()
        .h(zpx(24.))
        .flex()
        .items_center()
        .flex_none()
        .rounded_md()
        .text_size(zpx(11.))
        .cursor_pointer()
        .hover(|button| button.bg(rgba(0xffffff14)))
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
    fn saved_changes_requires_an_explicit_click() {
        let mut saved = SavedPanel::default();
        saved.open = true;
        saved.pane = Pane::Changes;
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
        // Header 32px + tree 220px + middle of the 8px handle.
        cx.simulate_mouse_down(
            point(px(100.0), px(256.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(100.0), px(276.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(px(100.0), px(296.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(100.0), px(296.0)),
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
