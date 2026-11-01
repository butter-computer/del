//! Harp workbench chrome around Del's page: back/forward navigation in the
//! title bar, the bottom terminal panel (⌘J) and the status bar.
use crate::appearance::{ActionTone, rgb, rgba, shell_surface};
use crate::state::{AppState, Panel, Selection, Tab};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use crate::{AppShell, ResizePreview};
use del_agent::SessionStatus;
use del_core::typography::TEXT_SM;
use del_mcp::SessionMode;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub const PANEL_DEFAULT_HEIGHT: f32 = 200.0;
const PANEL_MIN_HEIGHT: f32 = 96.0;
const PANEL_HEADER_HEIGHT: f32 = 30.0;
pub const STATUS_BAR_HEIGHT: f32 = 22.0;
/// Title-bar control size, matching the sidebar's header buttons.
const NAV_BUTTON: f32 = 28.0;
/// Narrowest navigation zone: toggle, recent, back and forward with their gaps.
pub const NAV_MIN_WIDTH: f32 = NAV_BUTTON * 4.0 + 2.0 * 2.0 + 12.0 + 8.0;

/// The bottom panel: one shell per project, kept alive while hidden.
#[derive(Default)]
pub struct BottomPanel {
    pub open: bool,
    pub maximized: bool,
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
pub fn chrome_button(id: impl Into<ElementId>, label: &'static str, enabled: bool) -> Stateful<Div> {
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
        .text_color(rgba(0xffffff8c))
        .when(enabled, |b| b.cursor_pointer().hover(|b| b.bg(rgba(0xffffff0a))))
        .when(!enabled, |b| b.opacity(0.35))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A small, flat status-bar item.
fn status_item(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .ghost()
        .h(zpx(STATUS_BAR_HEIGHT - 2.0))
        .px(zpx(6.0))
        .rounded(zpx(4.0))
        .text_size(zpx(TEXT_SM))
        .text_color(rgba(0xffffff8c))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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

/// Live, unarchived sessions: (id, terminal, running, waiting).
fn agent_rows(state: &AppState) -> Vec<(del_agent::SessionId, bool, bool, bool)> {
    state
        .unified_rows()
        .into_iter()
        .filter(|row| !row.archived)
        .map(|row| {
            let terminal = state.terminal_rows.iter().any(|t| t.id == row.id);
            let running = row.status == SessionStatus::Running || state.session_working(&row.id);
            let waiting = row.status == SessionStatus::WaitingApproval;
            (row.id, terminal, running, waiting)
        })
        .collect()
}

impl AppShell {
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

    /// Sidebar toggle, then recent, back and forward right-aligned to the
    /// sidebar's edge. With the sidebar hidden the zone shrinks to its
    /// buttons and the tabs follow it.
    pub(super) fn render_nav(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let back = self.nav.can_step(-1, reachable(state));
        let forward = self.nav.can_step(1, reachable(state));
        let settings = state.panel == Panel::Providers;
        let shell = cx.entity();
        h_flex()
            .flex_none()
            .w(width)
            .h_full()
            .items_center()
            .gap(zpx(2.0))
            .pr(zpx(8.0))
            .child(
                chrome_button("toggle-sidebar", "Toggle left sidebar", true)
                    .when(settings, |button| button.invisible())
                    .on_click(cx.listener(|this, _, _, cx| this.set_sidebar(!this.sidebar_open, cx)))
                    .child(crate::icons::icon(if self.sidebar_open {
                        IconName::PanelLeftClose
                    } else {
                        IconName::PanelLeftOpen
                    })),
            )
            .child(div().flex_1().min_w(zpx(12.0)))
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("nav-recent")
                            .ghost()
                            .size(zpx(NAV_BUTTON))
                            .p_0()
                            .rounded(zpx(6.0))
                            .text_color(rgba(0xffffff8c))
                            .tooltip("Recent sessions")
                            .accessibility_label("Recent sessions")
                            .child(crate::icons::path("icons/clock.svg"))
                            .dropdown_menu(move |mut menu, _, cx| {
                                let (state, recent) = {
                                    let this = shell.read(cx);
                                    let state = this.state.clone();
                                    let recent = this.nav.recent(reachable(state.read(cx)));
                                    (state, recent)
                                };
                                if recent.is_empty() {
                                    return menu.item(PopupMenuItem::label("No recent sessions"));
                                }
                                for tab in recent.into_iter().take(12) {
                                    let (title, _) = state.read(cx).tab_info(&tab);
                                    let state = state.clone();
                                    menu = menu.item(PopupMenuItem::new(title).on_click(
                                        move |_, _, cx| {
                                            let tab = tab.clone();
                                            state.update(cx, |state, cx| match &tab {
                                                Tab::Session(id) if !state.tabs.contains(&tab) => {
                                                    state.select(id.clone(), cx)
                                                }
                                                Tab::Terminal(id) if !state.tabs.contains(&tab) => {
                                                    state.select_terminal(id.clone(), cx)
                                                }
                                                _ => state.select_tab(&tab, cx),
                                            });
                                        },
                                    ));
                                }
                                menu.min_w(zpx(220.0))
                            }),
                    ),
            )
            .child(
                chrome_button("nav-back", "Back", back)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Back (⌘[)").build(window, cx))
                    .when(back, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| this.navigate(-1, window, cx)))
                    })
                    .child(crate::icons::icon(IconName::ChevronLeft)),
            )
            .child(
                chrome_button("nav-forward", "Forward", forward)
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Forward (⌘])").build(window, cx))
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
        if self.bottom.open {
            self.ensure_panel_terminal(window, cx);
            let root = self.panel_root(cx);
            if let Some(terminal) = self.bottom.terminals.get(&root) {
                terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            }
        } else {
            self.bottom.maximized = false;
            self.focus_composer(window, cx);
        }
        cx.notify();
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

    pub(super) fn resize_bottom_panel(&mut self, drag: &PanelResize, window: &Window, cx: &mut Context<Self>) {
        let delta =
            del_core::zoom::unscaled(drag.start_y.get() - window.mouse_position().y.as_f32());
        let viewport = del_core::zoom::unscaled(window.viewport_size().height.as_f32());
        self.sidebar_widths.bottom =
            (drag.height + delta).clamp(PANEL_MIN_HEIGHT, (viewport - 160.0).max(PANEL_MIN_HEIGHT));
        self.bottom.maximized = false;
        self.save_sidebar_widths_soon(cx);
        cx.notify();
    }

    /// The bottom terminal panel, framed like the page beneath it.
    pub(super) fn render_bottom_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.bottom.open || self.state.read(cx).panel != Panel::Chat {
            return None;
        }
        let root = self.panel_root(cx);
        let project = self.state.read(cx).project_name(&root);
        let height = self.sidebar_widths.bottom.max(PANEL_MIN_HEIGHT);
        let drag = PanelResize {
            start_y: self.bottom.resize_origin.clone(),
            height,
        };
        let start_y = drag.start_y.clone();
        let shell = cx.entity();
        let maximized = self.bottom.maximized;
        Some(
            v_flex()
                .id("bottom-panel")
                .relative()
                .w_full()
                .min_h_0()
                .when(maximized, |panel| panel.flex_1())
                .when(!maximized, |panel| panel.flex_none().h(zpx(height + PANEL_HEADER_HEIGHT)))
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
                                this.save_sidebar_widths_soon(cx);
                                cx.notify();
                            }
                        }))
                        .on_drag(drag, move |drag, _, window, cx| {
                            shell.update(cx, |this, cx| this.resize_bottom_panel(drag, window, cx));
                            cx.new(|_| ResizePreview)
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .h(zpx(PANEL_HEADER_HEIGHT))
                        .items_center()
                        .gap_2()
                        .text_size(zpx(TEXT_SM))
                        .child(
                            div()
                                .px_2()
                                .py(zpx(3.0))
                                .rounded(zpx(6.0))
                                .bg(rgba(0xffffff14))
                                .text_color(rgb(0xf5f5f5))
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
                                if maximized { "Restore panel size" } else { "Maximize panel" },
                                true,
                            )
                            .size(zpx(24.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bottom.maximized = !this.bottom.maximized;
                                cx.notify();
                            }))
                            .child(crate::icons::icon(if maximized {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })),
                        )
                        .child(
                            chrome_button("bottom-close", "Close panel", true)
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
                        .bg(gpui_kit::rgb(crate::appearance::TERMINAL_BACKGROUND))
                        .when_some(self.bottom.error.clone(), |body, error| {
                            body.child(div().p_2().text_size(zpx(TEXT_SM)).child(error))
                        })
                        .when_some(self.bottom.terminals.get(&root).cloned(), |body, terminal| {
                            body.child(terminal)
                        }),
                )
                .into_any_element(),
        )
    }

    /// Select the next running (or waiting) session after the current one.
    fn jump_to(&mut self, waiting: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (rows, current) = {
            let state = self.state.read(cx);
            let current = match &state.selection {
                Selection::Session(id) | Selection::Terminal(id) => Some(id.clone()),
                Selection::Draft(_) => None,
            };
            (agent_rows(state), current)
        };
        let matches: Vec<_> = rows
            .into_iter()
            .filter(|(_, _, running, wait)| if waiting { *wait } else { *running })
            .collect();
        let start = current
            .and_then(|id| matches.iter().position(|(row, ..)| row == &id))
            .map_or(0, |at| at + 1);
        if let Some((id, terminal, ..)) = matches.get(start % matches.len().max(1)).cloned() {
            self.state.update(cx, |state, cx| {
                if terminal {
                    state.select_terminal(id, cx)
                } else {
                    state.select(id, cx)
                }
            });
            self.focus_composer(window, cx);
        }
    }

    /// The 22 px status bar on the chrome surface.
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let (provider, _, cwd) = state.effective();
        let root = state.project_root(&cwd);
        let project = state.project_name(&root);
        let space = state
            .spaces
            .items
            .iter()
            .find(|space| space.id == state.spaces.active)
            .map(|space| space.name.clone())
            .unwrap_or_default();
        let git = state.project_info.get(&cwd).cloned().unwrap_or_default();
        let rows = agent_rows(state);
        let running = rows.iter().filter(|(_, _, running, _)| *running).count();
        let waiting = rows.iter().filter(|(.., waiting)| *waiting).count();
        let mode = state.mode();
        let draft = state.is_draft();
        let caffeine = state.caffeine_enabled;
        let window_active = state.window_active;
        let queued = self
            .open_chat(cx)
            .map_or(0, |chat| chat.read(cx).queue.items.iter().filter(|item| !item.sending).count());
        let model = self.model_picker.read(cx).summary(cx);
        let state_entity = self.state.clone();
        let wait_color = ActionTone::Warning.foreground();
        h_flex()
            .id("status-bar")
            .flex_none()
            .w_full()
            .h(zpx(STATUS_BAR_HEIGHT))
            .px(zpx(6.0))
            .gap(zpx(2.0))
            .items_center()
            .bg(shell_surface())
            .text_size(zpx(TEXT_SM))
            .text_color(rgba(0xffffff8c))
            .child(
                status_item("status-space")
                    .tooltip("Switch Space")
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(status_icon("icons/layers.svg"))
                            .child(format!("{space} · {project}")),
                    )
                    .dropdown_menu({
                        let state = state_entity.clone();
                        move |menu, _, cx| crate::sidebar::space_switch_menu(menu, &state, cx)
                    }),
            )
            .when(!git.current.is_empty(), |bar| {
                bar.child(
                    status_item("status-branch")
                        .tooltip(format!("Branch: {}", git.current))
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(status_icon("icons/git-branch.svg"))
                                .child(format!(
                                    "{}{}",
                                    git.current,
                                    if git.dirty { "*" } else { "" }
                                ))
                                .when(git.ahead > 0, |item| item.child(format!("↑{}", git.ahead))),
                        )
                        .dropdown_menu(move |menu, _, _| {
                            let menu = menu.item(
                                PopupMenuItem::new("View changes")
                                    .action(Box::new(crate::OpenChanges)),
                            );
                            if draft {
                                menu.item(
                                    PopupMenuItem::new("Choose starting branch…")
                                        .action(Box::new(crate::SelectBranch)),
                                )
                            } else {
                                menu
                            }
                        }),
                )
            })
            .when_some(git.worktree.clone(), |bar, worktree| {
                bar.child(
                    status_item("status-worktree")
                        .tooltip("Show files")
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(crate::icons::icon(IconName::Folder).with_size(zpx(13.0)))
                                .child(format!("worktree: {worktree}")),
                        )
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::OpenFiles), cx)),
                )
            })
            .when(running > 0, |bar| {
                bar.child(
                    status_item("status-running")
                        .tooltip("Jump to the next running session")
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(status_icon("icons/working-ring.svg"))
                                .child(format!("{running} running")),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.jump_to(false, window, cx))),
                )
            })
            .when(waiting > 0, |bar| {
                bar.child(
                    status_item("status-waiting")
                        .tooltip("Jump to the next session waiting for you")
                        .text_color(wait_color)
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(status_icon("icons/waiting-ring.svg"))
                                .child(format!(
                                    "{waiting} need{} you",
                                    if waiting == 1 { "s" } else { "" }
                                )),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.jump_to(true, window, cx))),
                )
            })
            .child(div().flex_1().min_w(zpx(8.0)))
            .child(
                status_item("status-provider")
                    .tooltip_with_action("Select provider / account", &crate::SelectProvider, None)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(crate::composer_ui::provider_logo(provider).size(zpx(13.0)))
                            .child(crate::providers::display_name(provider)),
                    )
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::SelectProvider), cx)
                    }),
            )
            .child(
                status_item("status-model")
                    .tooltip_with_action("Select model and effort", &crate::SelectModel, None)
                    .child(model)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::SelectModel), cx)),
            )
            .child(
                status_item("status-mode")
                    .tooltip(if mode == SessionMode::Chat {
                        "Switch to Terminal"
                    } else {
                        "Switch to Chat"
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(status_icon("icons/queue.svg"))
                            .child(if mode == SessionMode::Chat { "Chat" } else { "Terminal" }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let next = if mode == SessionMode::Chat {
                            SessionMode::Terminal
                        } else {
                            SessionMode::Chat
                        };
                        this.apply_mode(next, window, cx);
                    })),
            )
            .child(
                status_item("status-caffeine")
                    .tooltip(if caffeine { "Allow Mac to sleep" } else { "Keep Mac awake" })
                    .accessibility_label(if caffeine {
                        "Keep Mac awake: on"
                    } else {
                        "Keep Mac awake: off"
                    })
                    .child(crate::sidebar::coffee_icon(caffeine, window_active, cx))
                    .on_click({
                        let state = state_entity.clone();
                        move |_, _, cx| state.update(cx, |s, cx| s.toggle_caffeine(cx))
                    }),
            )
            .when(queued > 0, |bar| {
                bar.child(
                    status_item("status-queue")
                        .tooltip("Queued follow-ups")
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(status_icon("icons/queue.svg"))
                                .child(format!("{queued} queued")),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.focus_composer(window, cx))),
                )
            })
            .child(
                status_item("status-version")
                    .tooltip("Check for updates")
                    .text_color(rgba(0xffffff70))
                    .child(concat!("v", env!("CARGO_PKG_VERSION")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.updater.update(cx, |updater, cx| updater.check(true, cx))
                    })),
            )
    }
}
