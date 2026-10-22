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

use chat::{Backend, ChatView};
use composer_ui::ComposerCard;
use del_agent::{MockBackend, PermissionMode, Provider, SessionConfig, SessionManager};
use del_core::tabs::Tabs;
use del_core::typography::{TEXT_LG, TEXT_MD};
use del_core::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics, composer};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// The prompt above the composer.
const HEADING: &str = "What are we building?";

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = 560.0;

struct AppShell {
    tabs: Tabs,
    composer: Entity<TextareaState>,
    metrics: ShellMetrics,
    backend: Backend,
    /// The chat page of each tab that has had a first message sent.
    chats: HashMap<u64, Entity<ChatView>>,
}

impl AppShell {
    fn new(backend: Backend, demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

        let mut shell = Self {
            tabs: Tabs::new(),
            composer,
            metrics: ShellMetrics::default(),
            backend,
            chats: HashMap::new(),
        };
        if demo {
            let id = shell.tabs.active_id();
            let chat = shell.new_chat(window, cx);
            chat.update(cx, |chat, cx| chat.preload(&chat::demo::events(), cx));
            shell.chats.insert(id, chat);
        }
        shell
    }

    fn new_chat(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChatView> {
        let backend = self.backend.clone();
        cx.new(|cx| ChatView::new(backend, window, cx))
    }

    /// Send the composer's text, if any, and move this tab to its chat page.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value();
        let Some(text) = composer::submission(&text) else {
            return;
        };
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        let chat = self.new_chat(window, cx);
        chat.update(cx, |chat, cx| chat.send_first(text, cx));
        self.chats.insert(self.tabs.active_id(), chat);
        cx.notify();
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ComposerCard::new(
            &self.composer,
            COMPOSER_MAX_WIDTH,
            cx.listener(|this, _, window, cx| this.submit(window, cx)),
        )
    }

    fn render_topbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgba(0xffffff8c);
        let active_id = self.tabs.active_id();

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
                h_flex()
                    .h_full()
                    .items_center()
                    .gap_0p5()
                    .pl_2()
                    .children(self.tabs.tabs().iter().map(|tab| {
                        let id = tab.id;
                        let active = id == active_id;
                        div()
                            .id(("tab", id as usize))
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(TEXT_MD))
                            .text_color(if active { rgb(0xf5f5f5) } else { muted })
                            .when(active, |tab| tab.bg(rgba(0xffffff14)))
                            .when(!active, |tab| tab.hover(|tab| tab.bg(rgba(0xffffff0a))))
                            // Clicking a tab must not start a window drag.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tabs.select(id);
                                cx.notify();
                            }))
                            .child(tab.title.clone())
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tabs.add();
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Plus).small().text_color(muted)),
                    ),
            )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let radius = px(self.metrics.page_radius);
        let page = || {
            div()
                .id("page")
                .relative()
                .size_full()
                .rounded(radius)
                .overflow_hidden()
                .bg(rgba(PAGE_SURFACE_RGBA))
        };

        if let Some(chat) = self.chats.get(&self.tabs.active_id()) {
            return page().child(chat.clone()).into_any_element();
        }

        div()
            .id("page")
            .relative()
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .bg(rgba(PAGE_SURFACE_RGBA))
            .child(
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
                                    .child(HEADING),
                            )
                            .child(self.render_composer(cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = px(self.metrics.page_inset);

        v_flex().size_full().child(self.render_topbar(cx)).child(
            div()
                .flex_1()
                .min_h_0()
                .bg(rgba(SHELL_SURFACE_RGBA))
                .px(inset)
                .pb(inset)
                .child(self.render_page(cx)),
        )
    }
}

/// Build the agent backend from `DEL_BACKEND`: `mock` and `mock-demo` use the
/// in-memory `MockBackend` (no model needed); anything else registers no
/// backend yet, so sending shows an error banner. Returns the backend and
/// whether to open on the canned demo transcript.
fn backend_from_env(runtime: &tokio::runtime::Runtime) -> (Backend, bool) {
    let mode = std::env::var("DEL_BACKEND").unwrap_or_default();
    let mock = mode.starts_with("mock");
    let backends: Vec<Arc<dyn del_agent::AgentBackend>> = if mock {
        vec![Arc::new(
            MockBackend::new(Provider::Claude).with_delay(Duration::from_millis(60)),
        )]
    } else {
        Vec::new()
    };
    let index = if mock {
        std::env::temp_dir().join("del-mock-sessions.jsonl")
    } else {
        del_agent::default_index_path().expect("no data directory")
    };
    let manager = SessionManager::new(index, backends).expect("failed to open session index");
    let config = SessionConfig {
        cwd: std::env::current_dir().unwrap_or_default(),
        model: None,
        provider: Provider::Claude,
        permission_mode: PermissionMode::Ask,
    };
    let backend = Backend {
        runtime: runtime.handle().clone(),
        manager,
        config,
    };
    (backend, mode == "mock-demo")
}

fn main() {
    // Agent calls and the broadcast pumps run here; GPUI only awaits their
    // results. Kept alive for the life of the app.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to start the agent runtime");
    let (backend, demo) = backend_from_env(&runtime);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
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
                cx.new(|cx| AppShell::new(backend, demo, window, cx))
            })
            .expect("failed to open window");

            cx.activate(true);
        });
}
