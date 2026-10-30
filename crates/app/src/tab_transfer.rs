//! Overflow fades shared by session tabs, sidebar tools, and the session list.
use crate::zoom::px as zpx;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
pub(crate) fn edge_fades(scroll: &ScrollHandle, surface: Rgba) -> Vec<AnyElement> {
    scroll_edge_fades(scroll, surface, false)
}

pub(crate) fn vertical_edge_fades(scroll: &ScrollHandle, surface: Rgba) -> Vec<AnyElement> {
    scroll_edge_fades(scroll, surface, true)
}

fn scroll_edge_fades(scroll: &ScrollHandle, surface: Rgba, vertical: bool) -> Vec<AnyElement> {
    let extent = zpx(if vertical { 40. } else { 16. });
    [true, false]
        .into_iter()
        .map(|left| {
            let scroll = scroll.clone();
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, _| {
                    let offset = if vertical {
                        scroll.offset().y
                    } else {
                        scroll.offset().x
                    };
                    let max = if vertical {
                        scroll.max_offset().y
                    } else {
                        scroll.max_offset().x
                    };
                    let distance = if left { -offset } else { max + offset };
                    let alpha = (distance / extent).clamp(0., 1.);
                    if alpha > 0. {
                        window.paint_quad(fill(
                            bounds,
                            linear_gradient(
                                if vertical {
                                    if left { 180. } else { 0. }
                                } else if left {
                                    90.
                                } else {
                                    270.
                                },
                                linear_color_stop(surface.opacity(alpha), 0.),
                                linear_color_stop(surface.opacity(0.), 1.),
                            ),
                        ));
                    }
                },
            )
            .absolute()
            .when(vertical, |fade| {
                fade.left_0()
                    .right_0()
                    .h(extent)
                    .when(left, |fade| fade.top_0())
                    .when(!left, |fade| fade.bottom_0())
            })
            .when(!vertical, |fade| {
                fade.top_0()
                    .bottom_0()
                    .w(extent)
                    .when(left, |fade| fade.left_0())
                    .when(!left, |fade| fade.right_0())
            })
            .into_any_element()
        })
        .collect()
}
