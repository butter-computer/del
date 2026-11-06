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

/// Sliding between rows of the same group.
const HOVER_DURATION: Duration = Duration::from_millis(90);
/// The highlight fades in when the pointer first enters the group...
const FADE_IN: Duration = Duration::from_millis(100);
/// ...and fades out in place when the pointer leaves it.
const FADE_OUT: Duration = Duration::from_millis(120);

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
    /// Views that paint this group's layer. Hover changes redraw only them.
    owners: std::collections::HashSet<EntityId>,
    /// Highlight opacity, eased separately from its position.
    alpha_from: f32,
    alpha_to: f32,
    alpha_started: Option<Instant>,
    alpha_duration: Duration,
    /// Where the highlight was last painted, so it can fade out in place after
    /// the pointer leaves.
    shown: Option<Shown>,
}
#[derive(Clone, Copy)]
struct Shown {
    left: Pixels,
    width: Pixels,
    tint: Rgba,
    clip: Bounds<Pixels>,
    opacity: f32,
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

    fn alpha(&self) -> f32 {
        let t = self.alpha_started.map_or(1.0, |s| {
            (s.elapsed().as_secs_f32() / self.alpha_duration.as_secs_f32().max(0.001)).min(1.0)
        });
        self.alpha_from + (self.alpha_to - self.alpha_from) * crate::ui::ease_out(t)
    }

    /// Ease the highlight's opacity toward `target` from where it is now.
    fn fade_to(&mut self, target: f32, duration: Duration, reduce_motion: bool) {
        if reduce_motion {
            self.alpha_from = target;
            self.alpha_started = None;
            if target == 0. {
                self.shown = None;
            }
        } else {
            self.alpha_from = self.alpha();
            self.alpha_started = Some(Instant::now());
        }
        self.alpha_to = target;
        self.alpha_duration = duration;
    }
}
impl HoverMotion {
    /// Redraw the views showing this group; every window only before the first paint.
    fn refresh(&self, cx: &mut App) {
        let owners: Vec<_> = self.0.borrow().owners.iter().copied().collect();
        if owners.is_empty() {
            cx.refresh_windows();
        }
        for owner in owners {
            cx.notify(owner);
        }
    }

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
            // Rows can be separated by project headers, drafts, or clipped gaps.
            // Being inside the group's overall rectangle does not mean a row is
            // still hovered. Keep the moving highlight only over a visible row.
            let over_row = state
                .presentation
                .values()
                .any(|row| row.visible.contains(&mouse));
            if !over_row && state.current.is_some() {
                state.current = None;
                // Fade out in place; a slide still under way keeps easing.
                state.fade_to(0., FADE_OUT, cx.reduce_motion());
                state.generation += 1;
                drop(state);
                self.refresh(cx);
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
        let reduce_motion = cx.reduce_motion();
        // A highlight that is still visible (moving between rows, or fading out
        // after a brief exit) slides from where it is. A fresh one fades in on
        // its row instead of popping in.
        let visible = state.current.is_some() || (state.shown.is_some() && state.alpha() > 0.);
        state.from = if visible && !reduce_motion {
            state.y()
        } else {
            target
        };
        if state.current.is_none() || state.alpha_to < 1. {
            state.fade_to(1., FADE_IN, reduce_motion);
        }
        state.to = target;
        state.height = bounds.size.height.as_f32();
        state.current = Some(key.to_owned());
        state.started = Some(Instant::now());
        state.generation += 1;
        drop(state);
        self.refresh(cx);
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
                move |_, window, _| {
                    let mut state = capture.0.borrow_mut();
                    state.owners.insert(window.current_view());
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
                        // The row itself went away (a menu closed, a list
                        // changed): there is nothing left to fade over.
                        state.current = None;
                        state.started = None;
                        state.shown = None;
                        state.alpha_from = 0.;
                        state.alpha_to = 0.;
                        state.alpha_started = None;
                    }
                    let alpha = state.alpha();
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
                    let current = state.current.as_ref().and_then(|key| {
                        let presentation = state.presentation.get(key)?;
                        Some(Shown {
                            left: state.bounds.get(key)?.left(),
                            width: state.bounds.get(key)?.size.width,
                            tint: *state.tints.get(key)?,
                            clip: presentation.clip,
                            opacity: presentation.opacity,
                        })
                    });
                    if current.is_some() {
                        state.shown = current;
                    }
                    if let Some(shown) = state.shown.filter(|_| alpha > 0.) {
                        let tint = if let Some(base) = background {
                            Rgba {
                                r: base.r + (shown.tint.r - base.r) * shown.tint.a,
                                g: base.g + (shown.tint.g - base.g) * shown.tint.a,
                                b: base.b + (shown.tint.b - base.b) * shown.tint.a,
                                a: 1.,
                            }
                        } else {
                            shown.tint
                        };
                        let highlight = Bounds::new(
                            point(shown.left, gpui::px(state.paint_y)),
                            size(shown.width, gpui::px(state.height)),
                        );
                        window.with_content_mask(
                            Some(ContentMask { bounds: shown.clip }),
                            |window| {
                                window.paint_quad(
                                    fill(highlight, tint.opacity(shown.opacity * alpha))
                                        .corner_radii(zpx(6.)),
                                );
                            },
                        );
                    } else if state.current.is_none() {
                        state.shown = None;
                    }
                },
            )
            .absolute()
            .size_full()
            .with_animation(
                ("group-hover", generation),
                // Frames for the slower of the slide and the fades.
                Animation::new(FADE_OUT.max(FADE_IN).max(HOVER_DURATION)),
                |canvas, _| canvas,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::HoverMotion;
    use gpui::{
        Context, InteractiveElement, IntoElement, ParentElement, Render,
        StatefulInteractiveElement, Styled, Window, div, px, rgba,
    };
    use gpui_kit::prelude::FluentBuilder;
    use std::{cell::Cell, rc::Rc};

    struct SidebarRows(HoverMotion);

    impl Render for SidebarRows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(200.))
                .h(px(120.))
                .child(self.0.layer(None, rgba(0)))
                .children(
                    [
                        ("pinned", 0., true),
                        ("project", 32., false),
                        ("session", 64., true),
                    ]
                    .map(|(key, top, tracked)| {
                        let motion = self.0.clone();
                        div()
                            .id(key)
                            .debug_selector(move || key.into())
                            .absolute()
                            .top(px(top))
                            .w_full()
                            .h(px(28.))
                            .when(tracked, |row| {
                                row.on_hover(move |entered, window, cx| {
                                    motion.hover(key, *entered, window, cx);
                                })
                                .child(self.0.marker(
                                    key.into(),
                                    rgba(0),
                                    rgba(0xffffff0a),
                                ))
                            })
                            .when(!tracked, |row| row.hover(|row| row.bg(rgba(0xffffff0a))))
                    }),
                )
        }
    }

    #[gpui::test]
    fn moving_from_session_to_project_clears_shared_hover(cx: &mut gpui::TestAppContext) {
        let motion = HoverMotion::default();
        let rendered = motion.clone();
        let (_, cx) = cx.add_window_view(move |_, _| SidebarRows(rendered));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let session = cx.debug_bounds("session").unwrap().center();
        let project = cx.debug_bounds("project").unwrap().center();
        let pinned = cx.debug_bounds("pinned").unwrap().center();
        for _ in 0..5 {
            cx.simulate_mouse_move(session, None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(motion.is_hovered("session"));
            cx.simulate_mouse_move(project, None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(
                !motion.is_hovered("session"),
                "session remains highlighted over project header"
            );
            cx.simulate_mouse_move(pinned, None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(motion.is_hovered("pinned"));
        }
    }

    #[gpui::test]
    fn highlight_fades_in_and_out_instead_of_popping(cx: &mut gpui::TestAppContext) {
        let motion = HoverMotion::default();
        let rendered = motion.clone();
        let (_, cx) = cx.add_window_view(move |_, _| SidebarRows(rendered));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let session = cx.debug_bounds("session").unwrap().center();
        let outside = gpui::point(px(100.), px(110.));
        // Start with the pointer off every row.
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        motion.0.borrow_mut().shown = None;
        for reduce in [false, true] {
            cx.update(|_, cx| cx.set_reduce_motion(reduce));
            cx.simulate_mouse_move(session, None, gpui::Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            {
                let state = motion.0.borrow();
                assert_eq!(state.alpha_to, 1.);
                assert_eq!(state.from, state.to, "a fresh highlight starts on its row");
                assert!(state.shown.is_some());
                assert_eq!(state.alpha() < 1., !reduce, "only Reduce Motion pops in");
            }
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(!motion.is_hovered("session"));
            let state = motion.0.borrow();
            assert_eq!(state.alpha_to, 0.);
            assert_eq!(
                state.shown.is_some(),
                !reduce,
                "the highlight fades out in place unless motion is reduced"
            );
        }
    }

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
