//! Harp workbench chrome around Del's page: back/forward navigation in the
//! title bar, the bottom terminal panel (⌘J) and the status bar.
use crate::animated_icons::Glyph;
use crate::appearance::{ActionTone, rgba, shell_surface};
use crate::keyboard_control::KeyboardControl as _;
use crate::micro_motion::MotionIconExt as _;
use crate::state::{AppState, Panel, Selection, Tab};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use crate::{AppShell, ResizePreview};
use del_core::typography::{TEXT_SM, TEXT_XS};
use del_mcp::SessionMode;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const PANEL_DEFAULT_HEIGHT: f32 = 200.0;
const PANEL_MIN_HEIGHT: f32 = 96.0;
const PANEL_HEADER_HEIGHT: f32 = 30.0;
const STATUS_POPOVER_WIDTH: f32 = 340.0;
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// Title-bar control size, matching the sidebar's header buttons.
const NAV_BUTTON: f32 = 28.0;
/// Narrowest navigation zone: toggle, back and forward with their gaps.
const NAV_MIN_WIDTH: f32 = NAV_BUTTON * 3.0 + 3.0 * 2.0 + 12.0 + 8.0;

pub(super) fn navigation_width(sidebar_extent: f32, scale: f32, reserved: f32) -> f32 {
    // macOS traffic-light space is physical pixels, while the sidebar zooms.
    // Never reserve more than the minimum sidebar can actually provide.
    let minimum = (crate::LEFT_SIDEBAR_MIN * scale - reserved).min(NAV_MIN_WIDTH * scale);
    (sidebar_extent * scale - reserved).max(minimum)
}

fn navigation_button_width(width: Pixels) -> Pixels {
    ((width - zpx(8.0 + 3.0 * 2.0)) / 3.0).min(zpx(NAV_BUTTON))
}

#[cfg(test)]
mod navigation_layout_tests {
    #[test]
    fn first_tab_tracks_the_page_at_minimum_sidebar_width() {
        for scale in [0.7, 1.0, 1.4, 2.0] {
            for reserved in [80.0, 12.0] {
                for width in [crate::LEFT_SIDEBAR_MIN, 264.0, crate::LEFT_SIDEBAR_MAX] {
                    let tab_start = reserved + super::navigation_width(width, scale, reserved);
                    assert!(
                        (tab_start - width * scale).abs() < 0.01,
                        "tab {tab_start} must align with page {} at scale {scale}",
                        width * scale
                    );
                }
            }
        }
    }
}

/// The bottom panel: one shell per project, kept alive while hidden.
#[derive(Default)]
pub struct BottomPanel {
    pub open: bool,
    pub maximized: bool,
    /// Height reveal for opening and closing (220 ms, like the side panels).
    motion: crate::panel_motion::Reveal,
    /// Reveal between the normal and maximized heights.
    max_motion: crate::panel_motion::Reveal,
    terminals: HashMap<PathBuf, Entity<TerminalHost>>,
    error: Option<String>,
    /// A root whose shell failed to start; not retried on every frame.
    failed: Option<PathBuf>,
    resize_origin: std::rc::Rc<std::cell::Cell<f32>>,
}

#[derive(Clone)]
pub struct PanelResize {
    start_y: std::rc::Rc<std::cell::Cell<f32>>,
    height: f32,
}

/// A 28 px icon button in the title bar. It stops mouse-down so the bar's
/// window-drag area does not start a window move.
pub fn chrome_button(
    id: impl Into<ElementId>,
    label: &'static str,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .flex_none()
        .size(zpx(NAV_BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(zpx(6.0))
        .text_color(crate::appearance::secondary_text())
        .when(enabled, |b| {
            b.cursor_pointer().hover(|b| b.bg(crate::ui::row_hover()))
        })
        .when(!enabled, |b| b.opacity(0.35))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(enabled, |b| b.keyboard_control())
}

/// A small, flat status-bar item.
fn status_item(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .ghost()
        .h(zpx(STATUS_BAR_HEIGHT - 2.0))
        .px(zpx(6.0))
        .py_0()
        .line_height(zpx(16.0))
        .rounded(zpx(4.0))
        .text_size(zpx(TEXT_XS))
        .text_color(crate::appearance::secondary_text())
}

/// Status popovers state facts, so they read as a heading, label/value rows
/// and an optional note rather than a stack of same-weight lines.
fn status_heading(text: impl Into<SharedString>) -> PopupMenuItem {
    let text = text.into();
    PopupMenuItem::element(move |_, _| {
        div()
            .max_w(zpx(STATUS_POPOVER_WIDTH))
            .truncate()
            .text_size(zpx(TEXT_XS))
            .text_color(crate::appearance::secondary_text())
            .child(text.clone())
    })
    .disabled(true)
}

fn status_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> PopupMenuItem {
    let (label, value) = (label.into(), value.into());
    PopupMenuItem::element(move |_, cx| {
        h_flex()
            .w_full()
            .min_w(zpx(180.))
            .max_w(zpx(STATUS_POPOVER_WIDTH))
            .gap(zpx(16.))
            .justify_between()
            .text_size(zpx(TEXT_SM))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(cx.theme().foreground)
                    .child(label.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(crate::appearance::secondary_text())
                    .child(value.clone()),
            )
    })
    .disabled(true)
}

/// A row with its fine print underneath, kept together as one item.
fn status_row_with_note(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    note: impl Into<SharedString>,
) -> PopupMenuItem {
    let (label, value, note) = (label.into(), value.into(), note.into());
    PopupMenuItem::element(move |_, cx| {
        v_flex()
            .w_full()
            .min_w(zpx(180.))
            .max_w(zpx(STATUS_POPOVER_WIDTH))
            .py(zpx(2.))
            .child(
                h_flex()
                    .w_full()
                    .gap(zpx(16.))
                    .justify_between()
                    .text_size(zpx(TEXT_SM))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().foreground)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(crate::appearance::secondary_text())
                            .child(value.clone()),
                    ),
            )
            .child(
                div()
                    .text_size(zpx(TEXT_XS))
                    .text_color(crate::appearance::secondary_text())
                    .child(note.clone()),
            )
    })
    .disabled(true)
}

fn status_note(text: impl Into<SharedString>) -> PopupMenuItem {
    let text = text.into();
    PopupMenuItem::element(move |_, _| {
        div()
            .max_w(zpx(STATUS_POPOVER_WIDTH))
            .truncate()
            .text_size(zpx(TEXT_XS))
            .text_color(crate::appearance::secondary_text())
            .child(text.clone())
    })
    .disabled(true)
}

fn status_icon(path: &'static str) -> Icon {
    crate::icons::path(path).with_size(zpx(13.0))
}

fn selected_tab(state: &AppState) -> Tab {
    match &state.selection {
        Selection::Draft(key) => Tab::Draft(*key),
        Selection::Session(id) => Tab::Session(id.clone()),
        Selection::Terminal(id) => Tab::Terminal(id.clone()),
    }
}

/// History entries that can still be opened.
fn reachable(state: &AppState) -> impl Fn(&Tab) -> bool + '_ {
    move |tab| match tab {
        Tab::Draft(_) => state.tabs.contains(tab),
        Tab::Session(id) => state.provider_of(id).is_some(),
        Tab::Terminal(id) => state.terminal_rows.iter().any(|row| &row.id == id),
    }
}

/// Use the same measured spring reveal as the transcript disclosures.
fn process_section(
    heading: &'static str,
    open: bool,
    content: impl IntoElement,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    gpui_kit::component::collapsible::Collapsible::new()
        .motion_id((ElementId::from(heading), "process-section-reveal"))
        .open(open)
        .w_full()
        .flex_none()
        .child(
            div()
                .id(heading)
                .keyboard_control()
                .role(gpui::Role::Button)
                .aria_label(heading)
                .aria_expanded(open)
                .debug_selector(move || format!("process-heading-{heading}"))
                .w_full()
                .py(zpx(6.))
                .rounded(zpx(4.))
                .cursor_pointer()
                .hover(|s| s.bg(crate::ui::row_hover()))
                .text_size(zpx(TEXT_XS))
                .text_color(crate::appearance::secondary_text())
                .child(heading)
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_toggle(event, window, cx);
                }),
        )
        .content(content)
}

/// A live menu: an open list follows snapshots and stop completion, rather than
/// keeping the clone captured when the popover first opened.
struct ProcessMenu {
    shell: WeakEntity<AppShell>,
    owner: Option<String>,
    hovered: Option<u32>,
    collapsed: HashSet<&'static str>,
    focus: crate::keyboard_control::FocusHandles,
    _subscription: Subscription,
}
impl ProcessMenu {
    fn new(shell: Entity<AppShell>, owner: Option<String>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&shell, |_, _, cx| cx.notify());
        Self {
            shell: shell.downgrade(),
            owner,
            hovered: None,
            collapsed: HashSet::from([crate::status_processes::SHARED_SECTION]),
            focus: Default::default(),
            _subscription: subscription,
        }
    }
}
impl Render for ProcessMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex()
            .id("process-list")
            .w(zpx(STATUS_POPOVER_WIDTH))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation());
        let Some(shell) = self.shell.upgrade() else {
            return list;
        };
        let snapshot = shell.read(cx).processes.clone();
        let stopping = shell.read(cx).stopping_processes.clone();
        if let Some(error) = &snapshot.error {
            return list.child(error.clone());
        }
        for (heading, rows) in snapshot.sections(self.owner.as_deref()) {
            let open = !self.collapsed.contains(heading);
            let mut body = v_flex().w_full();
            if rows.is_empty() {
                body = body.child(div().text_size(zpx(TEXT_SM)).child("0 running"));
            }
            for process in rows {
                let pid = process.pid;
                let focus = self.focus.control(&pid.to_string(), cx);
                let show = self.hovered == Some(pid)
                    || crate::keyboard_control::keyboard_within(&focus, window, cx);
                let busy = stopping.contains(&process.pid);
                let detail = if process.ports.is_empty() {
                    format!("PID {}", process.pid)
                } else {
                    format!("localhost:{}", process.ports.join(", :"))
                };
                let target = process.clone();
                let shell = self.shell.clone();
                body = body.child(
                    h_flex()
                        .id(("process-row", pid as u64))
                        .debug_selector(move || format!("process-row-{pid}"))
                        .w_full()
                        .h(zpx(30.))
                        .gap(zpx(8.))
                        .rounded(zpx(4.))
                        .hover(|s| s.bg(crate::ui::row_hover()))
                        .on_hover(cx.listener(move |this, entered, _, cx| {
                            if *entered {
                                this.hovered = Some(pid);
                            } else if this.hovered == Some(pid) {
                                this.hovered = None;
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(zpx(TEXT_SM))
                                .child(process.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(zpx(TEXT_XS))
                                .text_color(crate::appearance::secondary_text())
                                .child(if busy { "Stopping…".into() } else { detail }),
                        )
                        .child(
                            div()
                                .id(("stop-process", process.pid as u64))
                                .when(open, |button| button.keyboard_control_with(&focus))
                                .aria_label(format!("Stop {} (PID {})", process.name, process.pid))
                                .debug_selector(move || format!("stop-process-{pid}"))
                                .size(zpx(22.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(zpx(4.))
                                .when(show && !busy, |button| {
                                    button.child(div().motion_child(true, |feedback| {
                                        feedback.glyph(Glyph::Close).with_size(zpx(12.))
                                    }))
                                })
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Stop process")
                                        .build(window, cx)
                                })
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    if open
                                        && show
                                        && !busy
                                        && let Some(shell) = shell.upgrade()
                                    {
                                        shell.update(cx, |shell, cx| {
                                            shell.stop_process(target.clone(), cx)
                                        });
                                    }
                                }),
                        ),
                );
            }
            list = list.child(process_section(
                heading,
                open,
                body,
                cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(heading) {
                        this.collapsed.insert(heading);
                    }
                    this.hovered = None;
                    cx.notify();
                }),
            ));
        }
        list
    }
}

impl AppShell {
    pub(super) fn start_status_ticker(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut tick = 0u64;
            loop {
                if this
                    .update(cx, |this, cx| {
                        if this.state.read(cx).window_active {
                            this.refresh_status(tick.is_multiple_of(5), cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                tick = tick.wrapping_add(1);
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        })
        .detach();
    }

    fn refresh_status(&mut self, refresh_git: bool, cx: &mut Context<Self>) {
        if self.status_refreshing {
            return;
        }
        self.status_refreshing = true;
        if refresh_git {
            self.state.update(cx, |state, cx| {
                state.refresh_git(state.effective().2);
                state.refresh_selected_usage(cx);
            });
        }
        let backend = self.state.read(cx).backend();
        let mut roots: Vec<crate::status_processes::Root> =
            self.terminals
                .iter()
                .map(|(id, host)| crate::status_processes::Root {
                    pid: host.read(cx).snapshot().pid,
                    owner: Some(format!("terminal:{}", id.as_str())),
                })
                .chain(self.mode_terminals.iter().map(|(id, terminal)| {
                    crate::status_processes::Root {
                        pid: terminal.host.read(cx).snapshot().pid,
                        owner: Some(format!("chat:{}", id.as_str())),
                    }
                }))
                .collect();
        roots.extend(
            self.bottom
                .terminals
                .values()
                .map(|host| crate::status_processes::Root {
                    pid: host.read(cx).snapshot().pid,
                    owner: None,
                }),
        );
        let mut side_owners = HashMap::new();
        for (panel, owner) in std::iter::once((&self.right, &self.right_key)).chain(
            self.right_cache
                .values()
                .map(|entry| (&entry.panel, &entry.key)),
        ) {
            for id in panel.read(cx).process_sessions(cx) {
                side_owners.insert(id, owner.clone());
            }
            roots.extend(panel.read(cx).process_roots(cx).into_iter().map(|pid| {
                crate::status_processes::Root {
                    pid,
                    owner: owner.clone(),
                }
            }));
        }
        let previous = self.processes.clone();
        let job = backend.runtime.spawn(async move {
            let mut tags = std::collections::BTreeMap::new();
            // Include reconnectable provider servers that may outlive their original parent.
            for id in backend.manager.live_session_ids() {
                let parent = backend
                    .manager
                    .peer_parent(&id)
                    .unwrap_or_else(|| id.clone());
                let owner = side_owners
                    .get(&id)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| format!("chat:{}", parent.as_str()));
                if let Some(tag) = backend.manager.process_tag(&id) {
                    tags.insert(tag, owner.clone());
                }
                if let Ok(Some((pid, shared))) = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    backend.manager.session_process(&id),
                )
                .await
                {
                    roots.push(crate::status_processes::Root {
                        pid,
                        owner: (!shared).then_some(owner),
                    });
                }
            }
            crate::status_processes::read(roots, tags, previous).await
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.status_refreshing = false;
                if let Ok(snapshot) = result {
                    this.processes = snapshot;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_process(&mut self, process: crate::status_processes::Process, cx: &mut Context<Self>) {
        if !self.stopping_processes.insert(process.pid) {
            return;
        }
        let job = self
            .state
            .read(cx)
            .backend()
            .runtime
            .spawn(crate::status_processes::stop(process.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.stopping_processes.remove(&process.pid);
                match result {
                    Ok(Ok(())) => this
                        .processes
                        .rows
                        .retain(|p| p.pid != process.pid || p.identity != process.identity),
                    other => this.state.update(cx, |state, _| {
                        state.report_error(match other {
                            Ok(Err(error)) => error,
                            Err(error) => format!("Could not stop process: {error}"),
                            _ => unreachable!(),
                        })
                    }),
                }
                this.refresh_status(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Record the shown tab in the back/forward history. A newly shown
    /// project also gets its Git state refreshed for the status bar.
    pub(super) fn track_navigation(&mut self, cx: &mut Context<Self>) {
        let tab = selected_tab(self.state.read(cx));
        self.nav.visit(tab);
        let cwd = self.state.read(cx).effective().2;
        if self.git_refreshed.as_ref() != Some(&cwd) {
            self.git_refreshed = Some(cwd.clone());
            self.state.update(cx, |state, _| state.refresh_git(cwd));
        }
    }

    pub(super) fn navigate(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let state = self.state.read(cx);
            self.nav.step(delta, reachable(state))
        };
        if let Some(tab) = target {
            self.state.update(cx, |state, cx| match &tab {
                Tab::Session(id) if !state.tabs.contains(&tab) => state.select(id.clone(), cx),
                Tab::Terminal(id) if !state.tabs.contains(&tab) => {
                    state.select_terminal(id.clone(), cx)
                }
                _ => state.select_tab(&tab, cx),
            });
            self.focus_composer(window, cx);
        }
    }

    /// Sidebar toggle, then back and forward right-aligned to the
    /// sidebar's edge. With the sidebar hidden the zone shrinks to its
    /// buttons and the tabs follow it.
    pub(super) fn render_nav(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let back = self.nav.can_step(-1, reachable(state));
        let forward = self.nav.can_step(1, reachable(state));
        let settings = state.panel == Panel::Providers;
        let button_width = navigation_button_width(width);
        h_flex()
            .flex_none()
            .w(width)
            .h_full()
            .items_center()
            .gap(zpx(2.0))
            .pr(zpx(8.0))
            .child(
                chrome_button("toggle-sidebar", "Toggle left sidebar", true)
                    .w(button_width)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Toggle left sidebar")
                            .action(&crate::ToggleSidebar, None)
                            .build(window, cx)
                    })
                    .when(settings, |button| button.invisible())
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar(!this.sidebar_open, cx)),
                    )
                    .motion_child(true, |feedback| feedback.glyph(Glyph::PanelLeft)),
            )
            .child(div().flex_1().min_w_0())
            .child(
                chrome_button("nav-back", "Back", back)
                    .w(button_width)
                    // Disabled at the start of history; the tooltip says why.
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if back {
                            "Back"
                        } else {
                            "Back · no earlier session"
                        })
                        .action(&crate::NavigateBack, None)
                        .build(window, cx)
                    })
                    .when(back, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| this.navigate(-1, window, cx)))
                    })
                    .child(crate::icons::icon(IconName::ChevronLeft)),
            )
            .child(
                chrome_button("nav-forward", "Forward", forward)
                    .w(button_width)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if forward {
                            "Forward"
                        } else {
                            "Forward · no later session"
                        })
                        .action(&crate::NavigateForward, None)
                        .build(window, cx)
                    })
                    .when(forward, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| this.navigate(1, window, cx)))
                    })
                    .child(crate::icons::icon(IconName::ChevronRight)),
            )
    }

    fn panel_root(&self, cx: &App) -> PathBuf {
        let state = self.state.read(cx);
        state.project_root(&state.effective().2)
    }

    pub(super) fn toggle_bottom_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).panel != Panel::Chat {
            return;
        }
        self.bottom.open = !self.bottom.open;
        let now = std::time::Instant::now();
        self.bottom
            .motion
            .set(self.bottom.open, cx.reduce_motion(), now);
        if self.bottom.open {
            self.ensure_panel_terminal(window, cx);
            let root = self.panel_root(cx);
            if let Some(terminal) = self.bottom.terminals.get(&root) {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            }
        } else {
            self.bottom.maximized = false;
            self.bottom.max_motion.set(false, cx.reduce_motion(), now);
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// The panel's reveal and maximize fractions, and whether either moves.
    pub(super) fn bottom_panel_progress(&self) -> (f32, f32, bool) {
        let now = std::time::Instant::now();
        let (open, opening) = self.bottom.motion.sample(now);
        let (max, maximizing) = self.bottom.max_motion.sample(now);
        (open, max, opening || maximizing)
    }

    /// The panel's shell for the selected project, started on first use.
    pub(super) fn ensure_panel_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.panel_root(cx);
        if self.bottom.terminals.contains_key(&root) || self.bottom.failed.as_ref() == Some(&root) {
            return;
        }
        match TerminalHost::spawn(TerminalLaunchSpec::shell(root.clone()), window, cx) {
            Ok(terminal) => {
                cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
                self.bottom.terminals.insert(root, terminal);
                self.bottom.error = None;
                self.bottom.failed = None;
            }
            Err(error) => {
                self.bottom.error = Some(format!("Could not open terminal: {error}"));
                self.bottom.failed = Some(root);
            }
        }
    }

    pub(super) fn resize_bottom_panel(
        &mut self,
        drag: &PanelResize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let delta =
            del_core::zoom::unscaled(drag.start_y.get() - window.mouse_position().y.as_f32());
        let viewport = del_core::zoom::unscaled(window.viewport_size().height.as_f32());
        self.sidebar_widths.bottom =
            (drag.height + delta).clamp(PANEL_MIN_HEIGHT, (viewport - 160.0).max(PANEL_MIN_HEIGHT));
        self.bottom.maximized = false;
        self.bottom.max_motion.settle(false);
        self.save_sidebar_widths_soon(cx);
        cx.notify();
    }

    /// The bottom terminal panel, framed like the page beneath it.
    /// The panel grows from the bottom edge with its contents pinned at the
    /// target height, so the shell is laid out (and resized) once rather than
    /// on every frame of the reveal.
    pub(super) fn render_bottom_panel(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (progress, max_progress, animating) = self.bottom_panel_progress();
        if progress <= 0. || self.state.read(cx).panel != Panel::Chat {
            return None;
        }
        if animating {
            window.request_animation_frame();
        }
        let root = self.panel_root(cx);
        let project = self.state.read(cx).project_name(&root);
        let height = self.sidebar_widths.bottom.max(PANEL_MIN_HEIGHT);
        // The column the page and panel share: below the title bar, above the
        // status bar and the page's bottom inset.
        let column = del_core::zoom::unscaled(window.viewport_size().height.as_f32())
            - self.metrics.topbar_height
            - STATUS_BAR_HEIGHT
            - self.metrics.page_inset;
        let normal = height + PANEL_HEADER_HEIGHT;
        let full = normal + (column.max(normal) - normal) * max_progress;
        let settled_max = self.bottom.maximized && max_progress >= 1. && !animating;
        let drag = PanelResize {
            start_y: self.bottom.resize_origin.clone(),
            height,
        };
        let start_y = drag.start_y.clone();
        let shell = cx.entity();
        let maximized = self.bottom.maximized;
        Some(
            div()
                .id("bottom-panel-frame")
                .relative()
                .w_full()
                .min_h_0()
                .when(settled_max, |panel| panel.flex_1())
                .when(!settled_max, |panel| {
                    panel.flex_none().h(zpx(full * progress))
                })
                .child(
                    div().absolute().inset_0().overflow_hidden().child(
                        v_flex()
                            .id("bottom-panel")
                            .relative()
                            .w_full()
                            .when(settled_max, |panel| panel.h_full())
                            .when(!settled_max, |panel| panel.h(zpx(full)))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .h(zpx(PANEL_HEADER_HEIGHT))
                                    .items_center()
                                    .gap(zpx(crate::ui::SPACE_SM))
                                    .text_size(zpx(TEXT_SM))
                                    .child(
                                        div()
                                            .px(zpx(crate::ui::SPACE_SM))
                                            .py(zpx(3.0))
                                            .rounded(zpx(6.0))
                                            .bg(crate::ui::row_selected())
                                            .text_color(crate::ui::primary_text())
                                            .child("Terminal"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family("Menlo")
                                            .text_color(rgba(0xffffff70))
                                            .child(format!("zsh — {project}")),
                                    )
                                    .child(
                                        chrome_button(
                                            "bottom-maximize",
                                            if maximized {
                                                "Restore panel size"
                                            } else {
                                                "Maximize panel"
                                            },
                                            true,
                                        )
                                        .tooltip(move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                if maximized {
                                                    "Restore panel size"
                                                } else {
                                                    "Maximize panel"
                                                },
                                            )
                                            .build(window, cx)
                                        })
                                        .size(zpx(24.0))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.bottom.maximized = !this.bottom.maximized;
                                            this.bottom.max_motion.set(
                                                this.bottom.maximized,
                                                cx.reduce_motion(),
                                                std::time::Instant::now(),
                                            );
                                            cx.notify();
                                        }))
                                        .child(
                                            crate::icons::icon(if maximized {
                                                IconName::Minimize
                                            } else {
                                                IconName::Maximize
                                            }),
                                        ),
                                    )
                                    .child(
                                        chrome_button("bottom-close", "Close panel", true)
                                            .tooltip(|window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    "Close panel",
                                                )
                                                .action(&crate::ToggleTerminalPanel, None)
                                                .build(window, cx)
                                            })
                                            .size(zpx(24.0))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.toggle_bottom_panel(window, cx)
                                            }))
                                            .child(crate::icons::icon(IconName::Close)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .rounded(zpx(self.metrics.page_radius))
                                    .overflow_hidden()
                                    .bg(crate::appearance::page_surface())
                                    .when_some(self.bottom.error.clone(), |body, error| {
                                        body.child(
                                            div()
                                                .p(zpx(crate::ui::SPACE_SM))
                                                .text_size(zpx(TEXT_SM))
                                                .child(error),
                                        )
                                    })
                                    .when_some(
                                        self.bottom.terminals.get(&root).cloned(),
                                        |body, terminal| {
                                            let view = terminal.read(cx).terminal_view();
                                            crate::terminal_host::apply_shell_theme(&view, cx);
                                            body.child(terminal)
                                        },
                                    ),
                            ),
                    ),
                )
                // Outside the clip, so the sash keeps its full 6 px target.
                .child(
                    div()
                        .id("resize-bottom-panel")
                        .absolute()
                        .top(zpx(-3.0))
                        .left_0()
                        .right_0()
                        .h(zpx(6.0))
                        .cursor_row_resize()
                        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                            start_y.set(event.position.y.as_f32());
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                            if event.click_count() == 2 {
                                this.sidebar_widths.bottom = PANEL_DEFAULT_HEIGHT;
                                this.bottom.maximized = false;
                                this.bottom.max_motion.settle(false);
                                this.save_sidebar_widths_soon(cx);
                                cx.notify();
                            }
                        }))
                        .on_drag(drag, move |drag, _, window, cx| {
                            shell.update(cx, |this, cx| this.resize_bottom_panel(drag, window, cx));
                            cx.new(|_| ResizePreview)
                        }),
                )
                .into_any_element(),
        )
    }

    /// Compact session metrics and the Chat / Terminal switch.
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let (provider, _, cwd) = state.effective();
        let git = state.project_info.get(&cwd).cloned().unwrap_or_default();
        let account = state.selected_account();
        let limits = state
            .account_usage
            .get(&(provider, account.clone()))
            .and_then(Option::as_ref)
            .cloned();
        let usage = self
            .open_chat(cx)
            .map(|chat| chat.read(cx).usage.clone())
            .or_else(|| match &state.selection {
                Selection::Session(id) => self.mode_terminals.get(id).map(|t| t.usage.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let usage = state.conversation_usage(&usage);
        let cost_breakdown = usage.cost_breakdown();
        let cost_rows = usage.provider_costs();
        let total_cost = usage.cost_summary();
        let terminal = state.mode() == SessionMode::Terminal
            || matches!(&state.selection, Selection::Session(id) if self.mode_terminals.contains_key(id));
        let context_label = if terminal {
            "Context —".into()
        } else {
            usage.context_label()
        };
        let cost_label = format!("Cost {}", crate::session_usage::money(total_cost));
        let remaining = limits.as_ref().and_then(|limits| limits.remaining());
        let limits_label = remaining
            .map(|n| format!("Limits {n:.0}% left"))
            .unwrap_or_else(|| "Limits —".into());
        let process_owner = match &state.selection {
            Selection::Session(id) => Some(format!("chat:{}", id.as_str())),
            Selection::Terminal(id) => Some(format!("terminal:{}", id.as_str())),
            Selection::Draft(_) => None,
        };
        let process_count = self.processes.commands(process_owner.as_deref()).count();
        // Like Context, Limits and Cost: shown only once known.
        let processes_known = self.processes.ready && self.processes.error.is_none();
        let process_label = format!("{process_count} running");
        let process_shell = cx.entity().downgrade();
        // Not read yet is unknown, so the branch stays hidden; only a finished
        // read can say there is no repository.
        let branch_known = state.project_info.contains_key(&cwd);
        let branch_name: String = if git.current.is_empty() {
            "No repository".into()
        } else {
            git.current.clone()
        };
        // Kept apart from the name, so a long branch truncates without hiding these.
        let branch_meta = if git.current.is_empty() {
            String::new()
        } else {
            [
                match git.changed_files {
                    Some(0) => String::new(),
                    Some(n) => format!("{n} {}", if n == 1 { "file" } else { "files" }),
                    None => "Changes unavailable".into(),
                },
                if git.ahead > 0 {
                    format!("↑{}", git.ahead)
                } else {
                    String::new()
                },
                if git.behind > 0 {
                    format!("↓{}", git.behind)
                } else {
                    String::new()
                },
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
        };
        let branch = if branch_meta.is_empty() {
            branch_name.clone()
        } else {
            format!("{branch_name} · {branch_meta}")
        };
        let branch_tooltip = format!(
            "{}\n{branch}{}",
            cwd.display(),
            git.worktree
                .as_ref()
                .map(|name| format!("\nWorktree: {name}"))
                .unwrap_or_default()
        );
        let account_name = match account.as_deref() {
            Some(id) => state
                .accounts
                .iter()
                .find(|a| a.id == id && a.provider == provider)
                .map(|a| a.identity.clone().unwrap_or_else(|| a.display())),
            None => state.default_identities.get(&provider).cloned(),
        }
        .unwrap_or_else(|| "Account identity unavailable".into());
        h_flex()
            .id("status-bar")
            .flex_none()
            .w_full()
            .h(zpx(STATUS_BAR_HEIGHT))
            .px(zpx(8.))
            .gap(zpx(2.))
            .items_center()
            // Horizontal only, so a focused item's ring is not cut by the bar's height.
            .overflow_x_hidden()
            .bg(shell_surface())
            .text_size(zpx(TEXT_XS))
            .text_color(crate::appearance::secondary_text())
            .child(
                status_item("status-folder")
                    .min_w_0()
                    .max_w(zpx(160.))
                    .accessibility_label(format!("Project folder: {}", crate::sessions::folder_name(&state.project_root(&cwd))))
                    .tooltip(cwd.display().to_string())
                    .child(h_flex().min_w_0().gap(zpx(6.)).items_center()
                        .child(crate::icons::icon(IconName::Folder).with_size(zpx(crate::icons::MARK_SIZE)))
                        .child(div().min_w_0().truncate().child(crate::sessions::folder_name(&state.project_root(&cwd)))))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenFiles), cx)),
            )
            .when(branch_known, |row| row.child(
                status_item("status-branch")
                    .min_w_0()
                    .max_w(zpx(390.))
                    .accessibility_label(format!("Git: {branch}"))
                    .tooltip(branch_tooltip)
                    .child(
                        // The clip lives here, not on the button, so its focus ring stays whole.
                        h_flex()
                            .min_w_0()
                            .overflow_x_hidden()
                            .gap(zpx(6.))
                            .items_center()
                            .text_size(zpx(TEXT_XS))
                            .line_height(zpx(16.))
                            .child(
                                h_flex()
                                    .w(zpx(14.))
                                    .h(zpx(16.))
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .child(status_icon("icons/git-branch.svg")),
                            )
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap(zpx(6.))
                                    .items_baseline()
                                    .child(div().min_w_0().truncate().child(branch_name))
                                    .when(!branch_meta.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .flex_none()
                                                .whitespace_nowrap()
                                                .child(branch_meta),
                                        )
                                    })
                                    .when_some(
                                        // A clean tree has nothing to report.
                                        git.changed_lines.filter(|lines| *lines != (0, 0)),
                                        |row, (added, removed)| {
                                            // A zero side has nothing to report either.
                                            row.when(added > 0, |row| {
                                                row.child(
                                                    div()
                                                        .flex_none()
                                                        .whitespace_nowrap()
                                                        .text_color(
                                                            ActionTone::Success.foreground(),
                                                        )
                                                        .child(format!("+{added}")),
                                                )
                                            })
                                            .when(removed > 0, |row| {
                                                row.child(
                                                    div()
                                                        .flex_none()
                                                        .whitespace_nowrap()
                                                        .text_color(ActionTone::Danger.foreground())
                                                        .child(format!("−{removed}")),
                                                )
                                            })
                                        },
                                    ),
                            ),
                    )
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::OpenChanges), cx)
                    }),
            ))
            // The right-hand metrics keep their places as items load in.
            .child(div().flex_1().min_w(zpx(4.)))
            .when(!terminal && usage.context_tokens.is_some() && usage.context_window.is_some_and(|n| n > 0), |row| row.child(
                status_item("status-context")
                    .accessibility_label(context_label.clone())
                    .tooltip(
                        "Conversation context currently used (history, instructions and tools)",
                    )
                    .child(div().text_size(zpx(TEXT_XS)).child(context_label))
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let menu = menu.item(status_heading("Context usage"));
                        if terminal {
                            return menu.item(status_note("Not reported in Terminal mode"));
                        }
                        match (usage.context_tokens, usage.context_window) {
                            (Some(used), Some(limit)) => {
                                let percent = used as f64 / limit as f64 * 100.;
                                menu.item(status_row("Used", format!("{percent:.0}%")))
                                    .item(status_row(
                                        "Remaining",
                                        format!("{:.0}%", 100. - percent.min(100.)),
                                    ))
                                    .item(status_row(
                                        "Tokens",
                                        format!(
                                            "{} / {}",
                                            crate::ui::group_digits(used),
                                            crate::ui::group_digits(limit)
                                        ),
                                    ))
                                    .separator()
                                    .item(status_note(
                                        "Includes the conversation carried by the active model",
                                    ))
                                    .item(status_note(
                                        "Older history may be compacted; this is not lifetime tokens",
                                    ))
                            }
                            (Some(used), None) => menu
                                .item(status_row("Tokens", crate::ui::group_digits(used)))
                                .item(status_note(
                                    "This provider has not reported a context capacity",
                                )),
                            _ => menu.item(status_note(
                                "No context usage reported for this session yet",
                            )),
                        }
                    }),
            ))
            .when(remaining.is_some(), |row| row.child(
                status_item("status-limits")
                    .accessibility_label(limits_label.clone())
                    .tooltip(format!(
                        "{} · {account_name}",
                        crate::providers::display_name(provider)
                    ))
                    .when(remaining.is_some_and(|n| n <= 10.), |b| {
                        b.text_color(ActionTone::Warning.foreground())
                    })
                    .child(div().text_size(zpx(TEXT_XS)).child(limits_label))
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let mut menu = menu.item(status_heading(format!(
                            "{} · {account_name}",
                            crate::providers::display_name(provider)
                        )));
                        if let Some(limits) = &limits {
                            for window in &limits.windows {
                                let left = format!("{:.0}% left", window.remaining);
                                menu = menu.item(match &window.resets_at {
                                    Some(reset) => status_row_with_note(
                                        window.label.clone(),
                                        left,
                                        format!("Resets {reset}"),
                                    ),
                                    None => status_row(window.label.clone(), left),
                                });
                            }
                        } else {
                            menu = menu.item(status_note(
                                "No allowance reported by this provider/account",
                            ));
                        }
                        menu
                    }),
            ))
            .when(total_cost.is_some(), |row| row.child(
                status_item("status-cost")
                    .accessibility_label(cost_label.clone())
                    .tooltip(cost_breakdown)
                    .child(div().text_size(zpx(TEXT_XS)).child(cost_label))
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let mut menu = menu.item(status_heading("Main + side chats · all provider switches"));
                        for (provider, cost) in &cost_rows {
                            let name = match provider.as_str() {
                                "claude" => "Claude",
                                "codex" => "Codex",
                                "opencode" => "OpenCode",
                                other => other,
                            };
                            menu = menu.item(status_row(
                                name.to_string(),
                                crate::session_usage::money(Some(*cost)),
                            ));
                        }
                        menu.separator()
                            .item(status_row(
                                "Total",
                                crate::session_usage::money(total_cost),
                            ))
                            .item(status_note(
                                "Estimates may exclude unreported or Terminal usage",
                            ))
                    }),
            ))
            .when(processes_known, |row| row.child(
                status_item("status-processes")
                    .accessibility_label(process_label.clone())
                    .tooltip(if process_owner.is_some() {
                        "Commands and servers · this session"
                    } else {
                        "Commands and servers · all sessions"
                    })
                    .child(div().text_size(zpx(TEXT_XS)).child(process_label))
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, cx| {
                        let Some(shell) = process_shell.upgrade() else { return menu; };
                        let owner = process_owner.clone();
                        let list = cx.new(|cx| ProcessMenu::new(shell, owner, cx));
                        menu.scrollable(true).max_h(zpx(320.)).item(
                            PopupMenuItem::element(move |_, _| list.clone()).custom_highlight()
                        )
                    }),
            ))
            .child(
                status_item("status-mode")
                    .accessibility_label(if terminal {
                        "Switch to Chat"
                    } else {
                        "Switch to Terminal"
                    })
                    .tooltip(if terminal {
                        "Switch to Chat"
                    } else {
                        "Switch to Terminal"
                    })
                    .child(div().text_size(zpx(TEXT_XS)).child(if terminal { "Terminal" } else { "Chat" }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_mode(
                            if terminal {
                                SessionMode::Chat
                            } else {
                                SessionMode::Terminal
                            },
                            window,
                            cx,
                        );
                    })),
            )
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::component::menu::PopupMenuItem;
    use std::{cell::Cell, rc::Rc};
    struct Footer {
        opened: Rc<Cell<bool>>,
    }
    impl Render for Footer {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            div().size_full().flex().flex_col().justify_end().child(
                h_flex().h(zpx(STATUS_BAR_HEIGHT)).overflow_hidden().child(
                    status_item("status-test")
                        .child(
                            div()
                                .debug_selector(|| "status-trigger".into())
                                .child("Context —"),
                        )
                        .dropdown_menu_with_anchor(Anchor::BottomRight, |menu, _, _| {
                            menu.label("Context usage")
                                .item(PopupMenuItem::element(|_, _| {
                                    div()
                                        .debug_selector(|| "status-menu-detail".into())
                                        .child("No usage reported yet")
                                }))
                        })
                        .on_open_change(move |open, _, _| opened.set(*open)),
                ),
            )
        }
    }
    #[::gpui::test]
    fn process_heading_click_reveals_and_hides_the_rows(cx: &mut ::gpui::TestAppContext) {
        struct Fixture(bool);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                v_flex()
                    .child(process_section(
                        "Shared",
                        self.0,
                        div().h(px(40.)).child("concord-mcp"),
                        cx.listener(|this, _, _, cx| {
                            this.0 = !this.0;
                            cx.notify();
                        }),
                    ))
                    .child(div().debug_selector(|| "after-section".into()).h(px(1.)))
            }
        }
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(false);
        });
        let (_, cx) = cx.add_window_view(|_, _| Fixture(false));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let header = cx.debug_bounds("process-heading-Shared").unwrap();
        let closed = cx.debug_bounds("after-section").unwrap().top();
        assert_eq!(closed, header.bottom());
        let click = |cx: &mut ::gpui::VisualTestContext| {
            cx.simulate_mouse_down(header.center(), MouseButton::Left, Default::default());
            cx.simulate_mouse_up(header.center(), MouseButton::Left, Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        click(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(32));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let partial = cx.debug_bounds("after-section").unwrap().top();
        assert!(partial > closed && partial < closed + px(40.));
        cx.update(|_, cx| cx.set_reduce_motion(true));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            cx.debug_bounds("after-section").unwrap().top(),
            closed + px(40.)
        );
        click(cx);
        assert_eq!(cx.debug_bounds("after-section").unwrap().top(), closed);
    }

    #[::gpui::test]
    fn status_menu_opens_on_pointer_press(cx: &mut ::gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let opened = Rc::new(Cell::new(false));
        let (_, cx) = cx.add_window_view(|_, _| Footer {
            opened: opened.clone(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let trigger = cx.debug_bounds("status-trigger").unwrap();
        cx.simulate_mouse_down(trigger.center(), MouseButton::Left, Default::default());
        cx.run_until_parked();
        assert!(opened.get(), "footer menu must receive the pointer press");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let detail = cx.debug_bounds("status-menu-detail").unwrap();
        assert!(
            detail.bottom() <= trigger.top(),
            "details should open above the footer"
        );
    }
}
