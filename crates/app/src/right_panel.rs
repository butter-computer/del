//! The right sidebar: a read-only file viewer and an interactive workspace PTY.

use crate::appearance::{rgb, rgba};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use del_core::typography::{TEXT_MD, TEXT_SM};
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
/// Largest file shown, and most lines rendered.
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FILE_LINES: usize = 3000;
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
        lines: std::sync::Arc<Vec<String>>,
        widest: usize,
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
    terminal: Option<Entity<TerminalHost>>,
    terminal_error: Option<String>,
    show_hidden: bool,
    file_tree_height: f32,
    file_request: u64,
    tree_request: u64,
    directory_entries: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    split_origin: Rc<Cell<f32>>,
    rows: Vec<(PathBuf, usize, bool)>,
    file_scroll: UniformListScrollHandle,
    markdown_preview: bool,
    markdown_scroll: ScrollHandle,
}

impl RightPanel {
    pub fn set_preview_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !visible && let Some(preview) = &self.document_preview {
            preview.hide();
        }
        if self.preview_visible != visible {
            self.preview_visible = visible;
            cx.notify();
        }
    }

    pub fn prepare_to_show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane == Pane::Terminal {
            self.ensure_terminal(window, cx);
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
        }
    }

    pub fn restore(&mut self, saved: SavedPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = saved.pane;
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
        if saved.open && self.pane == Pane::Terminal {
            self.ensure_terminal(window, cx);
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
            terminal: None,
            terminal_error: None,
            show_hidden: false,
            file_tree_height: DEFAULT_TREE_HEIGHT,
            file_request: 0,
            tree_request: 0,
            directory_entries: HashMap::new(),
            split_origin: Default::default(),
            rows: Vec::new(),
            file_scroll: UniformListScrollHandle::new(),
            markdown_preview: false,
            markdown_scroll: ScrollHandle::new(),
        };
        panel.refresh_tree(cx);
        panel
    }

    /// Point the panel at another project folder.
    pub fn set_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root != root {
            if let Some(terminal) = self.terminal.take() {
                terminal.update(cx, |terminal, cx| terminal.close(cx));
            }
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

    fn ensure_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.is_some() {
            return;
        }
        match TerminalHost::spawn(TerminalLaunchSpec::shell(self.root.clone()), window, cx) {
            Ok(terminal) => {
                self.terminal = Some(terminal);
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
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
        }
    }

    pub fn preview_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.pane = Pane::Files;
        self.open_file(path, cx);
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.file_request += 1;
        let request = self.file_request;
        self.viewer = Viewer::Loading { path: path.clone() };
        self.file_scroll = UniformListScrollHandle::new();
        self.markdown_scroll = ScrollHandle::new();
        let load = cx
            .background_executor()
            .spawn(async move { read_viewer(path) });
        cx.spawn(async move |this, cx| {
            let viewer = load.await;
            let _ = this.update(cx, |this, cx| {
                if this.file_request == request {
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
            .px_2()
            .h(px(24.0))
            .flex()
            .flex_none()
            .whitespace_nowrap()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(TEXT_MD))
            .text_color(if active {
                rgb(0xf5f5f5)
            } else {
                rgba(0xffffff8c)
            })
            .when(active, |t| t.bg(rgba(0xffffff14)))
            .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.pane = pane;
                if pane == Pane::Terminal {
                    this.ensure_terminal(window, cx);
                    this.focus_terminal(window, cx);
                } else {
                    this.refresh_tree(cx);
                }
                cx.notify();
            }))
            .child(label)
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .h(px(self.file_tree_height))
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
                    .h(px(22.0))
                    .flex_none()
                    .pl(px(4.0 + depth as f32 * 12.0))
                    .pr_1()
                    .child(
                        h_flex()
                            .id(SharedString::from(path.display().to_string()))
                            .h(px(22.0))
                            .min_w_0()
                            .px_1()
                            .gap_1p5()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(TEXT_SM))
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
                                Icon::new(if is_dir {
                                    if open {
                                        IconName::FolderOpen
                                    } else {
                                        IconName::Folder
                                    }
                                } else {
                                    IconName::File
                                })
                                .xsmall()
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
                .text_size(px(TEXT_SM))
                .text_color(muted)
                .child("Select a file to view it")
                .into_any_element(),
            Viewer::Loading { .. } => div()
                .p_3()
                .text_size(px(TEXT_SM))
                .text_color(muted)
                .child("Loading file…")
                .into_any_element(),
            Viewer::Note { message, .. } => div()
                .p_3()
                .whitespace_nowrap()
                .text_size(px(TEXT_SM))
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
                        .text_size(px(TEXT_SM))
                        .child(
                            TextView::markdown(
                                ("file-markdown", self.file_request),
                                markdown.clone(),
                            )
                            .selectable(true),
                        ),
                )
                .into_any_element(),
            Viewer::Text { lines, widest, .. } => {
                let lines = lines.clone();
                uniform_list("file-text", lines.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            h_flex()
                                .h(px(20.0))
                                .gap_2()
                                .px_2()
                                .font_family(MONO)
                                .text_size(px(TEXT_SM))
                                .child(
                                    div()
                                        .w(px(32.0))
                                        .flex_none()
                                        .text_right()
                                        .text_color(rgba(0xffffff40))
                                        .child((i + 1).to_string()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .whitespace_nowrap()
                                        .child(lines[i].clone()),
                                )
                        })
                        .collect()
                })
                .size_full()
                .with_width_from_item(Some(*widest))
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(&self.file_scroll)
                .into_any_element()
            }
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
            .rounded(px(10.0))
            .overflow_hidden()
            .when_some(selected, |preview, path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                preview.child(
                    h_flex()
                        .h(px(28.0))
                        .rounded_t(px(10.0))
                        .flex_none()
                        .px_2()
                        .gap_2()
                        .bg(rgba(0xffffff08))
                        .items_center()
                        .child(Icon::new(IconName::File).xsmall().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_SM))
                                .child(name),
                        )
                        .when(is_markdown, |header| {
                            header.child(h_flex().gap_0p5().flex_none().children(
                                [(false, "Markdown"), (true, "Preview")].into_iter().map(
                                    |(preview, label)| {
                                        div()
                                            .id(label)
                                            .role(gpui::Role::Button)
                                            .aria_label(label)
                                            .h(px(20.0))
                                            .px_1p5()
                                            .flex()
                                            .items_center()
                                            .rounded_md()
                                            .text_size(px(11.0))
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
                                .size(px(22.0))
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
                                    this.file_scroll = UniformListScrollHandle::new();
                                    cx.notify();
                                }))
                                .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                        ),
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
                    let delta = event.event.position.y.as_f32() - drag.start_y.get();
                    this.file_tree_height = resized_tree_height(
                        drag.start_height,
                        delta,
                        event.bounds.size.height.as_f32(),
                    );
                    cx.notify();
                }),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .h(px(self.file_tree_height))
                    .overflow_hidden()
                    .child(tree),
            )
            .child(
                div()
                    .id("file-preview-resize")
                    .group("file-preview-resize")
                    .h(px(8.0))
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
                            .h(px(1.0))
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

    fn render_terminal(&self, cx: &App) -> AnyElement {
        if let Some(terminal) = &self.terminal {
            return div()
                .size_full()
                .min_h_0()
                .child(terminal.read(cx).terminal_view())
                .into_any_element();
        }
        div()
            .p_3()
            .text_size(px(TEXT_SM))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if (!self.preview_visible
            || self.pane != Pane::Files
            || !matches!(self.viewer, Viewer::Document { .. }))
            && let Some(preview) = &self.document_preview
        {
            preview.hide();
        }
        let show_hidden = self.show_hidden;
        let body = match self.pane {
            Pane::Files => self.render_files(cx).into_any_element(),
            Pane::Terminal => self.render_terminal(cx),
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
                    .h(px(32.0))
                    .flex_none()
                    .overflow_hidden()
                    .px_2()
                    .gap_0p5()
                    .items_center()
                    .child(self.tab("tab-files", "Files", Pane::Files, cx))
                    .child(self.tab("tab-terminal", "Terminal", Pane::Terminal, cx))
                    .when(self.pane == Pane::Files, |row| {
                        row.child(div().flex_1()).child(
                            h_flex()
                                .id("show-hidden-files")
                                .gap(px(5.0))
                                .items_center()
                                .cursor_pointer()
                                .opacity(0.55)
                                .hover(|control| control.opacity(0.82))
                                .text_size(px(11.0))
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
                                        .w(px(22.0))
                                        .h(px(12.0))
                                        .rounded_full()
                                        .bg(if show_hidden {
                                            rgba(0x448aff80)
                                        } else {
                                            rgba(0xffffff20)
                                        })
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(2.0))
                                                .size(px(8.0))
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
                                                        thumb.left(px(from + (to - from) * t))
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

fn read_viewer(path: PathBuf) -> Viewer {
    if crate::document_preview::supports(&path) && path.is_file() {
        return Viewer::Document { path };
    }
    match std::fs::metadata(&path) {
        Ok(m) if m.len() > MAX_FILE_BYTES => Viewer::Note {
            path,
            message: format!("{} KB: too large to show", m.len() / 1024),
        },
        Ok(_) => match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    let lines: Vec<_> = text
                        .lines()
                        .take(MAX_FILE_LINES)
                        .map(|line| line.replace('\t', "    "))
                        .collect();
                    let widest = lines
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, line)| line.chars().count())
                        .map(|(ix, _)| ix)
                        .unwrap_or(0);
                    let markdown = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                        })
                        .then(|| SharedString::from(text));
                    Viewer::Text {
                        markdown,
                        path,
                        lines: std::sync::Arc::new(lines),
                        widest,
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

#[cfg(test)]
mod tests {
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
            assert!(panel.file_scroll.0.borrow().base_handle.max_offset().y > px(1000.0));
            assert_eq!(panel.snapshot(true).selected, Some(path.clone()));
        });
        assert_eq!(std::fs::read_to_string(path).unwrap(), source);
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
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = RightPanel::new(dir.path().into(), window, cx);
            panel.viewer = Viewer::Text {
                path: dir.path().join("long.txt"),
                lines: std::sync::Arc::new(vec!["long line ".repeat(100); 1000]),
                widest: 0,
                markdown: None,
            };
            panel
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            let state = panel.file_scroll.0.borrow();
            let max = state.base_handle.max_offset();
            assert!(
                max.x > px(100.0),
                "long lines must scroll horizontally: {max:?}"
            );
            assert!(
                max.y > px(1000.0),
                "long files must scroll vertically: {max:?}"
            );
        });
        panel.update(cx, |panel, cx| {
            panel
                .file_scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(-120.0), px(-400.0)));
            cx.notify();
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.file_scroll.0.borrow().base_handle.offset(),
                point(px(-120.0), px(-400.0))
            );
        });
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
