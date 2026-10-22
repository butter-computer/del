//! The `del` application shell: a custom top bar that hosts the macOS traffic
//! lights, over a rounded black page inset from the window edges. The window
//! itself is translucent and blurred, so the desktop shows through the shell.
//!
//! Layout metrics and the shell colours live in `del_core`; this crate renders
//! them.

// The wallpaper background is switched off for now; the loader stays so it can
// be wired back in without redoing the preload and decode work.
#[allow(dead_code)]
mod wallpaper;

mod assets;
mod chat;
mod composer_ui;
#[allow(dead_code)]
mod mcp_bridge;
mod mode_ui;
mod providers;
mod right_panel;
mod sessions;
mod sidebar;
mod signin;
mod state;
#[allow(dead_code)]
mod terminal_host;
mod terminal_store;

use chat::ChatView;
use composer_ui::ComposerCard;
use del_agent::{AgentBackend, MockBackend, Provider, SessionId};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use del_core::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics, composer};
use del_mcp::SessionMode;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mcp_bridge::McpBridge;
use mode_ui::PendingQuestion;
use right_panel::RightPanel;
use state::{AppState, Panel, Selection, Tab};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use terminal_host::{TerminalHost, TerminalLauncher};
use terminal_store::TerminalStore;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// The sidebar is reserved for something else for now: its code stays, but it is
/// neither shown nor toggleable (button and Cmd-B) until this is turned on.
const SIDEBAR_ENABLED: bool = true;

/// Every tab is this wide once settled; it animates to and from 0.
const TAB_WIDTH: f32 = 160.0;

/// Space after each tab, animated together with it.
const TAB_GAP: f32 = 2.0;

/// The title inside a tab.
fn tab_title(title: String) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(16.0))
        .line_height(px(16.0))
        .truncate()
        .child(title)
}

/// How long the right panel takes to slide open or closed.
const RIGHT_ANIM: Duration = Duration::from_millis(220);

/// The prompt above the composer.
const HEADING: &str = "What are we building?";

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = chat::COLUMN_MAX_WIDTH;

actions!(
    del,
    [
        NewSession,
        NextSession,
        PrevSession,
        FocusComposer,
        StopSession,
        ToggleProviders,
        ToggleSidebar,
        Quit,
        ToggleFiles,
        ToggleTerminal,
        CloseTab,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9
    ]
);

/// Backends for the session manager.
///
/// `DEL_BACKEND=mock` selects the scripted mock for both providers; otherwise
/// the real Codex and Claude backends drive the user's installed CLIs.
pub fn backends() -> Vec<Arc<dyn AgentBackend>> {
    if std::env::var("DEL_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("mock")) {
        return Provider::ALL
            .iter()
            .map(|&p| Arc::new(MockBackend::new(p)) as Arc<dyn AgentBackend>)
            .collect();
    }
    vec![
        Arc::new(del_codex::CodexBackend::new(Default::default())),
        Arc::new(del_acp::AcpBackend::default()),
        // Project settings only: the user's own `bypassPermissions` default would
        // otherwise make Claude never ask, so del's Ask mode could not work.
        Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
            setting_sources: Some("project".into()),
            ..Default::default()
        })),
    ]
}

struct AppShell {
    state: Entity<AppState>,
    composer: Entity<TextareaState>,
    rename: Entity<InputState>,
    /// The session whose title is loaded into the rename field.
    renaming_loaded: Option<SessionId>,
    sidebar_open: bool,
    right: Entity<RightPanel>,
    /// Where the right panel is heading (open or closed).
    right_open: bool,
    /// Whether it is on screen: stays true while the close animation plays.
    right_visible: bool,
    /// Bumped on every toggle so each one starts a fresh animation.
    right_toggles: u64,
    /// One chat view per session opened in this run.
    chats: HashMap<SessionId, Entity<ChatView>>,
    metrics: ShellMetrics,
    terminals: HashMap<SessionId, Entity<TerminalHost>>,
    launchers: HashMap<u64, Entity<TerminalLauncher>>,
    launcher_cwds: HashMap<u64, std::path::PathBuf>,
    terminal_store: TerminalStore,
    mcp: Option<McpBridge>,
    questions: VecDeque<PendingQuestion>,
    mcp_answer: Entity<InputState>,
    notices: VecDeque<(SessionId, String, String)>,
    reported: HashMap<SessionId, String>,
    mode_focus: Option<(Selection, SessionMode)>,
}

impl AppShell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask anything...")
                .auto_grow(3, 8)
                .submit_on_enter(true);
            state.set_submit_on_enter(true, cx);
            state
        });
        // Enter (without Shift) submits; Shift+Enter inserts a newline.
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )
        .detach();
        composer.update(cx, |state, cx| state.focus(window, cx));

        let state = cx.new(AppState::new);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();

        // Kill every child process when the app quits.
        let quit_state = state.clone();
        cx.on_app_quit(move |_, cx| {
            quit_state.read(cx).shutdown();
            async {}
        })
        .detach();

        let rename = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(&rename, window, |this, rename, event, _, cx| match event {
            InputEvent::PressEnter { .. } => {
                let title = rename.read(cx).value().to_string();
                this.state
                    .update(cx, |s, cx| s.finish_rename(Some(title), cx));
            }
            InputEvent::Blur => this.state.update(cx, |s, cx| s.finish_rename(None, cx)),
            _ => {}
        })
        .detach();

        let right_root = state.read(cx).effective().2;
        let right = cx.new(|cx| RightPanel::new(right_root, window, cx));
        let terminal_store = TerminalStore::load(&TerminalStore::path());
        state.update(cx, |s, _| {
            s.terminal_rows = terminal_store
                .records
                .iter()
                .map(|r| sessions::TerminalRow {
                    id: r.id.clone(),
                    title: r.title.clone(),
                    cwd: r.launch.cwd.clone(),
                    provider: r.launch.provider(),
                    active: false,
                    summary: String::new(),
                })
                .collect();
            if let Some(error) = &terminal_store.error {
                s.error = Some(error.clone());
            }
        });
        let mcp_answer = cx.new(|cx| InputState::new(window, cx).placeholder("Your answer…"));
        let mut shell = Self {
            state,
            composer,
            rename,
            renaming_loaded: None,
            sidebar_open: true,
            right,
            right_open: false,
            right_visible: false,
            right_toggles: 0,
            chats: HashMap::new(),
            metrics: ShellMetrics::default(),
            terminals: HashMap::new(),
            launchers: HashMap::new(),
            launcher_cwds: HashMap::new(),
            terminal_store,
            mcp: None,
            questions: VecDeque::new(),
            mcp_answer,
            notices: VecDeque::new(),
            reported: HashMap::new(),
            mode_focus: None,
        };
        shell.start_modes(window, cx);
        shell
    }

    /// Make sure the open session has a live chat view.
    fn ensure_chat(&mut self, cx: &mut Context<Self>) {
        self.chats.retain(|_, chat| !chat.read(cx).is_closed());
        let Selection::Session(id) = self.state.read(cx).selection.clone() else {
            return;
        };
        if self.chats.contains_key(&id) {
            return;
        }
        let (backend, first) = self
            .state
            .update(cx, |s, _| (s.backend(), s.take_first_prompt(&id)));
        let session = id.clone();
        let chat = cx.new(|cx| ChatView::new(backend, Some(session), first, cx));
        self.chats.insert(id, chat);
    }

    /// Open or close the right panel, animating its width.
    fn set_right(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.right_open == open {
            return;
        }
        self.right_open = open;
        self.right_toggles += 1;
        if open {
            self.right_visible = true;
        } else {
            // Keep it on screen until the close animation has finished.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RIGHT_ANIM * 12 / 10).await;
                let _ = this.update(cx, |this, cx| {
                    if !this.right_open {
                        this.right_visible = false;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn go_tab(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.select_nth(n, cx));
        self.focus_composer(window, cx);
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Selection::Terminal(id) = &self.state.read(cx).selection
            && let Some(terminal) = self.terminals.get(id)
        {
            terminal.update(cx, |t, cx| t.focus(window, cx));
            return;
        }
        if self.state.read(cx).mode() == SessionMode::Terminal
            && let Selection::Draft(key) = self.state.read(cx).selection
            && let Some(launcher) = self.launchers.get(&key)
        {
            launcher.update(cx, |l, cx| l.focus(window, cx));
            return;
        }
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// The open session's chat view, if it has one.
    fn open_chat(&self, cx: &App) -> Option<Entity<ChatView>> {
        match &self.state.read(cx).selection {
            Selection::Session(id) => self.chats.get(id).cloned(),
            Selection::Draft(_) | Selection::Terminal(_) => None,
        }
    }

    /// Send the composer's text: starts the session from a draft, or sends to
    /// (steers) the open one. Clears the field.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value();
        let Some(prompt) = composer::submission(&text) else {
            return;
        };
        let prompt = prompt.to_string();
        let chat = self.open_chat(cx);
        if chat.is_none() && self.state.read(cx).is_starting() {
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        match chat {
            Some(chat) => chat.update(cx, |c, cx| c.submit(prompt, cx)),
            None => self.state.update(cx, |s, cx| s.send(prompt, cx)),
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.open_chat(cx) {
            chat.update(cx, |c, cx| c.stop(cx));
        }
    }

    /// The shared composer card, identical on the new-chat and chat pages.
    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = self.open_chat(cx);
        let running = chat.as_ref().is_some_and(|c| c.read(cx).is_running());
        let (provider, model, cwd, locked, ready) = {
            let st = self.state.read(cx);
            let (provider, model, cwd) = st.effective();
            (
                provider,
                model,
                cwd,
                !st.is_draft(),
                st.status_of(provider).is_some_and(providers::ready),
            )
        };
        let (shell, state_p, state_m, state_f) = (
            cx.entity(),
            self.state.clone(),
            self.state.clone(),
            self.state.clone(),
        );
        let shell_stop = shell.clone();
        ComposerCard::new(
            &self.composer,
            COMPOSER_MAX_WIDTH,
            provider,
            ready,
            model,
            sessions::folder_name(&cwd),
            locked,
            running,
        )
        .on_send(move |window, cx| shell.update(cx, |this, cx| this.submit(window, cx)))
        .on_stop(move |_, cx| shell_stop.update(cx, |this, cx| this.stop(cx)))
        .on_provider(move |p, _, cx| state_p.update(cx, |s, cx| s.set_provider(p, cx)))
        .on_model(move |m, _, cx| state_m.update(cx, |s, cx| s.set_model(m, cx)))
        .on_folder(move |_, cx| state_f.update(cx, |s, cx| s.pick_folder(cx)))
    }

    /// Session tabs in the top bar, with a plus button for a new session. The
    /// bar is also the window's drag area, so tabs stop mouse-down from
    /// starting a drag.
    fn render_topbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgba(0xffffff8c);
        let (tabs, selection, panel) = {
            let st = self.state.read(cx);
            // Live tabs, with just-closed ones (ghosts) kept at their old place
            // while they shrink away.
            let mut tabs: Vec<(Option<Tab>, u64, String, Option<Provider>)> = st
                .tabs
                .iter()
                .map(|tab| {
                    let (title, provider) = st.tab_info(tab);
                    (Some(tab.clone()), st.uid_of(tab), title, provider)
                })
                .collect();
            for g in &st.ghosts {
                let at = g.at.min(tabs.len());
                tabs.insert(at, (None, g.uid, g.title.clone(), g.provider));
            }
            (tabs, st.selection.clone(), st.panel)
        };
        let tab = |id: SharedString, active: bool| {
            h_flex()
                .id(id.clone())
                .group(id)
                .h(px(24.0))
                .min_w(px(120.0))
                .max_w(px(200.0))
                .pl_2()
                .pr_1()
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(TEXT_MD))
                .text_color(if active { rgb(0xf5f5f5) } else { muted })
                .when(active, |t| t.bg(rgba(0xffffff14)))
                .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };

        TitleBar::new()
            .h(px(self.metrics.topbar_height))
            .border_b_0()
            // Painted once, here and on the gutter below, never underneath the
            // page: stacked translucent layers would make the page more
            // opaque than the bar.
            .bg(rgba(SHELL_SURFACE_RGBA))
            .child(
                h_flex()
                    .size_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .h_full()
                            .items_center()
                            .gap_0()
                            .pl_2()
                            .when(SIDEBAR_ENABLED, |row| {
                                row.child(
                                    div()
                                        .id("toggle-sidebar")
                                        .size(px(24.0))
                                        .mr_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|b| b.bg(rgba(0xffffff0a)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sidebar_open = !this.sidebar_open;
                                            cx.notify();
                                        }))
                                        .child(
                                            Icon::new(if self.sidebar_open {
                                                IconName::PanelLeftClose
                                            } else {
                                                IconName::PanelLeftOpen
                                            })
                                            .small()
                                            .text_color(muted),
                                        ),
                                )
                            })
                            .children(tabs.into_iter().map(|(tab_key, uid, title, provider)| {
                                let live = tab_key.is_some();
                                let active =
                                    tab_key.as_ref().is_some_and(|t| selection == t.selection())
                                        && panel == Panel::Chat;
                                let name = match &tab_key {
                                    Some(Tab::Session(id)) | Some(Tab::Terminal(id)) => {
                                        id.0.clone()
                                    }
                                    Some(Tab::Draft(k)) => format!("draft-{k}"),
                                    None => format!("closing-{uid}"),
                                };
                                let group = SharedString::from(format!("tab-{name}"));
                                let mut el = tab(group.clone(), active);
                                if let Some(tab_key) = tab_key.clone() {
                                    let (select, close) = (self.state.clone(), self.state.clone());
                                    let (select_tab, close_tab) = (tab_key.clone(), tab_key);
                                    el = el
                                        .on_click(move |_, _, cx| {
                                            let tab = select_tab.clone();
                                            select.update(cx, |s, cx| s.select_tab(&tab, cx));
                                        })
                                        .child(mode_ui::tab_mark(provider))
                                        .child(tab_title(title))
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("close-{name}")))
                                                .size(px(16.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .flex_none()
                                                .rounded_sm()
                                                // Shown on hover only; faded rather than hidden
                                                // so layout and painting stay stable.
                                                .opacity(0.0)
                                                .group_hover(group.clone(), |c| c.opacity(1.0))
                                                .hover(|c| c.bg(rgba(0xffffff1a)))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_click(move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    let tab = close_tab.clone();
                                                    close.update(cx, |s, cx| s.close_tab(&tab, cx));
                                                })
                                                .child(
                                                    Icon::new(IconName::Close)
                                                        .xsmall()
                                                        .text_color(muted),
                                                ),
                                        );
                                } else {
                                    // Closing: same look, no handlers.
                                    el = el
                                        .child(mode_ui::tab_mark(provider))
                                        .child(tab_title(title))
                                        .child(div().size(px(16.0)).flex_none());
                                }
                                // The tab keeps its full size inside a clip whose width
                                // (tab plus the gap after it) animates from 0 on open and
                                // back to 0 on close. Padding and gap live inside the clip,
                                // so nothing is left behind at width 0 and nothing jumps
                                // when a closed tab is finally removed.
                                div()
                                    .flex_none()
                                    .overflow_hidden()
                                    .child(
                                        el.w(px(TAB_WIDTH))
                                            .min_w(px(0.0))
                                            .max_w(px(TAB_WIDTH))
                                            .flex_none(),
                                    )
                                    .with_animation(
                                        SharedString::from(format!("tab-anim-{uid}-{live}")),
                                        Animation::new(state::TAB_ANIM)
                                            .with_easing(ease_out_quint()),
                                        move |clip, t| {
                                            let t = if live { t } else { 1.0 - t };
                                            clip.w(px((TAB_WIDTH + TAB_GAP) * t))
                                        },
                                    )
                            }))
                            .child(
                                div()
                                    .id("new-tab")
                                    .size(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|plus| plus.bg(rgba(0xffffff0a)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.state.update(cx, |s, cx| s.new_session(cx));
                                        this.focus_composer(window, cx);
                                    }))
                                    .child(Icon::new(IconName::Plus).small().text_color(muted)),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-right")
                            .size(px(24.0))
                            .mr_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|b| b.bg(rgba(0xffffff0a)))
                            .when(self.right_open, |b| b.bg(rgba(0xffffff14)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_right(!this.right_open, cx);
                            }))
                            .child(
                                Icon::new(if self.right_open {
                                    IconName::PanelRightClose
                                } else {
                                    IconName::PanelRightOpen
                                })
                                .small()
                                .text_color(muted),
                            ),
                    ),
            )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = px(self.metrics.page_radius);
        let (panel, error) = {
            let st = self.state.read(cx);
            (st.panel, st.error.clone())
        };
        let chat = self.open_chat(cx);
        if panel == Panel::Chat && self.state.read(cx).mode() == SessionMode::Terminal {
            return self.render_terminal_page(cx).into_any_element();
        }

        div()
            .id("page")
            .relative()
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .bg(rgba(PAGE_SURFACE_RGBA))
            .when(panel == Panel::Providers, |page| {
                page.child(signin::render(&self.state, cx))
            })
            .when(panel == Panel::Chat, |page| match chat {
                // Chat page: transcript above, the shared composer below.
                Some(chat) => page.child(
                    v_flex()
                        .size_full()
                        .child(div().flex_1().min_h_0().child(chat))
                        .child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .px_3()
                                .pb_3()
                                .child(self.render_composer(cx)),
                        ),
                ),
                // New-chat page: heading and the same composer, centred.
                None => page.child(
                    v_flex()
                        .absolute()
                        .inset_0()
                        .items_center()
                        .justify_center()
                        .px_3()
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(px(COMPOSER_MAX_WIDTH))
                                .items_start()
                                .gap_2()
                                .child(
                                    div()
                                        // The composer's padding plus the input's own
                                        // inset, so the heading and the input text
                                        // share a left edge.
                                        .pl(px(17.5))
                                        .text_size(px(TEXT_LG))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(0xf5f5f5))
                                        .child(HEADING),
                                )
                                .child(self.render_mode_picker(cx))
                                .child(self.render_composer(cx))
                                .when_some(error, |col, message| {
                                    col.child(
                                        div()
                                            .pl(px(17.5))
                                            .text_size(px(TEXT_SM))
                                            .text_color(rgb(0xf0524f))
                                            .child(message),
                                    )
                                }),
                        ),
                ),
            })
            .into_any_element()
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = px(self.metrics.page_inset);
        self.ensure_modes(window, cx);
        let mode_key = (
            self.state.read(cx).selection.clone(),
            self.state.read(cx).mode(),
        );
        if self.state.read(cx).panel == Panel::Chat && self.mode_focus.as_ref() != Some(&mode_key) {
            self.mode_focus = Some(mode_key);
            self.focus_composer(window, cx);
        }
        self.ensure_chat(cx);
        let root = self.state.read(cx).effective().2;
        self.right.update(cx, |r, cx| r.set_root(root, cx));

        // Load the title into the rename field when a rename starts.
        let renaming = self.state.read(cx).renaming.clone();
        if renaming != self.renaming_loaded {
            if let Some(id) = &renaming {
                let title = self.state.read(cx).current_title(id);
                self.rename.update(cx, |input, cx| {
                    input.set_value(title, window, cx);
                    input.focus(window, cx);
                });
            }
            self.renaming_loaded = renaming;
        }

        v_flex()
            .size_full()
            .key_context("Shell")
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if !SIDEBAR_ENABLED {
                    return;
                }
                this.sidebar_open = !this.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| {
                this.set_right(!this.right_open, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                if !this.right_open {
                    this.set_right(true, cx);
                }
                this.right.update(cx, |r, cx| r.show_terminal(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.state.update(cx, |s, cx| s.close_active_tab(cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Tab1, w, cx| this.go_tab(0, w, cx)))
            .on_action(cx.listener(|this, _: &Tab2, w, cx| this.go_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &Tab3, w, cx| this.go_tab(2, w, cx)))
            .on_action(cx.listener(|this, _: &Tab4, w, cx| this.go_tab(3, w, cx)))
            .on_action(cx.listener(|this, _: &Tab5, w, cx| this.go_tab(4, w, cx)))
            .on_action(cx.listener(|this, _: &Tab6, w, cx| this.go_tab(5, w, cx)))
            .on_action(cx.listener(|this, _: &Tab7, w, cx| this.go_tab(6, w, cx)))
            .on_action(cx.listener(|this, _: &Tab8, w, cx| this.go_tab(7, w, cx)))
            .on_action(cx.listener(|this, _: &Tab9, w, cx| this.go_tab(8, w, cx)))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.state.update(cx, |s, cx| s.new_session(cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(-1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &StopSession, _, cx| {
                this.state.update(cx, |s, cx| s.stop(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleProviders, _, cx| {
                this.state.update(cx, |s, cx| s.toggle_providers(cx));
            }))
            .child(self.render_topbar(cx))
            .child(self.render_agent_messages(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .bg(rgba(SHELL_SURFACE_RGBA))
                    .when(SIDEBAR_ENABLED && self.sidebar_open, |row| {
                        row.child(sidebar::render(&self.state, &self.rename, cx))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(inset)
                            .pb(inset)
                            .child(self.render_page(cx)),
                    )
                    .when(self.right_visible, |row| {
                        let opening = self.right_open;
                        let full = right_panel::PANEL_WIDTH + self.metrics.page_inset;
                        row.child(
                            div()
                                .h_full()
                                .flex_none()
                                .overflow_hidden()
                                .child(
                                    // Fills whatever width the animation gives it: the panel
                                    // grows out of the page's own space and lays out at the
                                    // current width instead of sliding in from offscreen.
                                    div().size_full().pr(inset).pb(inset).child(
                                        div()
                                            .size_full()
                                            .rounded(px(self.metrics.page_radius))
                                            .overflow_hidden()
                                            .bg(rgba(PAGE_SURFACE_RGBA))
                                            .child(self.right.clone()),
                                    ),
                                )
                                .with_animation(
                                    ("right-panel", self.right_toggles),
                                    Animation::new(RIGHT_ANIM).with_easing(ease_out_quint()),
                                    move |el, t| {
                                        let t = if opening { t } else { 1.0 - t };
                                        el.w(px(full * t))
                                    },
                                ),
                        )
                    }),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            // Must run before any gpui-component feature is used.
            gpui_kit::init(cx);
            del_terminal::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            // The window root paints the theme background across the whole
            // window, which would hide the blurred window material behind it. The
            // shell paints its own translucent surfaces, so make this one clear.
            Theme::update(cx, |theme| {
                theme.colors.background = gpui::transparent_black();
                // The composer input inherits the theme size; keep it on the
                // type scale.
                theme.font_size = px(TEXT_MD);
                // Dropdown menus use the composer's surface, not the stock
                // near-black popover with a hard border.
                theme.colors.popover = rgb(0x2b2b2e).into();
                theme.colors.popover_foreground = rgb(0xf0f0f0).into();
                theme.colors.border = rgba(0xffffff14).into();
                theme.colors.accent = rgba(0xffffff14).into();
                theme.colors.accent_foreground = rgb(0xf5f5f5).into();
            });

            cx.bind_keys([
                KeyBinding::new("cmd-\\", ToggleFiles, None),
                KeyBinding::new("cmd-shift-b", ToggleFiles, None),
                KeyBinding::new("ctrl-`", ToggleTerminal, None),
                KeyBinding::new("cmd-j", ToggleTerminal, None),
                KeyBinding::new("cmd-t", NewSession, None),
                KeyBinding::new("cmd-n", NewSession, None),
                KeyBinding::new("cmd-w", CloseTab, None),
                KeyBinding::new("cmd-1", Tab1, None),
                KeyBinding::new("cmd-2", Tab2, None),
                KeyBinding::new("cmd-3", Tab3, None),
                KeyBinding::new("cmd-4", Tab4, None),
                KeyBinding::new("cmd-5", Tab5, None),
                KeyBinding::new("cmd-6", Tab6, None),
                KeyBinding::new("cmd-7", Tab7, None),
                KeyBinding::new("cmd-8", Tab8, None),
                KeyBinding::new("cmd-9", Tab9, None),
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-tab", NextSession, None),
                KeyBinding::new("cmd-shift-]", NextSession, None),
                KeyBinding::new("ctrl-shift-tab", PrevSession, None),
                KeyBinding::new("cmd-shift-[", PrevSession, None),
                KeyBinding::new("cmd-l", FocusComposer, None),
                KeyBinding::new("cmd-.", StopSession, None),
                KeyBinding::new("cmd-,", ToggleProviders, None),
            ]);
            // Quit runs the shutdown hook, which ends every backend process.
            cx.on_action(|_: &Quit, cx| cx.quit());
            // Closing the window quits, which also runs the shutdown hook.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let metrics = ShellMetrics::default();
            let (light_x, light_y) = metrics.traffic_light_origin();

            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("del".into()),
                    traffic_light_position: Some(point(px(light_x), px(light_y))),
                    // Hides the system title bar so the top bar below is drawn
                    // entirely by us.
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    WINDOW_SIZE,
                    cx,
                ))),
                window_min_size: Some(size(px(480.0), px(320.0))),
                window_background: WindowBackgroundAppearance::Blurred,
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| AppShell::new(window, cx))
            })
            .expect("failed to open window");

            cx.activate(true);
        });
}
