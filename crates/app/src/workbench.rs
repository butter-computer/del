//! Harp workbench chrome around Del's page: back/forward navigation in the
//! title bar, the bottom terminal panel (⌘J) and the status bar.
use crate::animated_icons::Glyph;
use crate::appearance::{ActionTone, rgb, rgba, shell_surface};
use crate::micro_motion::MotionIconExt as _;
use crate::state::{AppState, Panel, Selection, Tab};
use crate::terminal_host::{TerminalHost, TerminalLaunchSpec};
use crate::zoom::px as zpx;
use crate::{AppShell, ResizePreview};
use del_core::typography::TEXT_SM;
use del_mcp::SessionMode;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub const PANEL_DEFAULT_HEIGHT: f32 = 200.0;
const PANEL_MIN_HEIGHT: f32 = 96.0;
const PANEL_HEADER_HEIGHT: f32 = 30.0;
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// Title-bar control size, matching the sidebar's header buttons.
const NAV_BUTTON: f32 = 28.0;
/// Narrowest navigation zone: toggle, back and forward with their gaps.
pub const NAV_MIN_WIDTH: f32 = NAV_BUTTON * 3.0 + 2.0 * 2.0 + 12.0 + 8.0;

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
            b.cursor_pointer().hover(|b| b.bg(rgba(0xffffff0a)))
        })
        .when(!enabled, |b| b.opacity(0.35))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
        .text_size(zpx(TEXT_SM))
        .text_color(crate::appearance::secondary_text())
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

impl AppShell {
    pub(super) fn start_status_ticker(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| {
                        if this.state.read(cx).window_active {
                            this.refresh_status(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
            }
        })
        .detach();
    }

    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        if self.status_refreshing {
            return;
        }
        self.status_refreshing = true;
        self.state.update(cx, |state, cx| {
            state.refresh_git(state.effective().2);
            state.refresh_selected_usage(cx);
        });
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
        for (panel, owner) in std::iter::once((&self.right, &self.right_key)).chain(
            self.right_cache
                .values()
                .map(|entry| (&entry.panel, &entry.key)),
        ) {
            roots.extend(panel.read(cx).process_roots(cx).into_iter().map(|pid| {
                crate::status_processes::Root {
                    pid,
                    owner: owner.clone(),
                }
            }));
        }
        let job = backend.runtime.spawn(async move {
            // Include reconnectable provider servers that may outlive their original parent.
            for session in backend
                .manager
                .list_sessions()
                .into_iter()
                .filter(|s| backend.manager.is_live(&s.id))
            {
                if let Ok(Some((pid, shared))) = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    backend.manager.session_process(&session.id),
                )
                .await
                {
                    roots.push(crate::status_processes::Root {
                        pid,
                        owner: (!shared).then(|| format!("chat:{}", session.id.as_str())),
                    });
                }
            }
            crate::status_processes::read(roots).await
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
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_sidebar(!this.sidebar_open, cx)),
                    )
                    .motion_child(true, |feedback| feedback.glyph(Glyph::PanelLeft)),
            )
            .child(div().flex_1().min_w(zpx(12.0)))
            .child(
                chrome_button("nav-back", "Back", back)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Back (⌘[)").build(window, cx)
                    })
                    .when(back, |b| {
                        b.on_click(cx.listener(|this, _, window, cx| this.navigate(-1, window, cx)))
                    })
                    .child(crate::icons::icon(IconName::ChevronLeft)),
            )
            .child(
                chrome_button("nav-forward", "Forward", forward)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Forward (⌘])").build(window, cx)
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
                .when(!maximized, |panel| {
                    panel.flex_none().h(zpx(height + PANEL_HEADER_HEIGHT))
                })
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
                                if maximized {
                                    "Restore panel size"
                                } else {
                                    "Maximize panel"
                                },
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
                        .when_some(
                            self.bottom.terminals.get(&root).cloned(),
                            |body, terminal| body.child(terminal),
                        ),
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
        let cost_label = format!(
            "Cost {}",
            crate::session_usage::money(usage.provider_cost(provider))
        );
        let remaining = limits.as_ref().and_then(|limits| limits.remaining());
        let limits_label = remaining
            .map(|n| format!("Limits {n:.0}% left"))
            .unwrap_or_else(|| "Limits —".into());
        let process_owner = match &state.selection {
            Selection::Session(id) => Some(format!("chat:{}", id.as_str())),
            Selection::Terminal(id) => Some(format!("terminal:{}", id.as_str())),
            Selection::Draft(_) => None,
        };
        let process_rows: Vec<_> = self
            .processes
            .rows
            .iter()
            .filter(|p| !p.background)
            .cloned()
            .collect();
        let process_count = process_rows
            .iter()
            .filter(|p| {
                process_owner
                    .as_ref()
                    .is_none_or(|owner| p.owner.as_ref() == Some(owner))
            })
            .count();
        let process_label = if !self.processes.ready {
            "Processes …".into()
        } else if self.processes.error.is_some() {
            "Processes —".into()
        } else {
            format!("{} processes", process_count)
        };
        let process_snapshot = self.processes.clone();
        let branch = if git.current.is_empty() {
            "No repository".into()
        } else {
            let changes = match git.changed_files {
                Some(0) => String::new(),
                Some(n) => format!(" · {n} {}", if n == 1 { "file" } else { "files" }),
                None => " · changes unavailable".into(),
            };
            format!(
                "{}{changes}{}{}",
                git.current,
                if git.ahead > 0 {
                    format!(" ↑{}", git.ahead)
                } else {
                    String::new()
                },
                if git.behind > 0 {
                    format!(" ↓{}", git.behind)
                } else {
                    String::new()
                }
            )
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
            .overflow_hidden()
            .bg(shell_surface())
            .text_size(zpx(TEXT_SM))
            .text_color(crate::appearance::secondary_text())
            .child(
                status_item("status-branch")
                    .min_w_0()
                    .max_w(zpx(390.))
                    .overflow_hidden()
                    .accessibility_label(format!("Git: {branch}"))
                    .tooltip(branch_tooltip)
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(zpx(6.))
                            .items_center()
                            .text_size(zpx(TEXT_SM))
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
                                    .child(div().min_w_0().truncate().child(branch))
                                    .when_some(
                                        // A clean tree has nothing to report.
                                        git.changed_lines.filter(|lines| *lines != (0, 0)),
                                        |row, (added, removed)| {
                                            row.child(
                                                div()
                                                    .flex_none()
                                                    .whitespace_nowrap()
                                                    .text_color(ActionTone::Success.foreground())
                                                    .child(format!("+{added}")),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .whitespace_nowrap()
                                                    .text_color(ActionTone::Danger.foreground())
                                                    .child(format!("−{removed}")),
                                            )
                                        },
                                    ),
                            ),
                    )
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::OpenChanges), cx)
                    }),
            )
            .child(div().flex_1().min_w(zpx(4.)))
            .child(
                status_item("status-context")
                    .accessibility_label(context_label.clone())
                    .tooltip(
                        "Conversation context currently used (history, instructions and tools)",
                    )
                    .child(context_label)
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let menu = menu.label("Context usage");
                        if terminal {
                            return menu.label("Not reported in Terminal mode");
                        }
                        match (usage.context_tokens, usage.context_window) {
                            (Some(used), Some(limit)) => menu
                                .label(format!("{used} / {limit} tokens"))
                                .label(format!(
                                    "{:.0}% used · {:.0}% remaining",
                                    used as f64 / limit as f64 * 100.,
                                    100. - (used as f64 / limit as f64 * 100.).min(100.)
                                ))
                                .label("Includes the conversation carried by the active model")
                                .label(
                                    "Older history may be compacted; this is not lifetime tokens",
                                ),
                            (Some(used), None) => menu
                                .label(format!("{used} tokens in the active conversation context"))
                                .label("This provider has not reported a context capacity"),
                            _ => menu.label("No context usage reported for this session yet"),
                        }
                    }),
            )
            .child(
                status_item("status-limits")
                    .accessibility_label(limits_label.clone())
                    .tooltip(format!(
                        "{} · {account_name}",
                        crate::providers::display_name(provider)
                    ))
                    .when(remaining.is_some_and(|n| n <= 10.), |b| {
                        b.text_color(ActionTone::Warning.foreground())
                    })
                    .child(limits_label)
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let mut menu = menu.label(format!(
                            "{} · {account_name}",
                            crate::providers::display_name(provider)
                        ));
                        if let Some(limits) = &limits {
                            for window in &limits.windows {
                                menu = menu.label(format!(
                                    "{}: {:.0}% left",
                                    window.label, window.remaining
                                ));
                                if let Some(reset) = &window.resets_at {
                                    menu = menu.label(format!("Resets {reset}"));
                                }
                            }
                        } else {
                            menu = menu.label("No allowance reported by this provider/account");
                        }
                        menu
                    }),
            )
            .child(
                status_item("status-cost")
                    .accessibility_label(cost_label.clone())
                    .tooltip(cost_breakdown)
                    .child(cost_label)
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let mut menu = menu.label("This conversation · provider costs");
                        for (provider, cost) in &cost_rows {
                            let name = match provider.as_str() {
                                "claude" => "Claude",
                                "codex" => "OpenAI / Codex",
                                "opencode" => "OpenCode",
                                other => other,
                            };
                            menu = menu.label(format!(
                                "{name}  {}",
                                crate::session_usage::money(Some(*cost))
                            ));
                        }
                        menu.separator()
                            .label(format!(
                                "Total  {}",
                                crate::session_usage::money(total_cost)
                            ))
                            .label("Estimates may exclude unreported or Terminal usage")
                    }),
            )
            .child(
                status_item("status-processes")
                    .accessibility_label(process_label.clone())
                    .tooltip(if process_owner.is_some() {
                        "Commands and servers · this session"
                    } else {
                        "Commands and servers · all sessions"
                    })
                    .child(process_label)
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let mut menu = menu.scrollable(true).max_h(zpx(320.));
                        if let Some(error) = &process_snapshot.error {
                            return menu.label(error.clone());
                        }
                        let describe = |process: &crate::status_processes::Process| {
                            let ports = if process.ports.is_empty() {
                                String::new()
                            } else {
                                format!(" · :{}", process.ports.join(", :"))
                            };
                            format!("{} · PID {}{ports}", process.name, process.pid)
                        };
                        if let Some(owner) = &process_owner {
                            menu = menu.label("This session · commands and servers");
                            let current = process_rows
                                .iter()
                                .filter(|p| p.owner.as_ref() == Some(owner))
                                .collect::<Vec<_>>();
                            if current.is_empty() {
                                menu = menu.label(if process_snapshot.ready {
                                    "0 running"
                                } else {
                                    "Reading processes…"
                                });
                            }
                            for process in current {
                                menu = menu.label(describe(process));
                            }
                            menu = menu.separator();
                        }
                        menu = menu.label("All sessions · commands and servers");
                        if process_rows.is_empty() {
                            menu = menu.label(if process_snapshot.ready {
                                "0 running"
                            } else {
                                "Reading processes…"
                            });
                        }
                        for process in &process_rows {
                            menu = menu.label(describe(process));
                        }
                        menu
                    }),
            )
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
                    .child(if terminal { "Terminal" } else { "Chat" })
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
