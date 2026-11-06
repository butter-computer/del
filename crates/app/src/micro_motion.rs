//! Small, interruptible feedback on controls. Only glyphs move; hit targets stay put.
use crate::animated_icons::Glyph;
use crate::zoom::px as zpx;
use gpui_kit::base::StyledExt;
use gpui_kit::base::{Transition, transition};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

#[derive(Default)]
struct PointerState {
    hovered: bool,
    pressed: bool,
}

#[derive(Clone, Copy)]
pub struct Feedback {
    hover: f32,
    press: f32,
    /// The same timelines for colour, which Reduce Motion keeps (snapped)
    /// while it holds glyphs still.
    fill_hover: f32,
    fill_press: f32,
}

impl Feedback {
    pub fn glyph(self, kind: Glyph) -> crate::animated_icons::AnimatedIcon {
        crate::animated_icons::AnimatedIcon::new(kind, self.hover, self.press)
    }

    /// A background that eases in with the glyph's hover and deepens on press,
    /// instead of a snapped `.hover(|s| s.bg(..))`. Opt in by building the
    /// control with [`control`] and setting `.bg(feedback.fill(hover, pressed))`
    /// on it; typically `ui::row_hover()` and `ui::row_pressed()`. At rest it is
    /// transparent. Under Reduce Motion the fill still shows, without easing.
    #[allow(dead_code)] // Opt-in: hand-built controls adopt it one by one.
    pub fn fill(self, hover: impl Into<Rgba>, pressed: impl Into<Rgba>) -> Rgba {
        let (hover, pressed) = (hover.into(), pressed.into());
        let rest = Rgba { a: 0., ..hover };
        let mix = |a: Rgba, b: Rgba, t: f32| Rgba {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        };
        mix(
            mix(rest, hover, self.fill_hover.clamp(0., 1.)),
            pressed,
            self.fill_press.clamp(0., 1.),
        )
    }
}

/// Add feedback after configuring a control's behavior. The existing element ID
/// keeps repeated controls independent, including rows in virtual lists.
pub trait MotionIconExt:
    IntoElement + InteractiveElement + ParentElement + Sized + 'static
{
    #[track_caller]
    fn motion_child<E: IntoElement>(
        mut self,
        enabled: bool,
        build: impl FnOnce(Feedback) -> E + 'static,
    ) -> impl IntoElement {
        let id = self
            .interactivity()
            .element_id
            .clone()
            .unwrap_or_else(|| ElementId::CodeLocation(*std::panic::Location::caller()));
        control((id, "icon-motion"), enabled, move |feedback| {
            self.child(build(feedback))
        })
    }
}
impl<T: IntoElement + InteractiveElement + ParentElement + 'static> MotionIconExt for T {}

#[derive(IntoElement)]
struct Control<E: IntoElement + InteractiveElement + 'static, F: FnOnce(Feedback) -> E + 'static> {
    id: ElementId,
    enabled: bool,
    build: F,
}

/// Keep the original control, layout, tooltip and click handler intact.
pub fn control<E: IntoElement + InteractiveElement + 'static>(
    id: impl Into<ElementId>,
    enabled: bool,
    build: impl FnOnce(Feedback) -> E + 'static,
) -> impl IntoElement {
    Control {
        id: id.into(),
        enabled,
        build,
    }
}

impl<E, F> RenderOnce for Control<E, F>
where
    E: IntoElement + InteractiveElement + 'static,
    F: FnOnce(Feedback) -> E + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        window.with_element_namespace(self.id, |window| {
            let state = window.use_keyed_state("pointer", cx, |_, _| PointerState::default());
            let pointer = state.read(cx);
            let hovered = self.enabled && pointer.hovered;
            let pressed = self.enabled && pointer.pressed;
            let mut feedback = Feedback {
                fill_hover: 0.,
                fill_press: 0.,
                hover: transition(
                    "hover",
                    if hovered { 1. } else { 0. },
                    Transition::new(Duration::from_millis(160)),
                    window,
                    cx,
                ),
                press: transition(
                    "press",
                    if pressed { 1. } else { 0. },
                    Transition::new(Duration::from_millis(90)),
                    window,
                    cx,
                ),
            };
            // Reduce Motion keeps color/confirmation feedback, without moving glyphs.
            // (`transition` already settles instantly then, so fills just snap.)
            feedback.fill_hover = feedback.hover;
            feedback.fill_press = feedback.press;
            if cx.reduce_motion() {
                feedback.hover = 0.;
                feedback.press = 0.;
            }
            let mut element = (self.build)(feedback);
            if self.enabled {
                let down = state.clone();
                let up = state.clone();
                element = element
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        down.update(cx, |state, cx| {
                            state.pressed = true;
                            cx.notify();
                        });
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                        up.update(cx, |state, cx| {
                            if state.pressed {
                                state.pressed = false;
                                cx.notify();
                            }
                        });
                    })
                    .on_mouse_up_out(MouseButton::Left, {
                        let state = state.clone();
                        move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                if state.pressed {
                                    state.pressed = false;
                                    cx.notify();
                                }
                            });
                        }
                    });
            } else {
                state.update(cx, |state, _| *state = PointerState::default());
            }
            HoverSurface {
                element: element.into_any_element(),
                state,
                enabled: self.enabled,
            }
        })
    }
}

// Forward the child's layout unchanged. A separate transparent hitbox lets motion
// observe hover without replacing the control's tooltip/own hover listener.
struct HoverSurface {
    element: AnyElement,
    state: Entity<PointerState>,
    enabled: bool,
}

impl IntoElement for HoverSurface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn set_hover(state: &Entity<PointerState>, hovered: bool, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.hovered != hovered {
            state.hovered = hovered;
            if !hovered {
                state.pressed = false;
            }
            cx.notify();
        }
    });
}

impl Element for HoverSurface {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.element.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.element.prepaint(window, cx);
        hitbox
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
        if !self.enabled {
            return;
        }
        let hovered = hitbox.is_hovered(window) && !cx.has_active_drag();
        if self.state.read(cx).hovered != hovered {
            let state = self.state.clone();
            window.defer(cx, move |_, cx| set_hover(&state, hovered, cx));
        }
        let state = self.state.clone();
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                set_hover(
                    &state,
                    hitbox.is_hovered(window) && !cx.has_active_drag(),
                    cx,
                );
            }
        });
        let state = self.state.clone();
        window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
            if phase.bubble() {
                set_hover(&state, false, cx);
            }
        });
    }
}

#[derive(IntoElement)]
pub struct MorphIcon {
    id: ElementId,
    kind: crate::animated_icons::Morph,
    active: bool,
    size: Pixels,
    style: StyleRefinement,
}
impl MorphIcon {
    pub fn new(id: impl Into<ElementId>, kind: crate::animated_icons::Morph, active: bool) -> Self {
        Self {
            id: id.into(),
            kind,
            active,
            size: zpx(crate::icons::GLYPH_SIZE),
            style: StyleRefinement::default(),
        }
    }
    pub fn with_size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}
impl Styled for MorphIcon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
impl RenderOnce for MorphIcon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let progress = transition(
            self.id,
            if self.active { 1. } else { 0. },
            Transition::new(Duration::from_millis(180)),
            window,
            cx,
        );
        crate::animated_icons::AnimatedIcon::morph(self.kind, progress)
            .with_size(self.size)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::gpui;
    use core::prelude::v1::test;
    use gpui_kit::component::{
        Disableable,
        button::{Button, ButtonVariants},
    };
    use std::{cell::Cell, rc::Rc};

    struct Fixture {
        enabled: bool,
        feedback: Rc<Cell<Feedback>>,
        clicks: Rc<Cell<usize>>,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let feedback = self.feedback.clone();
            let clicks = self.clicks.clone();
            let enabled = self.enabled;
            div().size(px(200.)).child(
                Button::new("test-button")
                    .ghost()
                    .tooltip("Motion and tooltip coexist")
                    .disabled(!enabled)
                    .debug_selector(|| "motion-target".into())
                    .size(px(40.))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .motion_child(enabled, move |motion| {
                        feedback.set(motion);
                        motion.glyph(Glyph::Settings)
                    }),
            )
        }
    }

    #[gpui::test]
    fn hover_press_and_reversal_preserve_click_target(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let feedback = Rc::new(Cell::new(Feedback {
            hover: 0.,
            press: 0.,
            fill_hover: 0.,
            fill_press: 0.,
        }));
        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| Fixture {
            enabled: true,
            feedback: feedback.clone(),
            clicks: clicks.clone(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx.debug_bounds("motion-target").unwrap();
        let target = bounds.center();
        cx.simulate_mouse_move(target, None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor().advance_clock(Duration::from_millis(80));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let halfway = feedback.get().hover;
        assert!(halfway > 0. && halfway < 1.);
        let fill = feedback.get().fill(rgba(0xffffff0a), rgba(0xffffff1a));
        assert!(
            fill.a > 0. && fill.a < rgba(0xffffff0a).a,
            "the fill eases with the glyph"
        );
        assert_eq!(bounds, cx.debug_bounds("motion-target").unwrap());

        let outside = point(px(150.), px(150.));
        cx.simulate_mouse_move(outside, None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!((feedback.get().hover - halfway).abs() < 0.001);
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(feedback.get().hover, 0.);

        cx.simulate_mouse_move(target, None, Modifiers::default());
        cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(feedback.get().press, 1.);
        assert_eq!(bounds, cx.debug_bounds("motion-target").unwrap());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        assert_eq!(clicks.get(), 1);

        cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(feedback.get().press, 0.);
        assert_eq!(clicks.get(), 1, "a cancelled press must not click");
    }

    #[gpui::test]
    fn disabled_and_reduced_motion_controls_stay_still(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let feedback = Rc::new(Cell::new(Feedback {
            hover: 0.,
            press: 0.,
            fill_hover: 0.,
            fill_press: 0.,
        }));
        let clicks = Rc::new(Cell::new(0));
        let (root, cx) = cx.add_window_view(|_, _| Fixture {
            enabled: false,
            feedback: feedback.clone(),
            clicks: clicks.clone(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("motion-target").unwrap().center();
        for enabled in [false, true] {
            cx.update(|window, cx| {
                cx.set_reduce_motion(enabled);
                root.update(cx, |root, cx| {
                    root.enabled = enabled;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            cx.simulate_mouse_move(target, None, Modifiers::default());
            cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(feedback.get().hover, 0.);
            assert_eq!(feedback.get().press, 0.);
            // Reduce Motion keeps the colour feedback, snapped; disabled has none.
            assert_eq!(feedback.get().fill_press, if enabled { 1. } else { 0. });
            cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            assert_eq!(clicks.get(), usize::from(enabled));
        }
    }
}
