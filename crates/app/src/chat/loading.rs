//! A transcript-shaped loading surface, with one shared shimmer clock.
use super::COLUMN_MAX_WIDTH;
use crate::appearance::rgba;
use crate::zoom::px as zpx;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

fn line(width: f32, phase: Option<f32>) -> impl IntoElement {
    div()
        .w(relative(width))
        .h(zpx(12.))
        .rounded(zpx(6.))
        .overflow_hidden()
        .bg(rgba(0xffffff10))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let Some(phase) = phase else { return };
                    let band = bounds.size.width * 0.3;
                    let center = bounds.left() - band + (bounds.size.width + band * 2.) * phase;
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

fn transcript(phase: Option<f32>) -> impl IntoElement {
    v_flex()
        .w_full()
        .max_w(zpx(COLUMN_MAX_WIDTH))
        .gap_8()
        .children([0.72, 0.54, 0.66].into_iter().map(|width| {
            v_flex()
                .w_full()
                .gap_5()
                .child(
                    div()
                        .w_full()
                        .p_3()
                        .rounded_lg()
                        .bg(rgba(0xffffff08))
                        .child(line(width, phase)),
                )
                .child(
                    v_flex().px_2().gap_3().children(
                        [0.95, 0.87, 0.62]
                            .into_iter()
                            .map(|width| line(width, phase)),
                    ),
                )
        }))
}

pub fn conversation_loading(message: SharedString, reduce_motion: bool) -> AnyElement {
    let body = v_flex().w_full().items_center();
    let body = if reduce_motion {
        body.child(transcript(None)).into_any_element()
    } else {
        body.with_animation(
            "conversation-shimmer",
            Animation::new(Duration::from_millis(1400)).repeat(),
            |body, phase| body.child(transcript(Some(phase))),
        )
        .into_any_element()
    };
    v_flex()
        .id("conversation-loading")
        .role(Role::Status)
        .aria_label(message)
        .size_full()
        .overflow_hidden()
        .items_center()
        .px_6()
        .pt_5()
        .child(body)
        .into_any_element()
}
