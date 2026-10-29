//! Searchable starting-branch selection for new sessions.
use crate::{appearance, state::AppState, zoom::px as zpx};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    popover::Popover,
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, path::PathBuf, rc::Rc};

pub struct BranchPicker {
    state: Entity<AppState>,
    panel: Entity<BranchPanel>,
    open: bool,
    root: Option<PathBuf>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl BranchPicker {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bounds = Rc::new(Cell::new(Bounds::default()));
        let panel = cx.new(|cx| BranchPanel::new(bounds.clone(), window, cx));
        cx.observe(&state, |this, _, cx| {
            if this
                .root
                .as_ref()
                .is_some_and(|root| *root != this.state.read(cx).effective().2)
            {
                this.open = false;
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&panel, |this, _, event: &BranchChosen, cx| {
            this.open = false;
            if let Some(branch) = &event.0 {
                this.state.update(cx, |state, cx| {
                    if !state.is_starting() && this.root.as_ref() == Some(&state.effective().2) {
                        let worktree = state.draft_launch_options().worktree;
                        state.set_launch_options(Some(branch.clone()), worktree, cx);
                    }
                });
            }
            cx.notify();
        })
        .detach();
        Self {
            state,
            panel,
            open: false,
            root: None,
            bounds,
        }
    }
}
impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let root = state.effective().2;
        let info = state.project_info.get(&root).cloned().unwrap_or_default();
        let selected = state.draft_launch_options().branch.unwrap_or(info.current);
        let disabled = state.is_starting();
        let panel = self.panel.clone();
        let bounds = self.bounds.clone();
        div()
            .on_children_prepainted(move |rects, _, _| {
                if let Some(rect) = rects.first() {
                    bounds.set(*rect);
                }
            })
            .child(
                Popover::new("branch-picker")
                    .open(self.open)
                    .appearance(false)
                    .anchor(Anchor::TopLeft)
                    .offset(zpx(6.))
                    .trigger(
                        Button::new("session-branch")
                            .ghost()
                            .disabled(disabled)
                            .h(zpx(24.))
                            .px_2()
                            .min_w_0()
                            .max_w(zpx(180.))
                            .text_size(zpx(12.))
                            .accessibility_label(format!("Starting branch: {selected}"))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(format!("Branch: {selected}")),
                            ),
                    )
                    .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                        this.open = *open;
                        if *open {
                            let state = this.state.read(cx);
                            let root = state.effective().2;
                            let info = state.project_info.get(&root).cloned().unwrap_or_default();
                            let selected =
                                state.draft_launch_options().branch.unwrap_or(info.current);
                            this.root = Some(root);
                            this.panel.update(cx, |panel, cx| {
                                panel.reset(info.branches, selected, window, cx)
                            });
                        }
                        cx.notify();
                    }))
                    .content(move |_, _, _| panel.clone()),
            )
    }
}
struct BranchChosen(Option<String>);
struct BranchPanel {
    search: Entity<InputState>,
    branches: Vec<String>,
    selected: String,
    highlighted: usize,
    scroll: ScrollHandle,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl EventEmitter<BranchChosen> for BranchPanel {}
impl BranchPanel {
    fn new(bounds: Rc<Cell<Bounds<Pixels>>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search branches…"));
        cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.highlighted = 0;
                this.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
                cx.notify();
            }
        })
        .detach();
        Self {
            search,
            branches: vec![],
            selected: String::new(),
            highlighted: 0,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            bounds,
        }
    }
    fn reset(
        &mut self,
        branches: Vec<String>,
        selected: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.branches = branches;
        self.selected = selected;
        self.search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.highlighted = 0;
        self.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
        cx.notify();
    }
    fn choices(&self, cx: &App) -> Vec<String> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        self.branches
            .iter()
            .filter(|name| name.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }
}
impl Render for BranchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let choices = self.choices(cx);
        self.highlighted = self.highlighted.min(choices.len().saturating_sub(1));
        let bounds = self.bounds.get();
        let available = window.viewport_size().height - bounds.bottom();
        let height = zpx(choices.len().max(1) as f32 * 28.)
            .min((available - zpx(58.)).clamp(zpx(28.), zpx(224.)));
        let scroll = self.scroll.clone();
        v_flex()
            .id("branch-panel")
            .debug_selector(|| "branch-panel".into())
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.modifiers.platform || key.modifiers.control || key.modifiers.alt {
                    return;
                }
                let choices = this.choices(cx);
                match key.key.as_str() {
                    "escape" => cx.emit(BranchChosen(None)),
                    "enter" => {
                        if let Some(branch) = choices.get(this.highlighted) {
                            cx.emit(BranchChosen(Some(branch.clone())));
                        }
                    }
                    "up" | "down" | "pageup" | "pagedown" => {
                        let step = if key.key.starts_with("page") {
                            (this.scroll.bounds().size.height / zpx(28.))
                                .floor()
                                .max(1.) as usize
                        } else {
                            1
                        };
                        this.highlighted = if key.key == "up" || key.key == "pageup" {
                            this.highlighted.saturating_sub(step)
                        } else {
                            (this.highlighted + step).min(choices.len().saturating_sub(1))
                        };
                        if !choices.is_empty() {
                            this.scroll.scroll_to_item(this.highlighted);
                        }
                        cx.notify();
                    }
                    _ => return,
                }
                window.prevent_default();
                cx.stop_propagation();
            }))
            .w(zpx(320.).min(window.viewport_size().width - zpx(24.)))
            .p_2()
            .gap_1()
            .rounded_lg()
            .bg(appearance::popup_surface())
            .border_1()
            .border_color(appearance::separator())
            .overflow_hidden()
            .child(
                Input::new(&self.search)
                    .small()
                    .appearance(false)
                    .bordered(false)
                    .cleanable(true),
            )
            .child(
                div()
                    .relative()
                    .h(height)
                    .flex_none()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("branch-list")
                            .debug_selector(|| "branch-list".into())
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .when(choices.is_empty(), |list| {
                                list.child(
                                    div()
                                        .h(zpx(28.))
                                        .px_2()
                                        .text_size(zpx(12.))
                                        .text_color(appearance::rgba(0xffffff80))
                                        .child("No branches found"),
                                )
                            })
                            .children(choices.into_iter().enumerate().map(|(index, branch)| {
                                let checked = branch == self.selected;
                                Button::new(("branch-choice", index))
                                    .debug_selector(move || format!("branch-choice-{index}"))
                                    .ghost()
                                    .w_full()
                                    .h(zpx(28.))
                                    .flex_none()
                                    .px_2()
                                    .justify_start()
                                    .text_size(zpx(12.))
                                    .accessibility_label(branch.clone())
                                    .when(index == self.highlighted, |button| {
                                        button.bg(appearance::rgba(0xffffff14))
                                    })
                                    .child(div().w(zpx(14.)).flex_none().when(checked, |slot| {
                                        slot.child(
                                            crate::icons::icon(IconName::Check).with_size(zpx(12.)),
                                        )
                                    }))
                                    .child(
                                        div().flex_1().min_w_0().truncate().child(branch.clone()),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(BranchChosen(Some(branch.clone())))
                                    }))
                            })),
                    )
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |_, bounds, window, _| {
                                let remaining = scroll.max_offset().y + scroll.offset().y;
                                let alpha = (remaining / zpx(16.)).clamp(0., 1.);
                                if alpha > 0. {
                                    window.paint_quad(fill(
                                        bounds,
                                        linear_gradient(
                                            0.,
                                            linear_color_stop(
                                                appearance::popup_surface().opacity(alpha),
                                                0.,
                                            ),
                                            linear_color_stop(
                                                appearance::popup_surface().opacity(0.),
                                                1.,
                                            ),
                                        ),
                                    ));
                                }
                            },
                        )
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(zpx(16.)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use std::cell::RefCell;

    #[::gpui::test]
    fn search_keyboard_and_wheel_keep_large_branch_lists_bounded(cx: &mut gpui::TestAppContext) {
        struct Fixture(Entity<BranchPanel>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.clone()
            }
        }
        cx.update(gpui_kit::init);
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let panel = cx.new(|cx| BranchPanel::new(Rc::default(), window, cx));
            cx.subscribe(&panel, move |_, _, event: &BranchChosen, _| {
                observed.borrow_mut().push(event.0.clone());
            })
            .detach();
            panel.update(cx, |panel, cx| {
                let mut branches: Vec<_> =
                    (0..100).map(|i| format!("origin/feature-{i:03}")).collect();
                branches.push(format!("origin/日本語/{}", "long-".repeat(100)));
                panel.reset(branches, "origin/feature-002".into(), window, cx);
            });
            Fixture(panel)
        });
        cx.run_until_parked();
        let panel = root.read_with(cx, |root, _| root.0.clone());
        let viewport = cx.debug_bounds("branch-list").unwrap();
        assert!(viewport.size.height <= zpx(224.));
        assert!(cx.debug_bounds("branch-panel").unwrap().size.width <= zpx(320.));
        cx.simulate_mouse_move(viewport.center(), None, Modifiers::default());
        cx.simulate_event(ScrollWheelEvent {
            position: viewport.center(),
            delta: ScrollDelta::Pixels(point(Pixels::ZERO, -zpx(200.))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.scroll.offset().y < Pixels::ZERO)
        });
        cx.simulate_keystrokes("pagedown pagedown down");
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert!(panel.highlighted > 8);
            let selected = panel.scroll.bounds_for_item(panel.highlighted).unwrap();
            let viewport = panel.scroll.bounds();
            assert!(selected.top() + panel.scroll.offset().y >= viewport.top() - zpx(1.));
            assert!(selected.bottom() + panel.scroll.offset().y <= viewport.bottom() + zpx(1.));
        });
        cx.simulate_input("FEATURE-09");
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.choices(cx).len(), 10);
            assert_eq!(panel.highlighted, 0);
        });
        cx.simulate_keystrokes("down enter escape");
        cx.run_until_parked();
        assert_eq!(
            &*events.borrow(),
            &[Some("origin/feature-091".into()), None]
        );
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("no-such-branch");
        cx.run_until_parked();
        cx.simulate_keystrokes("up down pagedown enter");
        cx.run_until_parked();
        assert_eq!(events.borrow().len(), 2);
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("日本語");
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| assert_eq!(panel.choices(cx).len(), 1));
    }
}
