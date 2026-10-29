//! Routes inner-tab commands to the last pane the user interacted with.
use crate::{
    right_panel::{Pane, RightPanel},
    workspace_tabs::WorkspaceTabs,
};
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Zone {
    #[default]
    Main,
    Right,
}

pub(crate) fn track(div: Div, active: Rc<Cell<Zone>>, zone: Zone) -> Div {
    let released = active.clone();
    div.capture_any_mouse_down(move |_, _, _| active.set(zone))
        .capture_any_mouse_up(move |_, _, _| released.set(zone))
}

impl Zone {
    pub fn new_tab(
        self,
        main: &Entity<WorkspaceTabs>,
        right: &Entity<RightPanel>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Main => main.update(cx, |tabs, cx| tabs.new_tab(window, cx)),
            Self::Right => right.update(cx, |tabs, cx| tabs.open_new_tab(window, cx)),
        }
    }
    pub fn close_tab(
        self,
        main: &Entity<WorkspaceTabs>,
        right: &Entity<RightPanel>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Main => {
                main.update(cx, |tabs, cx| tabs.close_active(window, cx));
            }
            Self::Right => right.update(cx, |tabs, cx| tabs.close_active_tab(window, cx)),
        }
    }
    pub fn open_tool(
        self,
        pane: Pane,
        main: &Entity<WorkspaceTabs>,
        right: &Entity<RightPanel>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Main => main.update(cx, |tabs, cx| tabs.open_tool(pane, window, cx)),
            Self::Right => right.update(cx, |tabs, cx| tabs.choose_new_tool(pane, window, cx)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use gpui_kit::component::*;

    #[::gpui::test]
    fn repeated_keyboard_commands_follow_clicked_zone(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            active: Rc<Cell<Zone>>,
            main: Entity<WorkspaceTabs>,
            right: Entity<RightPanel>,
            focus: FocusHandle,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                h_flex()
                    .w(px(800.))
                    .h(px(500.))
                    .track_focus(&self.focus)
                    .on_action(cx.listener(|this, _: &crate::NewWorkspaceTab, window, cx| {
                        this.active
                            .get()
                            .new_tab(&this.main, &this.right, window, cx);
                    }))
                    .on_action(
                        cx.listener(|this, _: &crate::CloseWorkspaceTab, window, cx| {
                            this.active
                                .get()
                                .close_tab(&this.main, &this.right, window, cx);
                        }),
                    )
                    .on_action(cx.listener(|this, _: &crate::OpenChanges, window, cx| {
                        this.active.get().open_tool(
                            Pane::Changes,
                            &this.main,
                            &this.right,
                            window,
                            cx,
                        );
                    }))
                    .child(
                        track(div(), self.active.clone(), Zone::Main)
                            .id("main-zone")
                            .debug_selector(|| "main-zone".into())
                            .w(px(400.))
                            .h_full()
                            .child(self.main.clone()),
                    )
                    .child(
                        track(div(), self.active.clone(), Zone::Right)
                            .id("right-zone")
                            .debug_selector(|| "right-zone".into())
                            .w(px(400.))
                            .h_full()
                            .child(self.right.clone()),
                    )
            }
        }
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-t", crate::NewWorkspaceTab, None),
                KeyBinding::new("cmd-shift-w", crate::CloseWorkspaceTab, None),
                KeyBinding::new("ctrl-shift-g", crate::OpenChanges, None),
            ]);
        });
        let dir = tempfile::tempdir().unwrap();
        let mut handles = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let main = cx.new(|cx| WorkspaceTabs::new(dir.path().into(), window, cx));
            let right = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
            cx.observe(&main, |_, _, cx| cx.notify()).detach();
            cx.observe(&right, |_, _, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            handles = Some((main.clone(), right.clone()));
            Fixture {
                active: Default::default(),
                main,
                right,
                focus,
            }
        });
        let (main, right) = handles.unwrap();
        let right_blanks = |cx: &mut gpui::VisualTestContext| {
            right.read_with(cx, |right, cx| {
                serde_json::to_value(right.snapshot(true, cx)).unwrap()["blank_tabs"]
                    .as_array()
                    .unwrap()
                    .len()
            })
        };
        let main_tabs = |cx: &mut gpui::VisualTestContext| {
            main.read_with(cx, |main, cx| {
                serde_json::to_value(main.snapshot(cx)).unwrap()["tabs"]
                    .as_array()
                    .unwrap()
                    .len()
            })
        };
        cx.run_until_parked();
        let click_zone = |cx: &mut gpui::VisualTestContext, selector| {
            let p = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_mouse_move(p, None, Modifiers::default());
            cx.simulate_mouse_down(p, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(p, MouseButton::Left, Modifiers::default());
            cx.run_until_parked();
        };
        click_zone(cx, "right-zone");
        for _ in 0..12 {
            cx.simulate_keystrokes("cmd-t");
        }
        cx.run_until_parked();
        assert_eq!(right_blanks(cx), 12);
        assert_eq!(main_tabs(cx), 0);
        cx.simulate_keystrokes("cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(right_blanks(cx), 11);
        click_zone(cx, "main-zone");
        cx.simulate_keystrokes("cmd-t cmd-t cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(main_tabs(cx), 1);
        assert_eq!(right_blanks(cx), 11);
        cx.simulate_keystrokes("ctrl-shift-g");
        cx.run_until_parked();
        main.read_with(cx, |main, cx| {
            let value = serde_json::to_value(main.snapshot(cx)).unwrap();
            assert_eq!(value["tabs"][0]["kind"], "Changes");
        });
        cx.simulate_keystrokes("cmd-shift-w cmd-shift-w");
        cx.run_until_parked();
        assert_eq!(main_tabs(cx), 0);
        assert!(main.read_with(cx, |main, _| main.is_chat()));
    }
}
