//! Keyboard reach for hand-built controls, matching what the shared Button gets.

use crate::zoom::px as zpx;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub trait KeyboardControl {
    /// Tab can reach this control and Enter or Space clicks it, with a ring shown
    /// only for keyboard focus. A pointer press leaves focus where it was, so
    /// clicking a disclosure or a title-bar button does not take it from the composer.
    fn keyboard_control(self) -> Self;
}

impl KeyboardControl for Stateful<Div> {
    fn keyboard_control(self) -> Self {
        self.focusable()
            .tab_stop(true)
            .focus_visible(|style| {
                style.shadow(vec![BoxShadow {
                    color: crate::appearance::ActionTone::Accent.foreground().into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: zpx(1.5),
                    inset: true,
                }])
            })
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
    }
}
