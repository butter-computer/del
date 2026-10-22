//! Model and reasoning controls backed by the selected provider's catalog.
use crate::appearance::{rgb, rgba};
use crate::state::{AppState, Selection};
use del_agent::EffortOption;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    popover::Popover,
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

#[derive(Clone)]
struct EffortDrag;

impl Render for EffortDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.)).opacity(0.0)
    }
}

pub struct ModelPicker {
    state: Entity<AppState>,
    panel: Entity<ModelPanel>,
    trigger_bottom: Rc<Cell<Pixels>>,
}

impl ModelPicker {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| {
            this.state
                .update(cx, |state, cx| state.load_models(false, cx));
            cx.notify();
        })
        .detach();
        let trigger_bottom = Rc::new(Cell::new(Pixels::ZERO));
        let panel = cx.new(|cx| ModelPanel::new(state.clone(), trigger_bottom.clone(), cx));
        state.update(cx, |state, cx| state.load_models(false, cx));
        Self {
            state,
            panel,
            trigger_bottom,
        }
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let above = matches!(state.selection, Selection::Session(_));
        let (_, model, _) = state.effective();
        let info = state.model_info();
        let label = info
            .as_ref()
            .map(|model| model.name.clone())
            .or_else(|| model.as_deref().map(|id| id.to_string()))
            .unwrap_or_else(|| "Default".into());
        let effort = state.effort().map(|e| EffortOption::named(e).name);
        let panel = self.panel.clone();
        let on_open = self.panel.clone();
        let trigger_bottom = self.trigger_bottom.clone();
        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    trigger_bottom.set(bounds.bottom());
                }
            })
            .child(
                Popover::new("model-picker")
                    .appearance(false)
                    .anchor(if above {
                        Anchor::BottomLeft
                    } else {
                        Anchor::TopLeft
                    })
                    .offset(px(20.))
                    .trigger(
                        Button::new("model-picker-trigger")
                            .ghost()
                            .small()
                            .compact()
                            .rounded_full()
                            .text_size(px(15.))
                            .disabled(state.is_starting())
                            .accessibility_label(format!("Select model: {label}"))
                            .child(h_flex().gap_1().items_center().child(label).when_some(
                                effort,
                                |row, effort| {
                                    row.child(div().text_color(rgba(0xffffff80)).child(effort))
                                },
                            )),
                    )
                    .on_open_change(move |open, _, cx| {
                        if *open {
                            on_open.update(cx, |panel, cx| {
                                panel.models = true;
                                panel.model_scroll.set_offset(point(px(0.), px(0.)));
                                panel.state.update(cx, |s, cx| s.load_models(false, cx));
                                cx.notify();
                            });
                        }
                    })
                    .content(move |_, _, _| panel.clone()),
            )
    }
}

struct ModelPanel {
    state: Entity<AppState>,
    models: bool,
    levels: Vec<EffortOption>,
    effort_index: usize,
    previous_effort_index: usize,
    trigger_bottom: Rc<Cell<Pixels>>,
    model_scroll: ScrollHandle,
}

impl ModelPanel {
    fn new(
        state: Entity<AppState>,
        trigger_bottom: Rc<Cell<Pixels>>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            models: true,
            levels: vec![],
            effort_index: 0,
            previous_effort_index: 0,
            trigger_bottom,
            model_scroll: ScrollHandle::new(),
        }
    }
}

impl Render for ModelPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let available_height = if matches!(state.selection, Selection::Session(_)) {
            self.trigger_bottom.get() - px(28.)
        } else {
            window.viewport_size().height - self.trigger_bottom.get()
        };
        let catalog = state.model_catalog();
        let (provider, selected, _) = state.effective();
        let accent = rgb(crate::composer_ui::provider_brand(provider).1);
        let info = state.model_info();
        let effort = state.effort();
        let fast = state.service_tier().is_some();
        let supports_fast = info.as_ref().is_some_and(|m| m.fast_tier.is_some());
        let locked = state.is_starting();
        let muted = rgba(0xffffff80);
        let root = v_flex()
            .w(px(278.))
            .p_3()
            .gap_3()
            .rounded_xl()
            .bg(rgb(0x222222))
            .text_size(px(13.));
        if self.models {
            let model_count = catalog
                .models
                .iter()
                .filter(|model| !(model.id == "default" && model.is_default))
                .count();
            let list_height = px((model_count + 1) as f32 * 42.)
                .min((available_height - px(84.)).clamp(px(60.), px(240.)));
            let scroll = self.model_scroll.clone();
            let mut list = v_flex().gap_0p5().child(
                Button::new("model-default")
                    .accessibility_label("Default model")
                    .ghost()
                    .w_full()
                    .h_auto()
                    .flex_none()
                    .py_1p5()
                    .text_size(px(12.))
                    .justify_start()
                    .disabled(locked)
                    .when(
                        selected.is_none() || selected.as_deref() == Some("default"),
                        |b| b.icon(IconName::Check),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .gap_0p5()
                            .child("Default")
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(muted)
                                    .child("Use the provider’s recommended model"),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.set_model(None, cx));
                        this.models = false;
                        cx.notify();
                    })),
            );
            for (i, model) in catalog.models.iter().enumerate() {
                if model.id == "default" && model.is_default {
                    continue;
                }
                let id = model.id.clone();
                list = list.child(
                    Button::new(("model-choice", i))
                        .accessibility_label(model.name.clone())
                        .ghost()
                        .w_full()
                        .h_auto()
                        .flex_none()
                        .py_1p5()
                        .text_size(px(12.))
                        .justify_start()
                        .disabled(locked)
                        .when(selected.as_deref() == Some(&id), |b| {
                            b.icon(IconName::Check)
                        })
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .items_start()
                                .gap_0p5()
                                .child(model.name.clone())
                                .when(!model.description.is_empty(), |col| {
                                    col.child(
                                        div()
                                            .text_size(px(10.5))
                                            .max_w(px(250.))
                                            .truncate()
                                            .text_color(muted)
                                            .child(model.description.clone()),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |s, cx| s.set_model(Some(id.clone()), cx));
                            this.models = false;
                            cx.notify();
                        })),
                );
            }
            return root
                .p_2p5()
                .gap_2()
                .text_size(px(12.))
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(div().text_color(muted).child("Select model"))
                        .child(
                            Button::new("refresh-models")
                                .ghost()
                                .small()
                                .label(if catalog.loading {
                                    "Refreshing…"
                                } else {
                                    "Refresh"
                                })
                                .disabled(catalog.loading)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |s, cx| s.load_models(true, cx))
                                })),
                        ),
                )
                .when(catalog.loading, |col| {
                    col.child(div().text_color(muted).child("Loading available models…"))
                })
                .when_some(catalog.error, |col, error| {
                    col.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xe9a285))
                            .child(error),
                    )
                })
                .child(
                    div()
                        .relative()
                        .h(list_height)
                        .flex_none()
                        .overflow_hidden()
                        .child(
                            div()
                                .id("model-list")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.model_scroll)
                                .child(list),
                        )
                        .child(
                            canvas(
                                |bounds, _, _| bounds,
                                move |_, bounds, window, _| {
                                    // Read after layout so the fade is accurate on the
                                    // first frame, after refresh, and at the scroll end.
                                    if scroll.max_offset().y + scroll.offset().y > px(1.) {
                                        window.paint_quad(fill(
                                            bounds,
                                            linear_gradient(
                                                0.,
                                                linear_color_stop(rgb(0x222222), 0.),
                                                linear_color_stop(rgba(0x22222200), 1.),
                                            ),
                                        ));
                                    }
                                },
                            )
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(18.)),
                        ),
                )
                .into_any_element();
        }
        self.levels = info.as_ref().map(|m| m.efforts.clone()).unwrap_or_default();
        let effective_effort = effort
            .clone()
            .or_else(|| info.as_ref().and_then(|m| m.default_effort.clone()));
        let index = effective_effort
            .as_ref()
            .and_then(|e| self.levels.iter().position(|v| &v.id == e))
            .unwrap_or(0);
        let max = self.levels.len().saturating_sub(1).max(1) as f32;
        if self.effort_index != index {
            self.previous_effort_index = self.effort_index;
            self.effort_index = index;
        }
        let effort_label = effective_effort
            .as_ref()
            .and_then(|id| self.levels.iter().find(|e| &e.id == id))
            .map(|e| e.name.clone());
        let model_name = info
            .as_ref()
            .map(|m| m.name.clone())
            .or(selected)
            .unwrap_or_else(|| "Select model".into());
        root.child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    Button::new("fast-mode")
                        .ghost()
                        .size(px(30.))
                        .flex_none()
                        .p_0()
                        .disabled(locked || !supports_fast)
                        .accessibility_label(if fast {
                            "Fast mode: on"
                        } else {
                            "Fast mode: off"
                        })
                        .tooltip(if !supports_fast {
                            "Fast mode is unavailable for this model"
                        } else if fast {
                            "Turn off Fast mode"
                        } else {
                            "Fast mode · increased usage"
                        })
                        .child(
                            Icon::default()
                                .path("icons/zap.svg")
                                .with_size(px(15.))
                                .text_color(if fast { accent } else { rgba(0xffffff70) }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.set_fast_mode(!fast, cx));
                        })),
                )
                .child(
                    Button::new("choose-another-model")
                        .ghost()
                        .small()
                        .compact()
                        .text_size(px(16.))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(model_name)
                                .when_some(effort_label, |row, effort| {
                                    row.child(div().text_color(rgba(0xffffff72)).child(effort))
                                })
                                .child(
                                    Icon::new(IconName::ChevronRight)
                                        .xsmall()
                                        .text_color(rgba(0xffffff62)),
                                ),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.models = true;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("reset-effort")
                        .ghost()
                        .size(px(30.))
                        .flex_none()
                        .p_0()
                        .accessibility_label("Reset model settings and refresh")
                        .tooltip("Reset model settings and refresh")
                        .text_color(rgba(0xffffff70))
                        .disabled(locked || catalog.loading)
                        .child(
                            Icon::new(IconName::RefreshCw)
                                .with_size(px(15.))
                                .text_color(rgba(0xffffff70)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.reset_model_settings(cx))
                        })),
                ),
        )
        .when(!self.levels.is_empty(), |col| {
            const TRACK_WIDTH: f32 = 254.0;
            const THUMB_SIZE: f32 = 28.0;
            const TRACK_INSET: f32 = THUMB_SIZE / 2.0;
            let travel = TRACK_WIDTH - THUMB_SIZE;
            let position = |value: usize| travel * value as f32 / max;
            let old_position = position(self.previous_effort_index.min(self.levels.len() - 1));
            let new_position = position(self.effort_index);
            let old_fill = old_position + TRACK_INSET;
            let new_fill = new_position + TRACK_INSET;
            let stops = self.levels.len();

            col.child(
                div()
                    .id("effort-track")
                    .relative()
                    .w(px(TRACK_WIDTH))
                    .h(px(32.))
                    .cursor_grab()
                    .on_drag(EffortDrag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_drag_move(
                        cx.listener(|this, event: &DragMoveEvent<EffortDrag>, _, cx| {
                            let stops = this.levels.len().saturating_sub(1);
                            if stops == 0 {
                                return;
                            }
                            let x = (event.event.position.x - event.bounds.origin.x)
                                .as_f32()
                                .clamp(0.0, TRACK_WIDTH);
                            let stop = (((x - TRACK_INSET) / (TRACK_WIDTH - THUMB_SIZE))
                                * stops as f32)
                                .round()
                                .clamp(0.0, stops as f32)
                                as usize;
                            let effort = this.levels.get(stop).map(|level| level.id.clone());
                            this.state
                                .update(cx, |state, cx| state.set_effort(effort, cx));
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(4.))
                            .w_full()
                            .h(px(24.))
                            .rounded(px(12.))
                            .overflow_hidden()
                            .bg(rgb(0x2a2a2d))
                            .child(
                                div().h_full().rounded(px(12.)).bg(accent).with_animation(
                                    SharedString::from(format!(
                                        "effort-fill-{old_fill}-{new_fill}"
                                    )),
                                    Animation::new(std::time::Duration::from_millis(180))
                                        .with_easing(ease_out_quint()),
                                    move |fill, t| fill.w(px(old_fill + (new_fill - old_fill) * t)),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .absolute()
                            .left(px(TRACK_INSET))
                            .right(px(TRACK_INSET))
                            .top_0()
                            .bottom_0()
                            .items_center()
                            .justify_between()
                            .children(
                                (0..stops).map(|_| {
                                    div().size(px(5.)).rounded_full().bg(rgba(0xffffff40))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .size(px(THUMB_SIZE))
                            .rounded_full()
                            .bg(rgb(0xf4f4f4))
                            .shadow_md()
                            .with_animation(
                                SharedString::from(format!(
                                    "effort-thumb-{old_position}-{new_position}"
                                )),
                                Animation::new(std::time::Duration::from_millis(180))
                                    .with_easing(ease_out_quint()),
                                move |thumb, t| {
                                    thumb.left(px(old_position + (new_position - old_position) * t))
                                },
                            ),
                    )
                    .children((0..stops).map(|stop| {
                        let effort = self.levels.get(stop).map(|level| level.id.clone());
                        div()
                            .id(("effort-stop", stop))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(stop as f32 / stops as f32))
                            .w(relative(1.0 / stops as f32))
                            .cursor_pointer()
                            .on_drag(EffortDrag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.set_effort(effort.clone(), cx))
                            }))
                    })),
            )
        })
        .when(self.levels.is_empty(), |col| {
            col.child(
                div()
                    .text_color(muted)
                    .child("This model does not advertise an effort setting."),
            )
        })
        .into_any_element()
    }
}
