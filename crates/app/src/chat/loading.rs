//! A transcript-shaped loading surface, with one shared shimmer clock.
use super::COLUMN_MAX_WIDTH;
use crate::appearance::rgba;
use crate::ui;
use crate::zoom::px as zpx;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

/// The column's bounds, measured each frame. Every placeholder line sweeps the
/// same band across this one span, so the bands line up instead of fanning out
/// at speeds set by each line's own width.
type Column = Rc<Cell<Bounds<Pixels>>>;

fn line(width: f32, sweep: Option<(f32, Column)>) -> impl IntoElement {
    div()
        .w(relative(width))
        .h(zpx(12.))
        .rounded(zpx(ui::RADIUS_ROW))
        .overflow_hidden()
        .bg(rgba(0xffffff10))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let Some((phase, column)) = &sweep else {
                        return;
                    };
                    let column = column.get();
                    let band = column.size.width * 0.3;
                    let center = column.left() - band + (column.size.width + band * 2.) * *phase;
                    let mask = ContentMask { bounds };
                    window.with_content_mask(Some(mask), |window| {
                        for (left, direction) in [(center - band, 90.), (center, 270.)] {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(left, bounds.top()),
                                    size(band, bounds.size.height),
                                ),
                                linear_gradient(
                                    direction,
                                    linear_color_stop(rgba(0xffffff00), 0.),
                                    linear_color_stop(rgba(0xffffff18), 1.),
                                ),
                            ));
                        }
                    });
                },
            )
            .size_full(),
        )
}

fn transcript(phase: Option<f32>, column: Column) -> impl IntoElement {
    let measured = column.clone();
    let sweep = move || phase.map(|phase| (phase, column.clone()));
    v_flex()
        .relative()
        .w_full()
        .max_w(zpx(COLUMN_MAX_WIDTH))
        .gap(zpx(ui::SPACE_2XL))
        // Measured before the lines paint, which read it.
        .child(
            canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .children([0.72, 0.54, 0.66].into_iter().map(|width| {
            v_flex()
                .w_full()
                .gap(zpx(ui::SPACE_XL))
                .child(
                    div()
                        .w_full()
                        .p(zpx(ui::SPACE_LG))
                        .rounded(zpx(ui::RADIUS_CARD))
                        .bg(rgba(0xffffff08))
                        .child(line(width, sweep())),
                )
                .child(
                    v_flex()
                        .px(zpx(ui::SPACE_SM))
                        .gap(zpx(ui::SPACE_MD))
                        .children(
                            [0.95, 0.87, 0.62]
                                .into_iter()
                                .map(|width| line(width, sweep())),
                        ),
                )
        }))
}

#[derive(IntoElement)]
struct ConversationLoading {
    message: SharedString,
    reduce_motion: bool,
}

impl RenderOnce for ConversationLoading {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let now = cx.background_executor().now();
        let since = window.use_keyed_state("conversation-loading-since", cx, |_, _| now);
        // Loads that finish quickly never flash a skeleton. The status label is
        // there from the first frame; only the picture waits.
        let shown = now.saturating_duration_since(*since.read(cx)) >= ui::LOADING_DELAY;
        let reduce_motion = self.reduce_motion || cx.reduce_motion();
        let column: Column = Rc::default();
        let body = v_flex().w_full().items_center();
        let body = if !shown {
            window.request_animation_frame();
            body.into_any_element()
        } else if reduce_motion {
            body.child(transcript(None, column.clone()))
                .into_any_element()
        } else {
            body.with_animation(
                "conversation-shimmer",
                Animation::new(ui::SHIMMER_PERIOD).repeat_synced(),
                move |body, phase| body.child(transcript(Some(phase), column.clone())),
            )
            .into_any_element()
        };
        v_flex()
            .id("conversation-loading")
            .role(Role::Status)
            .aria_label(self.message)
            .size_full()
            .overflow_hidden()
            .items_center()
            .px(zpx(ui::SPACE_2XL))
            .pt(zpx(ui::SPACE_XL))
            .child(body.into_any_element())
    }
}

pub fn conversation_loading(message: SharedString, reduce_motion: bool) -> AnyElement {
    ConversationLoading {
        message,
        reduce_motion,
    }
    .into_any_element()
}
