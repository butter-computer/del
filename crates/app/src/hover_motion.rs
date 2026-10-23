//! One moving highlight shared by a group of rows. Bounds are measured rather
//! than inferred, so separators, scrolling, and differently sized rows work.
use crate::zoom::px as zpx;
use gpui_kit::*;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

const HOVER_DURATION: Duration = Duration::from_millis(90);

#[derive(Clone, Default)]
pub struct HoverMotion(Rc<RefCell<Motion>>);
#[derive(Default)]
struct Motion {
    bounds: HashMap<String, Bounds<Pixels>>,
    current: Option<String>,
    from: f32,
    to: f32,
    height: f32,
    started: Option<Instant>,
    generation: usize,
    keys: Option<Subscription>,
    tints: HashMap<String, Rgba>,
    paint_y: f32,
    bases: HashMap<String, Rgba>,
    presentation: HashMap<String, RowPresentation>,
    divider: Option<Bounds<Pixels>>,
}
struct RowPresentation {
    visible: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    opacity: f32,
}

impl Motion {
    fn y(&self) -> f32 {
        let t = self.started.map_or(1.0, |s| {
            (s.elapsed().as_secs_f32() / HOVER_DURATION.as_secs_f32()).min(1.0)
        });
        self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3))
    }
}
impl HoverMotion {
    pub fn is_hovered(&self, key: &str) -> bool {
        self.0.borrow().current.as_deref() == Some(key)
    }

    pub fn menu(cx: &mut App) -> Self {
        let motion = Self::default();
        let weak = Rc::downgrade(&motion.0);
        let subscription = cx.observe_keystrokes(move |event, window, cx| {
            let direction = match event.keystroke.key.as_str() {
                "down" => 1,
                "up" => -1,
                _ => return,
            };
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let target = {
                let state = shared.borrow();
                let mut rows = state.bounds.iter().collect::<Vec<_>>();
                rows.sort_by(|(_, a), (_, b)| {
                    a.origin
                        .y
                        .partial_cmp(&b.origin.y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                if rows.is_empty() {
                    return;
                }
                let current = rows
                    .iter()
                    .position(|(key, _)| Some(key.as_str()) == state.current.as_deref());
                let index = current
                    .map_or(if direction > 0 { 0 } else { rows.len() - 1 }, |index| {
                        (index as isize + direction).rem_euclid(rows.len() as isize) as usize
                    });
                rows[index].0.clone()
            };
            Self(shared).hover(&target, true, window, cx);
        });
        motion.0.borrow_mut().keys = Some(subscription);
        motion
    }

    pub fn hover(&self, key: &str, entered: bool, window: &Window, cx: &mut App) {
        let mut state = self.0.borrow_mut();
        if !entered {
            let mouse = window.mouse_position();
            let min_y = state
                .bounds
                .values()
                .map(|b| b.origin.y.as_f32())
                .fold(f32::INFINITY, f32::min);
            let max_y = state
                .bounds
                .values()
                .map(|b| b.bottom().as_f32())
                .fold(f32::NEG_INFINITY, f32::max);
            let in_width = state
                .bounds
                .values()
                .any(|b| mouse.x >= b.left() && mouse.x <= b.right());
            if !in_width || mouse.y.as_f32() < min_y || mouse.y.as_f32() > max_y {
                state.current = None;
                state.started = None;
                cx.refresh_windows();
            }
            return;
        }
        if state.current.as_deref() == Some(key) {
            return;
        }
        let Some(bounds) = state.bounds.get(key).copied() else {
            return;
        };
        let target = bounds.origin.y.as_f32();
        state.from = if state.current.is_some() && !cx.reduce_motion() {
            state.y()
        } else {
            target
        };
        state.to = target;
        state.height = bounds.size.height.as_f32();
        state.current = Some(key.to_owned());
        state.started = Some(Instant::now());
        state.generation += 1;
        cx.refresh_windows();
    }

    pub fn clicked(&self, event: &ClickEvent) -> Option<String> {
        let state = self.0.borrow();
        if matches!(event, ClickEvent::Keyboard(_)) {
            state.current.clone()
        } else {
            state
                .bounds
                .iter()
                .find(|(_, bounds)| bounds.contains(&event.position()))
                .map(|(key, _)| key.clone())
        }
    }

    /// Measure rows without painting a separate hover shape inside each one.
    pub fn marker(&self, key: String, base: Rgba, tint: Rgba) -> Div {
        self.marker_with_opacity(key, base, tint, None)
    }

    pub fn marker_with_opacity(
        &self,
        key: String,
        base: Rgba,
        tint: Rgba,
        opacity: Option<Rc<Cell<f32>>>,
    ) -> Div {
        let capture = self.clone();
        div().absolute().inset_0().child(
            canvas(
                move |bounds, window, _| {
                    let clip = window.content_mask().bounds;
                    let visible = bounds.intersect(&clip);
                    let opacity = opacity
                        .as_ref()
                        .map_or(1., |value| value.get())
                        .clamp(0., 1.);
                    if opacity == 0.
                        || visible.size.width <= zpx(0.)
                        || visible.size.height <= zpx(0.)
                    {
                        return;
                    }
                    let mut state = capture.0.borrow_mut();
                    state.presentation.insert(
                        key.clone(),
                        RowPresentation {
                            visible,
                            clip,
                            opacity,
                        },
                    );
                    if state.current.as_deref() == Some(&key) {
                        let delta = bounds.top().as_f32() - state.to;
                        state.from += delta;
                        state.to += delta;
                        state.height = bounds.size.height.as_f32();
                    }
                    state.bounds.insert(key.clone(), bounds);
                    state.bases.insert(key.clone(), base);
                    state.tints.insert(key.clone(), tint);
                    state.paint_y = state.y();
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }

    pub fn divider(&self) -> Div {
        let motion = self.clone();
        div().h(zpx(6.)).relative().child(
            canvas(
                move |bounds, _, _| {
                    motion.0.borrow_mut().divider = Some(bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }

    /// One canvas behind the entire group. Its highlight can cross row gaps,
    /// rounded corners and separators without being clipped or painted twice.
    pub fn layer(&self, background: Option<Rgba>, divider: Rgba) -> Div {
        let capture = self.clone();
        let motion = self.clone();
        let generation = self.0.borrow().generation;
        div().absolute().inset_0().child(
            canvas(
                move |_, _, _| {
                    let mut state = capture.0.borrow_mut();
                    state.bounds.clear();
                    state.bases.clear();
                    state.presentation.clear();
                    state.tints.clear();
                    state.divider = None;
                },
                move |_, _, window, _| {
                    let mut state = motion.0.borrow_mut();
                    if state
                        .current
                        .as_ref()
                        .is_some_and(|key| !state.bounds.contains_key(key))
                    {
                        state.current = None;
                        state.started = None;
                    }
                    for (key, bounds) in &state.bounds {
                        if let Some(base) = state.bases.get(key).filter(|base| base.a > 0.)
                            && let Some(presentation) = state.presentation.get(key)
                        {
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: presentation.visible,
                                }),
                                |window| {
                                    window.paint_quad(
                                        fill(*bounds, base.opacity(presentation.opacity))
                                            .corner_radii(zpx(6.)),
                                    );
                                },
                            );
                        }
                    }
                    if let Some(bounds) = state.divider {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.left(), bounds.center().y),
                                size(bounds.size.width, zpx(1.)),
                            ),
                            divider,
                        ));
                    }
                    if let Some((bounds, tint, presentation)) =
                        state.current.as_ref().and_then(|key| {
                            Some((
                                state.bounds.get(key)?,
                                state.tints.get(key)?,
                                state.presentation.get(key)?,
                            ))
                        })
                    {
                        let tint = if let Some(base) = background {
                            Rgba {
                                r: base.r + (tint.r - base.r) * tint.a,
                                g: base.g + (tint.g - base.g) * tint.a,
                                b: base.b + (tint.b - base.b) * tint.a,
                                a: 1.,
                            }
                        } else {
                            *tint
                        };
                        let highlight = Bounds::new(
                            point(bounds.left(), gpui::px(state.paint_y)),
                            size(bounds.size.width, gpui::px(state.height)),
                        );
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: presentation.clip,
                            }),
                            |window| {
                                window.paint_quad(
                                    fill(highlight, tint.opacity(presentation.opacity))
                                        .corner_radii(zpx(6.)),
                                );
                            },
                        );
                    }
                },
            )
            .absolute()
            .size_full()
            .with_animation(
                ("group-hover", generation),
                Animation::new(HOVER_DURATION),
                |canvas, _| canvas,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::HoverMotion;
    use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgba};
    use std::{cell::Cell, rc::Rc};

    struct FadingRows {
        motion: HoverMotion,
        opacity: Rc<Cell<f32>>,
        height: Rc<Cell<f32>>,
    }

    impl Render for FadingRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(200.))
                .h(px(40.))
                .child(self.motion.layer(None, rgba(0)))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(self.height.get()))
                        .opacity(self.opacity.get())
                        .overflow_hidden()
                        .child(div().relative().w_full().h(px(28.)).child(
                            self.motion.marker_with_opacity(
                                "row".into(),
                                rgba(0xffffff14),
                                rgba(0xffffff0a),
                                Some(self.opacity.clone()),
                            ),
                        )),
                )
        }
    }

    #[gpui::test]
    fn collapsing_rows_fade_clip_and_clear_hover(cx: &mut gpui::TestAppContext) {
        let motion = HoverMotion::default();
        let opacity = Rc::new(Cell::new(1.));
        let height = Rc::new(Cell::new(40.));
        let fixture = FadingRows {
            motion: motion.clone(),
            opacity: opacity.clone(),
            height: height.clone(),
        };
        let (_, cx) = cx.add_window_view(move |_, _| fixture);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            motion.hover("row", true, window, cx);
        });
        opacity.set(0.3);
        height.set(14.);
        cx.update(|window, cx| {
            cx.refresh_windows();
            window.draw(cx).clear(cx);
        });
        {
            let state = motion.0.borrow();
            let row = &state.presentation["row"];
            assert_eq!(row.opacity, 0.3);
            assert_eq!(row.visible.size.height, px(14.));
            assert_eq!(row.clip.size.height, px(14.));
        }
        opacity.set(0.);
        height.set(0.);
        cx.update(|window, cx| {
            cx.refresh_windows();
            window.draw(cx).clear(cx);
        });
        assert!(motion.0.borrow().bounds.is_empty());
        assert!(!motion.is_hovered("row"));
        opacity.set(1.);
        height.set(40.);
        cx.update(|window, cx| {
            cx.refresh_windows();
            window.draw(cx).clear(cx);
        });
        assert_eq!(motion.0.borrow().presentation["row"].opacity, 1.);
        assert!(!motion.is_hovered("row"));
    }

    struct SelectedRows(HoverMotion);
    impl Render for SelectedRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(200.))
                .h(px(40.))
                .overflow_hidden()
                .child(self.0.layer(None, rgba(0)))
                .children(
                    [("visible", 0.), ("partial", 28.), ("hidden", 56.)].map(|(key, top)| {
                        div()
                            .absolute()
                            .top(px(top))
                            .w_full()
                            .h(px(28.))
                            .child(
                                self.0
                                    .marker(key.into(), rgba(0xffffff14), rgba(0xffffff0a)),
                            )
                    }),
                )
        }
    }

    #[gpui::test]
    fn selected_rows_record_clipping_before_painting(cx: &mut gpui::TestAppContext) {
        let motion = HoverMotion::default();
        let rendered = motion.clone();
        let (_, cx) = cx.add_window_view(move |_, _| SelectedRows(rendered));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let state = motion.0.borrow();
        assert_eq!(state.presentation["visible"].visible.size.height, px(28.));
        assert_eq!(state.presentation["partial"].visible.size.height, px(12.));
        assert!(!state.bounds.contains_key("hidden"));
        assert!(
            state
                .bounds
                .keys()
                .all(|key| state.presentation.contains_key(key))
        );
    }
}
