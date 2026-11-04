//! Keyboard reach for hand-built controls, matching what the shared Button gets.

use crate::zoom::px as zpx;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cell::RefCell;
use std::collections::HashMap;

pub trait KeyboardControl: Sized {
    /// Tab can reach this control and Enter or Space clicks it, with a ring shown
    /// only for keyboard focus. A pointer press leaves focus where it was, so
    /// clicking a disclosure or a title-bar button does not take it from the composer.
    fn keyboard_control(self) -> Self;
    /// The same, with a handle the owner keeps so it can reveal hover-only parts
    /// while keyboard focus is inside, through render state as hover does.
    fn keyboard_control_with(self, handle: &FocusHandle) -> Self;
}

fn ring(style: StyleRefinement) -> StyleRefinement {
    style.shadow(vec![BoxShadow {
        color: crate::appearance::ActionTone::Accent.foreground().into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: zpx(1.5),
        // A spread shadow outside the bounds paints a filled shape under a
        // transparent control, so the ring is drawn inside them.
        inset: true,
    }])
}

impl KeyboardControl for Stateful<Div> {
    fn keyboard_control(self) -> Self {
        self.focusable()
            .tab_stop(true)
            .focus_visible(ring)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
    }

    fn keyboard_control_with(self, handle: &FocusHandle) -> Self {
        self.track_focus(handle)
            .focus_visible(ring)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
    }
}

/// Focus handles for rows that outlive a single render.
#[derive(Default)]
pub struct FocusHandles(RefCell<HashMap<String, FocusHandle>>);

impl FocusHandles {
    /// The handle of a control Tab can land on.
    pub fn control(&self, key: &str, cx: &App) -> FocusHandle {
        self.get(key, true, cx)
    }

    /// The handle of a row that only groups focusable children.
    pub fn container(&self, key: &str, cx: &App) -> FocusHandle {
        self.get(key, false, cx)
    }

    fn get(&self, key: &str, tab_stop: bool, cx: &App) -> FocusHandle {
        self.0
            .borrow_mut()
            .entry(key.to_owned())
            .or_insert_with(|| cx.focus_handle().tab_stop(tab_stop))
            .clone()
    }
}

/// Keyboard focus is on this element or inside it. A pointer click also moves
/// focus to a shared Button, and that must not leave hover-only parts showing.
pub fn keyboard_within(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    window.last_input_was_keyboard() && handle.contains_focused(window, cx)
}

/// Scrolls the nearest clipped edge of a keyboard-focused control into view. Tab moves
/// focus but not the list, and a focus ring off screen tells the user nothing.
pub fn reveal_when_focused(scroll: ScrollHandle, vertical: bool) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            let view = scroll.bounds();
            let mut offset = scroll.offset();
            let (before, after) = if vertical {
                (view.top() - bounds.top(), bounds.bottom() - view.bottom())
            } else {
                (view.left() - bounds.left(), bounds.right() - view.right())
            };
            let delta = if before > px(0.) {
                before
            } else if after > px(0.) {
                -after
            } else {
                return;
            };
            if vertical {
                offset.y += delta;
            } else {
                offset.x += delta;
            }
            scroll.set_offset(offset);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// Remembers lost focus between renders; see [`restore_lost_focus`].
#[derive(Default)]
pub struct FocusWatch(RefCell<Option<Option<FocusHandle>>>);

/// Focus can end up with nothing, as a closed popup leaves it, or with an element
/// that is no longer on screen, as a launcher replaced by its tool leaves it. App
/// shortcuts dispatch from the focused element, so both go deaf to the keyboard.
/// Acts only when it is still lost on the next render, so focus given to an element
/// that is about to appear is left alone.
pub fn restore_lost_focus<T: 'static>(
    shell: &FocusHandle,
    watch: &FocusWatch,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    use gpui_kit::component::WindowExt as _;
    let lost = !shell.contains_focused(window, cx)
        && !window.has_active_dialog(cx)
        && !window.has_active_sheet(cx);
    if !lost {
        *watch.0.borrow_mut() = None;
        return;
    }
    let focused = window.focused(cx);
    let seen_before = watch.0.borrow().as_ref() == Some(&focused);
    if seen_before {
        *watch.0.borrow_mut() = None;
        let shell = shell.clone();
        cx.defer_in(window, move |_, window, cx| shell.focus(window, cx));
    } else {
        *watch.0.borrow_mut() = Some(focused);
        cx.defer_in(window, |_, _, cx| cx.notify());
    }
}

#[cfg(test)]
mod tests {
    // Not a glob of the prelude: its `test` attribute would shadow the built-in one.
    use super::{FocusWatch, restore_lost_focus};
    use ::gpui::{
        AppContext as _, Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding, Render,
        Styled, Window, div,
    };

    ::gpui::actions!(keyboard_control_tests, [Probe]);

    struct Shell {
        focus: FocusHandle,
        restore: bool,
        watch: FocusWatch,
        probes: usize,
    }
    impl Render for Shell {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.restore {
                restore_lost_focus(&self.focus, &self.watch, window, cx);
            }
            div()
                .size_full()
                .track_focus(&self.focus)
                .key_context("Shell")
                .on_action(cx.listener(|this, _: &Probe, _, _| this.probes += 1))
        }
    }

    fn probes_after_a_dialog_closes(restore: bool, cx: &mut ::gpui::TestAppContext) -> usize {
        use gpui_kit::component::WindowExt as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
            cx.bind_keys([KeyBinding::new("cmd-,", Probe, Some("Shell"))]);
        });
        let mut shell = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Shell {
                focus: cx.focus_handle(),
                restore,
                watch: FocusWatch::default(),
                probes: 0,
            });
            shell = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let shell = shell.unwrap();
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-,");
        cx.run_until_parked();
        assert_eq!(shell.read_with(cx, |s, _| s.probes), 1, "shortcut before");
        // As when a menu opens the dialog: the element focused beforehand is the
        // menu, which is gone by the time the dialog tries to hand focus back.
        cx.update(|window, cx| {
            let menu = cx.focus_handle();
            menu.focus(window, cx);
            window.open_dialog(cx, |dialog, _, _| dialog.title("Dialog"));
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.close_dialog(cx));
        cx.run_until_parked();
        shell.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-,");
        cx.run_until_parked();
        shell.read_with(cx, |s, _| s.probes - 1)
    }

    #[::gpui::test]
    fn shortcuts_keep_working_after_a_dialog_closes(cx: &mut ::gpui::TestAppContext) {
        assert_eq!(probes_after_a_dialog_closes(true, cx), 1);
    }

    /// A launcher replaced by its tool leaves focus on an element no longer on screen.
    #[::gpui::test]
    fn shortcuts_keep_working_after_the_focused_element_leaves(cx: &mut ::gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.bind_keys([KeyBinding::new("cmd-,", Probe, Some("Shell"))]);
        });
        let mut shell = None;
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Shell {
                focus: cx.focus_handle(),
                restore: true,
                watch: FocusWatch::default(),
                probes: 0,
            });
            shell = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let shell = shell.unwrap();
        let gone = cx.update(|window, cx| {
            let gone = cx.focus_handle();
            gone.focus(window, cx);
            gone
        });
        shell.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-,");
        cx.run_until_parked();
        assert_eq!(shell.read_with(cx, |s, _| s.probes), 1);
        drop(gone);
    }

    /// The control: this is the failure the fallback exists for.
    #[::gpui::test]
    fn without_the_fallback_a_closed_dialog_silences_shortcuts(cx: &mut ::gpui::TestAppContext) {
        assert_eq!(probes_after_a_dialog_closes(false, cx), 0);
    }
}
