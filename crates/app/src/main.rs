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

mod chat;
mod composer_ui;
mod providers;
mod sessions;
mod sidebar;
mod signin;
mod state;

use chat::ChatView;
use del_agent::{AgentBackend, MockBackend, Provider, SessionId, SessionStatus};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use del_core::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics, composer};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use state::{AppState, Panel, Selection};
use std::collections::HashMap;
use std::sync::Arc;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// The prompt above the composer.
const HEADING: &str = "What are we building?";

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = 560.0;

/// Fill of the composer card: a faint white lift, like the page it sits on.
const COMPOSER_SURFACE_RGBA: u32 = 0xffffff12;

actions!(
    del,
    [
        NewSession,
        NextSession,
        PrevSession,
        FocusComposer,
        StopSession,
        ToggleProviders
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
        Arc::new(del_claude::ClaudeBackend::default()),
    ]
}

struct AppShell {
    state: Entity<AppState>,
    composer: Entity<TextareaState>,
    rename: Entity<InputState>,
    /// The session whose title is loaded into the rename field.
    renaming_loaded: Option<SessionId>,
    /// One chat view per session opened in this run.
    chats: HashMap<SessionId, Entity<ChatView>>,
    metrics: ShellMetrics,
}

impl AppShell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask, build, or describe a change...")
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
            chats: HashMap::new(),
            metrics: ShellMetrics::default(),
        }
    }

    /// Make sure the open session has a live chat view.
    fn ensure_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let (provider, model, _) = self.state.read(cx).effective();
        let label = format!(
            "{} · {}",
            providers::display_name(provider),
            providers::model_label(model.as_deref())
        );
        let session = id.clone();
        let chat = cx.new(|cx| ChatView::new(backend, Some(session), label, first, window, cx));
        self.chats.insert(id, chat);
    }

    fn has_chat(&self, cx: &App) -> bool {
        match &self.state.read(cx).selection {
            Selection::Session(id) => self.chats.contains_key(id),
            Selection::Draft => false,
        }
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// Send the composer's text to the open session (starting it if it is a
    /// draft) and clear the field.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read(cx).active_status() == SessionStatus::Running {
            return;
        }
        let text = self.composer.read(cx).value();
        let Some(prompt) = composer::submission(&text) else {
            return;
        };
        let prompt = prompt.to_string();
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.state.update(cx, |s, cx| s.send(prompt, cx));
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (running, is_draft, provider, model, cwd, provider_ready) = {
            let st = self.state.read(cx);
            let (provider, model, cwd) = st.effective();
            (
                st.active_status() == SessionStatus::Running,
                st.selection == Selection::Draft,
                provider,
                model,
                cwd,
                st.status_of(provider).is_some_and(providers::ready),
            )
        };
        let can_send = !running && composer::submission(&self.composer.read(cx).value()).is_some();
        let muted = rgba(0xffffff8c);
        let (state_p, state_m, state_f, state_stop) = (
            self.state.clone(),
            self.state.clone(),
            self.state.clone(),
            self.state.clone(),
        );

        // No border: the card is told apart from the page by its fill alone.
        v_flex()
            .w_full()
            .max_w(px(COMPOSER_MAX_WIDTH))
            .p_2()
            .gap_1()
            .rounded(px(14.0))
            .bg(rgba(COMPOSER_SURFACE_RGBA))
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .bordered(false)
                    // Same token as the chips below, so the input and the
                    // model selector read at one size.
                    .text_size(px(TEXT_MD)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_0p5()
                            .child(
                                composer_chip("attach")
                                    .child(Icon::new(IconName::Plus).small().text_color(muted)),
                            )
                            .child(
                                picker_chip("provider", is_draft)
                                    .on_click(move |_, _, cx| {
                                        state_p.update(cx, |s, cx| s.cycle_provider(cx))
                                    })
                                    .child(
                                        div().size(px(6.0)).rounded_full().bg(rgb(
                                            if provider_ready { 0x4cc38a } else { 0xf5a524 },
                                        )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(TEXT_MD))
                                            .text_color(rgb(0xf0f0f0))
                                            .child(providers::display_name(provider)),
                                    ),
                            )
                            .child(
                                picker_chip("model", is_draft)
                                    .on_click(move |_, _, cx| {
                                        state_m.update(cx, |s, cx| s.cycle_model(cx))
                                    })
                                    .child(
                                        div()
                                            .text_size(px(TEXT_MD))
                                            .text_color(muted)
                                            .child(providers::model_label(model.as_deref())),
                                    ),
                            )
                            .child(
                                picker_chip("folder", is_draft)
                                    .on_click(move |_, _, cx| {
                                        state_f.update(cx, |s, cx| s.pick_folder(cx))
                                    })
                                    .child(
                                        div()
                                            .text_size(px(TEXT_MD))
                                            .text_color(muted)
                                            .child(sessions::folder_name(&cwd)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("send")
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(if can_send {
                                rgb(0xffffff)
                            } else {
                                rgb(0x4a4a4e)
                            })
                            .when(can_send, |send| {
                                send.cursor_pointer().on_click(
                                    cx.listener(|this, _, window, cx| this.submit(window, cx)),
                                )
                            })
                            .when(running, |send| {
                                send.cursor_pointer()
                                    .bg(rgb(0xffffff))
                                    .on_click(move |_, _, cx| {
                                        state_stop.update(cx, |s, cx| s.stop(cx))
                                    })
                                    .child(div().size(px(8.0)).rounded_sm().bg(rgb(0x111113)))
                            })
                            .when(!running, |send| {
                                send.child(Icon::new(IconName::ArrowUp).small().text_color(
                                    if can_send {
                                        rgb(0x111113)
                                    } else {
                                        rgb(0x8a8a8e)
                                    },
                                ))
                            }),
                    ),
            )
    }

    fn render_topbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgba(0xffffff8c);
        let title = {
            let st = self.state.read(cx);
            match (st.panel, st.active_row()) {
                (Panel::Providers, _) => "Providers".to_string(),
                (_, Some(row)) => row.title,
                _ => sessions::DEFAULT_TITLE.to_string(),
            }
        };

        // The system title bar is hidden (`appears_transparent`), so this bar
        // is the whole chrome: it paints the shared shell colour with no
        // bottom border, owns window dragging, and leaves room on the left for
        // the inset traffic lights.
        TitleBar::new()
            .h(px(self.metrics.topbar_height))
            .border_b_0()
            // Painted once, here and on the gutter below, never underneath the
            // page: stacked translucent layers would make the page more
            // opaque than the bar.
            .bg(rgba(SHELL_SURFACE_RGBA))
            .child(
                div()
                    .pl_2()
                    .text_size(px(TEXT_MD))
                    .text_color(muted)
                    .child(title),
            )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = px(self.metrics.page_radius);
        let (panel, heading, error, open_chat) = {
            let st = self.state.read(cx);
            let heading = st.active_row().map_or(HEADING.to_string(), |row| row.title);
            let open_chat = match &st.selection {
                Selection::Session(id) => self.chats.get(id).cloned(),
                Selection::Draft => None,
            };
            (st.panel, heading, st.error.clone(), open_chat)
        };

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
            .when_some(open_chat.filter(|_| panel == Panel::Chat), |page, chat| {
                page.child(chat)
            })
            .when(panel == Panel::Chat && !self.has_chat(cx), |page| {
                page.child(
                    // Centre one column on the page, and left-align its contents so
                    // the heading starts at the composer's left edge.
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
                                        .child(heading),
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
                )
            })
    }
}

/// A footer chip that cycles or opens a choice on click. Once a session has
/// started its choices are fixed, so the chip stops reacting.
fn picker_chip(id: &'static str, enabled: bool) -> Stateful<Div> {
    let chip = h_flex()
        .id(id)
        .h(px(24.0))
        .px_1p5()
        .gap_1()
        .items_center()
        .rounded_full();
    if enabled {
        chip.cursor_pointer()
            .hover(|chip| chip.bg(rgba(0xffffff14)))
    } else {
        chip
    }
}

/// A borderless pill used for the composer's footer controls: it lays out its
/// children in a row and lightens on hover.
fn composer_chip(id: &'static str) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(24.0))
        .px_1p5()
        .gap_1()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|chip| chip.bg(rgba(0xffffff14)))
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = px(self.metrics.page_inset);
        self.ensure_chat(window, cx);

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
                    .child(sidebar::render(&self.state, &self.rename, cx))
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
        .with_assets(gpui_kit::assets::Assets)
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
            });

            cx.bind_keys([
                KeyBinding::new("cmd-n", NewSession, None),
                KeyBinding::new("cmd-]", NextSession, None),
                KeyBinding::new("cmd-[", PrevSession, None),
                KeyBinding::new("cmd-l", FocusComposer, None),
                KeyBinding::new("cmd-.", StopSession, None),
                KeyBinding::new("cmd-,", ToggleProviders, None),
            ]);
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
