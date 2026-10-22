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

use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use del_core::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics, composer};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = 560.0;

/// Fill of the composer card: a faint white lift, like the page it sits on.
const COMPOSER_SURFACE_RGBA: u32 = 0xffffff12;

struct AppShell {
    composer: Entity<TextareaState>,
    metrics: ShellMetrics,
}

impl AppShell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask, build, or describe a change...")
                .auto_grow(1, 6)
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

        Self {
            composer,
            metrics: ShellMetrics::default(),
        }
    }

    /// Send the composer's text, if any, and clear it.
    ///
    /// There is no chat backend yet, so sending only clears the field.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value();
        if composer::submission(&text).is_none() {
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_send = composer::submission(&self.composer.read(cx).value()).is_some();
        let muted = rgba(0xffffff8c);

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
                    .bordered(false),
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
                                composer_chip("model")
                                    .child(
                                        div()
                                            .text_size(px(TEXT_MD))
                                            .text_color(rgb(0xf0f0f0))
                                            .child("Sonnet 5.5"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(TEXT_SM))
                                            .text_color(muted)
                                            .child("Medium"),
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
                            .child(
                                Icon::new(IconName::ArrowUp)
                                    .small()
                                    .text_color(if can_send {
                                        rgb(0x111113)
                                    } else {
                                        rgb(0x8a8a8e)
                                    }),
                            ),
                    ),
            )
    }

    fn render_topbar(&self, cx: &App) -> impl IntoElement {
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
                h_flex().h_full().items_center().child(
                    div()
                        .text_size(px(TEXT_MD))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("del"),
                ),
            )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = px(self.metrics.page_radius);

        div()
            .id("page")
            .relative()
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .bg(rgba(PAGE_SURFACE_RGBA))
            .child(
                v_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .px_3()
                    .child(
                        div()
                            .text_size(px(TEXT_LG))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xf5f5f5))
                            .child("What should we work on in del?"),
                    )
                    .child(self.render_composer(cx)),
            )
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
