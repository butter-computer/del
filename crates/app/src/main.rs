//! The `del` application shell: a custom top bar that hosts the macOS traffic
//! lights, over a rounded page inset from the window edges whose background is
//! the user's desktop wallpaper (or a bundled default).
//!
//! Layout metrics, the shell colour and wallpaper lookup live in `del_core`;
//! this crate decodes the wallpaper and renders.

mod wallpaper;

use std::sync::Arc;

use del_core::{SHELL_SURFACE_RGB, ShellMetrics, composer};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Initial window size in logical pixels.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(960.0), px(640.0));

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = 640.0;

struct AppShell {
    composer: Entity<TextareaState>,
    metrics: ShellMetrics,
    /// Decoded before the window opened, so the first frame already shows it.
    wallpaper: Option<Arc<RenderImage>>,
}

impl AppShell {
    fn new(
        wallpaper: Option<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask, build, or describe a change...")
                .auto_grow(2, 8)
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
            wallpaper,
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

        v_flex()
            .w_full()
            .max_w(px(COMPOSER_MAX_WIDTH))
            .gap_2()
            .p_3()
            .rounded(px(16.0))
            .bg(rgba(0x0b0b0dcc))
            .border_1()
            .border_color(rgba(0xffffff1f))
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .bordered(false),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("send")
                        .primary()
                        .label("Send")
                        .disabled(!can_send)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
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
            .bg(rgb(SHELL_SURFACE_RGB))
            .child(
                h_flex().h_full().items_center().child(
                    div()
                        .text_sm()
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
            .bg(rgb(0x14161b))
            .when_some(self.wallpaper.clone(), |page, image| {
                page.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Cover),
                )
            })
            // A scrim that darkens toward the bottom keeps text legible over
            // any wallpaper and blends the page into the dark shell.
            // It needs the page's radius too: `overflow_hidden` clips to the
            // page's rectangle, not its rounded outline, so an unrounded
            // scrim would paint square corners over the rounded wallpaper.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(radius)
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgba(0x00000040), 0.0),
                        linear_color_stop(rgba(0x000000d9), 1.0),
                    )),
            )
            .child(
                v_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .gap_5()
                    .px_6()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xf5f5f5))
                            .child("What should we work on in del?"),
                    )
                    .child(self.render_composer(cx)),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = px(self.metrics.page_inset);

        v_flex()
            .size_full()
            .bg(rgb(SHELL_SURFACE_RGB))
            .child(self.render_topbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(inset)
                    .pb(inset)
                    .child(self.render_page(cx)),
            )
    }
}

fn main() {
    // Kick off wallpaper lookup and decoding immediately; it overlaps GPUI's
    // startup and is joined right before the window is created.
    let wallpaper = wallpaper::preload();

    gpui_kit::application().run(|cx| {
        // Must run before any gpui-component feature is used.
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);

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
            window_background: WindowBackgroundAppearance::Opaque,
            ..TitleBar::window_options()
        };

        let wallpaper = wallpaper.finish();
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| AppShell::new(wallpaper, window, cx))
        })
        .expect("failed to open window");

        cx.activate(true);
    });
}
