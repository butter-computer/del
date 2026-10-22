//! One moving highlight shared by a group of rows. Bounds are measured rather
//! than inferred, so separators, scrolling, and differently sized rows work.
use gpui_kit::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

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
}
impl Motion {
    fn y(&self) -> f32 {
        let t = self
            .started
            .map_or(1.0, |s| (s.elapsed().as_secs_f32() / 0.16).min(1.0));
        self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3))
    }
}
impl HoverMotion {
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

    pub fn background(&self, key: String, base: Rgba, tint: Rgba) -> Div {
        let capture = self.clone();
        let motion = self.clone();
        let own = Rc::new(RefCell::new(0.0));
        let measured = own.clone();
        let generation = self.0.borrow().generation;
        div()
            .absolute()
            .inset_0()
            .rounded_md()
            .overflow_hidden()
            .bg(base)
            .child(
                canvas(
                    move |bounds, _, _| {
                        *measured.borrow_mut() = bounds.origin.y.as_f32();
                        capture.0.borrow_mut().bounds.insert(key.clone(), bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .rounded_md()
                    .bg(tint)
                    .with_animation(
                        ("row-hover", generation),
                        Animation::new(Duration::from_millis(170)),
                        move |highlight, _| {
                            let state = motion.0.borrow();
                            highlight
                                .top(px(state.y() - *own.borrow()))
                                .h(px(state.height))
                                .opacity(if state.current.is_some() { 1.0 } else { 0.0 })
                        },
                    ),
            )
    }
}
