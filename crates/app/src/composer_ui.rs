//! The composer card shared by the new-chat page and the chat page: a
//! borderless card with the input on top and the attach/model chips plus the
//! send (and, while a turn runs, stop) buttons below.

use del_core::composer;
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Fill of the composer card: a faint white lift, like the page it sits on.
pub const COMPOSER_SURFACE_RGBA: u32 = 0xffffff12;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A borderless pill used for the composer's footer controls: it lays out its
/// children in a row and lightens on hover.
pub fn composer_chip(id: &'static str) -> Stateful<Div> {
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

#[derive(IntoElement)]
pub struct ComposerCard {
    input: Entity<TextareaState>,
    max_width: f32,
    on_send: ClickHandler,
    on_stop: Option<ClickHandler>,
}

impl ComposerCard {
    pub fn new(
        input: &Entity<TextareaState>,
        max_width: f32,
        on_send: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            input: input.clone(),
            max_width,
            on_send: Box::new(on_send),
            on_stop: None,
        }
    }

    /// Show a stop button; set while a turn is running.
    pub fn on_stop(
        mut self,
        on_stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_stop = Some(Box::new(on_stop));
        self
    }
}

impl RenderOnce for ComposerCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let can_send = composer::submission(&self.input.read(cx).value()).is_some();
        let muted = rgba(0xffffff8c);
        let ComposerCard {
            input,
            max_width,
            on_send,
            on_stop,
        } = self;

        // No border: the card is told apart from the page by its fill alone.
        v_flex()
            .w_full()
            .max_w(px(max_width))
            .p_2()
            .gap_1()
            .rounded(px(14.0))
            .bg(rgba(COMPOSER_SURFACE_RGBA))
            .child(
                Textarea::new(&input)
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
                        h_flex()
                            .items_center()
                            .gap_1()
                            .when_some(on_stop, |row, on_stop| {
                                row.child(
                                    div()
                                        .id("stop")
                                        .size(px(24.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(rgba(0xffffff1f))
                                        .cursor_pointer()
                                        .hover(|stop| stop.bg(rgba(0xffffff2e)))
                                        .on_click(move |e, w, cx| on_stop(e, w, cx))
                                        // A plain square: the stop glyph.
                                        .child(div().size(px(8.0)).rounded_xs().bg(rgb(0xf5f5f5))),
                                )
                            })
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
                                        send.cursor_pointer()
                                            .on_click(move |e, w, cx| on_send(e, w, cx))
                                    })
                                    .child(Icon::new(IconName::ArrowUp).small().text_color(
                                        if can_send {
                                            rgb(0x111113)
                                        } else {
                                            rgb(0x8a8a8e)
                                        },
                                    )),
                            ),
                    ),
            )
    }
}
