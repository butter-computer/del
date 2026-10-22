//! The right sidebar: a read-only file viewer and a command runner for the
//! open session's project folder.
//!
//! The "terminal" runs one command at a time through the user's shell and shows
//! its output. It is not a PTY: no interactive programs, no colours, no stdin.

use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const PANEL_WIDTH: f32 = 340.0;
const MONO: &str = "Menlo";
/// Largest file shown, and most lines rendered.
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FILE_LINES: usize = 3000;
/// Terminal scrollback kept, in lines.
const MAX_TERM_LINES: usize = 2000;
/// Directories never listed.
const HIDDEN: &[&str] = &[".git", "target", "node_modules", ".DS_Store"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Files,
    Terminal,
}

enum Viewer {
    None,
    Text { path: PathBuf, lines: Vec<String> },
    Note { path: PathBuf, message: String },
}

pub struct RightPanel {
    pane: Pane,
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    viewer: Viewer,
    term: Vec<String>,
    term_input: Entity<InputState>,
    running: bool,
}

impl RightPanel {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let term_input = cx.new(|cx| InputState::new(window, cx).placeholder("Run a command…"));
        cx.subscribe_in(&term_input, window, |this, input, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let command = input.read(cx).value().to_string();
                input.update(cx, |i, cx| i.set_value("", window, cx));
                this.run(command, cx);
            }
        })
        .detach();
        Self {
            pane: Pane::Files,
            root,
            expanded: HashSet::new(),
            viewer: Viewer::None,
            term: Vec::new(),
            term_input,
            running: false,
        }
    }

    /// Point the panel at another project folder.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
            self.expanded.clear();
            self.viewer = Viewer::None;
            cx.notify();
        }
    }

    pub fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = Pane::Terminal;
        self.term_input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.viewer = match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_FILE_BYTES => Viewer::Note {
                path,
                message: format!("{} KB: too large to show", m.len() / 1024),
            },
            Ok(_) => match std::fs::read(&path) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Viewer::Text {
                        lines: text
                            .lines()
                            .take(MAX_FILE_LINES)
                            .map(|l| l.replace('\t', "    "))
                            .collect(),
                        path,
                    },
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
        };
        cx.notify();
    }

    fn push_term(&mut self, lines: impl IntoIterator<Item = String>) {
        self.term.extend(lines);
        if self.term.len() > MAX_TERM_LINES {
            let extra = self.term.len() - MAX_TERM_LINES;
            self.term.drain(..extra);
        }
    }

    fn run(&mut self, command: String, cx: &mut Context<Self>) {
        let command = command.trim().to_string();
        if command.is_empty() || self.running {
            return;
        }
        self.push_term([format!("$ {command}")]);
        self.running = true;
        let root = self.root.clone();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let task = cx.background_executor().spawn(async move {
            std::process::Command::new(shell)
                .arg("-lc")
                .arg(&command)
                .current_dir(root)
                .stdin(Stdio::null())
                .output()
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(out) => {
                        let text = format!(
                            "{}{}",
                            String::from_utf8_lossy(&out.stdout),
                            String::from_utf8_lossy(&out.stderr)
                        );
                        this.push_term(text.lines().map(str::to_owned));
                        if !out.status.success() {
                            this.push_term([format!("exit {}", out.status.code().unwrap_or(-1))]);
                        }
                    }
                    Err(e) => this.push_term([format!("could not run: {e}")]),
                }
                this.running = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Visible rows of the tree under `dir`, depth first.
    fn tree_rows(&self, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, usize, bool)>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<(PathBuf, bool)> = read
            .filter_map(Result::ok)
            .filter(|e| !HIDDEN.contains(&e.file_name().to_string_lossy().as_ref()))
            .map(|e| {
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                (e.path(), is_dir)
            })
            .collect();
        entries.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
        });
        for (path, is_dir) in entries {
            out.push((path.clone(), depth, is_dir));
            if is_dir && self.expanded.contains(&path) {
                self.tree_rows(&path, depth + 1, out);
            }
        }
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
            .on_click(cx.listener(move |this, _, _, cx| {
                this.pane = pane;
                cx.notify();
            }))
            .child(label)
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = Vec::new();
        self.tree_rows(&self.root, 0, &mut rows);
        let selected = match &self.viewer {
            Viewer::Text { path, .. } | Viewer::Note { path, .. } => Some(path.clone()),
            Viewer::None => None,
        };
        let muted = rgba(0xffffff8c);

        let tree = v_flex()
            .id("file-tree")
            .flex_none()
            .max_h(px(220.0))
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
                    .id(SharedString::from(path.display().to_string()))
                    .h(px(22.0))
                    .pl(px(8.0 + depth as f32 * 12.0))
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
                    .when(is_selected, |r| r.bg(rgba(0xffffff14)))
                    .when(!is_selected, |r| r.hover(|r| r.bg(rgba(0xffffff0a))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            if !this.expanded.remove(&click) {
                                this.expanded.insert(click.clone());
                            }
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
                    .child(div().min_w_0().truncate().child(name))
            }));

        let viewer = match &self.viewer {
            Viewer::None => div()
                .p_3()
                .text_size(px(TEXT_SM))
                .text_color(muted)
                .child("Select a file to view it")
                .into_any_element(),
            Viewer::Note { message, .. } => div()
                .p_3()
                .text_size(px(TEXT_SM))
                .text_color(muted)
                .child(message.clone())
                .into_any_element(),
            Viewer::Text { lines, .. } => v_flex()
                .id("file-text")
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .py_1()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .children(lines.iter().enumerate().map(|(i, line)| {
                    h_flex()
                        .gap_2()
                        .px_2()
                        .child(
                            div()
                                .w(px(32.0))
                                .flex_none()
                                .text_right()
                                .text_color(rgba(0xffffff40))
                                .child((i + 1).to_string()),
                        )
                        .child(div().whitespace_nowrap().child(line.clone()))
                }))
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .child(tree)
            .child(div().h(px(1.0)).mx_2().my_1().bg(rgba(0xffffff14)))
            .child(viewer)
    }

    fn render_terminal(&self) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("term-out")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .p_2()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(rgba(0xffffffcc))
                    .children(self.term.iter().map(|l| {
                        div()
                            .whitespace_nowrap()
                            .when(l.starts_with("$ "), |d| d.text_color(rgb(0xf5f5f5)))
                            .child(if l.is_empty() {
                                " ".to_string()
                            } else {
                                l.clone()
                            })
                    })),
            )
            .child(
                h_flex()
                    .px_2()
                    .pb_2()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_color(rgba(0xffffff8c))
                            .font_family(MONO)
                            .child("$"),
                    )
                    .child(
                        div().flex_1().child(
                            Input::new(&self.term_input)
                                .appearance(false)
                                .bordered(false)
                                .text_size(px(TEXT_SM)),
                        ),
                    ),
            )
    }
}

impl Render for RightPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let body = match self.pane {
            Pane::Files => self.render_files(cx).into_any_element(),
            Pane::Terminal => self.render_terminal().into_any_element(),
        };
        v_flex()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex_none()
            .child(
                h_flex()
                    .h(px(32.0))
                    .px_2()
                    .gap_0p5()
                    .items_center()
                    .child(self.tab("tab-files", "Files", Pane::Files, cx))
                    .child(self.tab("tab-terminal", "Terminal", Pane::Terminal, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_right()
                            .text_size(px(TEXT_SM))
                            .text_color(rgba(0xffffff52))
                            .child(folder),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
