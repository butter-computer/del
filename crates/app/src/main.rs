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
mod providers;
mod sessions;
mod sidebar;
mod signin;
mod state;

use chat::ChatView;
use composer_ui::ComposerCard;
use del_agent::{AgentBackend, MockBackend, Provider, SessionId};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use del_core::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics, composer};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use state::{AppState, Panel, Selection};
use std::collections::HashMap;
use std::sync::Arc;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// The sidebar is reserved for something else for now: its code stays, but it is
/// neither shown nor toggleable (button and Cmd-B) until this is turned on.
const SIDEBAR_ENABLED: bool = false;

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
        Quit
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
    /// One chat view per session opened in this run.
    chats: HashMap<SessionId, Entity<ChatView>>,
    metrics: ShellMetrics,
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

        Self {
            state,
            composer,
            rename,
            renaming_loaded: None,
            sidebar_open: true,
            chats: HashMap::new(),
            metrics: ShellMetrics::default(),
        }
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

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// The open session's chat view, if it has one.
    fn open_chat(&self, cx: &App) -> Option<Entity<ChatView>> {
        match &self.state.read(cx).selection {
            Selection::Session(id) => self.chats.get(id).cloned(),
            Selection::Draft => None,
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
        if chat.is_none() && self.state.read(cx).starting {
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
            let tabs: Vec<(SessionId, String, Option<Provider>)> = st
                .open_tabs
                .iter()
                .map(|id| (id.clone(), st.current_title(id), st.provider_of(id)))
                .collect();
            (tabs, st.selection.clone(), st.panel)
        };
        let draft_active = selection == Selection::Draft && panel == Panel::Chat;
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
                    .h_full()
                    .items_center()
                    .gap_0p5()
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
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                    .children(tabs.into_iter().map(|(id, title, provider)| {
                        let active =
                            selection == Selection::Session(id.clone()) && panel == Panel::Chat;
                        let (select, close) = (self.state.clone(), self.state.clone());
                        let (select_id, close_id) = (id.clone(), id.clone());
                        let group = SharedString::from(format!("tab-{}", id.0));
                        tab(group.clone(), active)
                            .on_click(move |_, _, cx| {
                                let id = select_id.clone();
                                select.update(cx, |s, cx| s.select(id, cx));
                            })
                            .children(provider.map(provider_badge))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(16.0))
                                    .line_height(px(16.0))
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("close-{}", id.0)))
                                    .size(px(16.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_none()
                                    .rounded_sm()
                                    // Shown on hover only; faded rather than hidden so
                                    // layout and painting stay stable.
                                    .opacity(0.0)
                                    .group_hover(group.clone(), |c| c.opacity(1.0))
                                    .hover(|c| c.bg(rgba(0xffffff1a)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        let id = close_id.clone();
                                        close.update(cx, |s, cx| s.close_tab(&id, cx));
                                    })
                                    .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                            )
                    }))
                    .when(draft_active, |row| {
                        row.child(tab("tab-draft".into(), true).child("New session"))
                    })
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.state.update(cx, |s, cx| s.new_session(cx));
                                this.focus_composer(window, cx);
                            }))
                            .child(Icon::new(IconName::Plus).small().text_color(muted)),
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
    }
}

/// The agent's logo, drawn in the logo's own colour with no background.
fn provider_badge(provider: Provider) -> impl IntoElement {
    let (path, color) = match provider {
        Provider::Claude => ("icons/claude.svg", 0xd97757),
        Provider::Codex => ("icons/codex.svg", 0xf5f5f5),
    };
    svg()
        .path(path)
        .size(px(14.0))
        .flex_none()
        .text_color(rgb(color))
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = px(self.metrics.page_inset);
        self.ensure_chat(cx);

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
                    ),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            // Must run before any gpui-component feature is used.
            gpui_kit::init(cx);
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
                KeyBinding::new("cmd-n", NewSession, None),
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-]", NextSession, None),
                KeyBinding::new("cmd-[", PrevSession, None),
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
