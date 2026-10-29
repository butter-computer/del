//! Model and reasoning controls backed by the selected provider's catalog.
use crate::appearance::{rgb, rgba};
use crate::state::{AppState, Selection};
use crate::zoom::px as zpx;
use del_agent::{EffortOption, Provider};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    popover::Popover,
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

fn model_label(selected: Option<&str>, info: Option<&del_agent::ModelInfo>) -> String {
    match selected {
        None | Some("default") => "Default".into(),
        Some(id) => info
            .map(|model| model.name.clone())
            .unwrap_or_else(|| id.into()),
    }
}

fn effort_label(explicit: Option<&str>, info: Option<&del_agent::ModelInfo>) -> Option<String> {
    let id = explicit.or_else(|| info.and_then(|model| model.default_effort.as_deref()))?;
    Some(
        info.and_then(|model| model.efforts.iter().find(|effort| effort.id == id))
            .map(|effort| effort.name.clone())
            .unwrap_or_else(|| EffortOption::named(id).name),
    )
}

#[derive(Clone)]
struct EffortDrag;

impl Render for EffortDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(zpx(1.)).opacity(0.0)
    }
}

/// A shared picker presentation with a conversation-scoped settings target.
#[derive(Clone)]
enum ModelSource {
    Main(Entity<AppState>),
    Side(WeakEntity<crate::side_chat::SideChat>),
}

pub(crate) struct ModelSelection {
    pub above: bool,
    pub provider: Provider,
    pub model: Option<String>,
    pub catalog: crate::state::ModelCatalog,
    pub effort: Option<String>,
    pub tier: Option<String>,
    pub locked: bool,
}
impl ModelSelection {
    fn info(&self) -> Option<del_agent::ModelInfo> {
        self.catalog
            .models
            .iter()
            .find(|m| self.model.as_ref().map_or(m.is_default, |id| &m.id == id))
            .cloned()
    }
}

pub(crate) enum ModelChange {
    Model(Option<String>),
    Effort(Option<String>),
    Fast(bool),
    Reset,
    Refresh(bool),
}
impl ModelSource {
    fn read(&self, cx: &App) -> ModelSelection {
        match self {
            Self::Main(state) => {
                let state = state.read(cx);
                let (provider, model, _) = state.effective();
                ModelSelection {
                    above: matches!(state.selection, Selection::Session(_)),
                    provider,
                    model,
                    catalog: state.model_catalog(),
                    effort: state.effort(),
                    tier: state.service_tier(),
                    locked: state.is_starting(),
                }
            }
            Self::Side(side) => side
                .upgrade()
                .map(|side| side.read(cx).model_selection(cx))
                .unwrap_or(ModelSelection {
                    above: true,
                    provider: Provider::Codex,
                    model: None,
                    catalog: Default::default(),
                    effort: None,
                    tier: None,
                    locked: true,
                }),
        }
    }
    fn apply(&self, change: ModelChange, cx: &mut App) {
        match self {
            Self::Main(state) => state.update(cx, |state, cx| match change {
                ModelChange::Model(model) => state.set_model(model, cx),
                ModelChange::Effort(effort) => state.set_effort(effort, cx),
                ModelChange::Fast(enabled) => state.set_fast_mode(enabled, cx),
                ModelChange::Reset => state.reset_model_settings(cx),
                ModelChange::Refresh(refresh) => state.load_models(refresh, cx),
            }),
            Self::Side(side) => {
                let _ = side.update(cx, |side, cx| side.change_model(change, cx));
            }
        }
    }
    fn observe<T: 'static>(&self, cx: &mut Context<T>) {
        match self {
            Self::Main(state) => cx.observe(state, |_, _, cx| cx.notify()).detach(),
            Self::Side(side) => {
                if let Some(side) = side.upgrade() {
                    cx.observe(&side, |_, _, cx| cx.notify()).detach();
                }
            }
        }
    }
}

pub struct ModelPicker {
    state: ModelSource,
    panel: Entity<ModelPanel>,
    open: bool,
    trigger_bottom: Rc<Cell<Pixels>>,
}

impl ModelPicker {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| {
            this.state.apply(ModelChange::Refresh(false), cx);
            cx.notify();
        })
        .detach();
        state.update(cx, |state, cx| state.load_models(false, cx));
        Self::with_source(ModelSource::Main(state), window, cx)
    }

    pub fn for_side_chat(
        side: WeakEntity<crate::side_chat::SideChat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = ModelSource::Side(side);
        source.observe(cx);
        Self::with_source(source, window, cx)
    }

    fn with_source(state: ModelSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let trigger_bottom = Rc::new(Cell::new(Pixels::ZERO));
        let panel = cx.new(|cx| ModelPanel::new(state.clone(), trigger_bottom.clone(), window, cx));
        cx.subscribe(&panel, |this, _, _: &CloseModelPicker, cx| {
            this.open = false;
            cx.notify();
        })
        .detach();
        Self {
            state,
            panel,
            open: false,
            trigger_bottom,
        }
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let above = state.above;
        let model = &state.model;
        let info = state.info();
        let label = model_label(model.as_deref(), info.as_ref());
        let effort = effort_label(state.effort.as_deref(), info.as_ref());
        let panel = self.panel.clone();
        let trigger_bottom = self.trigger_bottom.clone();
        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    trigger_bottom.set(bounds.bottom());
                }
            })
            .child(
                Popover::new("model-picker")
                    .open(self.open)
                    .appearance(false)
                    .anchor(if above {
                        Anchor::BottomLeft
                    } else {
                        Anchor::TopLeft
                    })
                    .offset(zpx(20.))
                    .trigger(
                        Button::new("model-picker-trigger")
                            .ghost()
                            .small()
                            .compact()
                            .rounded_full()
                            .text_size(zpx(15.))
                            .disabled(state.locked)
                            .accessibility_label(format!("Select model: {label}"))
                            .child(h_flex().gap_1().items_center().child(label).when_some(
                                effort,
                                |row, effort| {
                                    row.child(div().text_color(rgba(0xffffff80)).child(effort))
                                },
                            )),
                    )
                    .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                        this.open = *open;
                        if *open {
                            this.panel.update(cx, |panel, cx| {
                                panel.models = true;
                                panel.highlighted = None;
                                panel.search.update(cx, |search, cx| {
                                    search.set_value("", window, cx);
                                    search.focus(window, cx);
                                });
                                panel.model_scroll.set_offset(point(zpx(0.), zpx(0.)));
                                panel.state.apply(ModelChange::Refresh(false), cx);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    }))
                    .content(move |_, _, _| panel.clone()),
            )
    }
}

struct CloseModelPicker;

struct ModelPanel {
    state: ModelSource,
    models: bool,
    levels: Vec<EffortOption>,
    effort_index: usize,
    previous_effort_index: usize,
    trigger_bottom: Rc<Cell<Pixels>>,
    model_scroll: ScrollHandle,
    search: Entity<InputState>,
    focus: FocusHandle,
    choices: Vec<Option<String>>,
    highlighted: Option<usize>,
}

impl EventEmitter<CloseModelPicker> for ModelPanel {}

impl ModelPanel {
    fn choose_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.read(cx).locked || index == self.effort_index {
            return;
        }
        let Some(level) = self.levels.get(index) else {
            return;
        };
        let effort = Some(level.id.clone());
        self.previous_effort_index = self.effort_index;
        self.effort_index = index;
        self.state.apply(ModelChange::Effort(effort), cx);
        crate::haptics::tick();
        cx.notify();
    }

    fn new(
        state: ModelSource,
        trigger_bottom: Rc<Cell<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        state.observe(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
        cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.highlighted = None;
                this.model_scroll.set_offset(point(zpx(0.), zpx(0.)));
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            models: true,
            levels: vec![],
            effort_index: 0,
            previous_effort_index: 0,
            trigger_bottom,
            model_scroll: ScrollHandle::new(),
            search,
            focus: cx.focus_handle(),
            choices: Vec::new(),
            highlighted: None,
        }
    }
}

impl Render for ModelPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let available_height = if state.above {
            self.trigger_bottom.get() - zpx(28.)
        } else {
            window.viewport_size().height - self.trigger_bottom.get()
        };
        let catalog = state.catalog.clone();
        let (provider, selected) = (state.provider, state.model.clone());
        let accent = rgb(match provider {
            Provider::Codex => 0x007aff,
            Provider::OpenCode => 0xd0d0ce,
            _ => crate::composer_ui::provider_brand(provider).1,
        });
        let info = state.info();
        let effort = state.effort.clone();
        let fast = state.tier.is_some();
        let supports_fast = info.as_ref().is_some_and(|m| m.fast_tier.is_some());
        let locked = state.locked;
        let muted = rgba(0xffffff80);
        let root = v_flex()
            .id("model-panel")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.key == "escape" {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.emit(CloseModelPicker);
                } else if !this.models
                    && !key.modifiers.platform
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && matches!(key.key.as_str(), "left" | "right")
                    && !this.state.read(cx).locked
                    && let Some(index) =
                        next_effort_index(this.effort_index, this.levels.len(), key.key == "right")
                {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.choose_effort(index, cx);
                }
            }))
            .w(zpx(278.))
            .p_3()
            .gap_3()
            .rounded_xl()
            .bg(crate::appearance::popup_surface())
            .text_size(zpx(13.));
        if self.models {
            let query = self.search.read(cx).value().to_string();
            let show_default = fuzzy_model_matches(&query, "Default recommended");
            let models: Vec<_> = catalog
                .models
                .iter()
                .enumerate()
                .filter(|(_, model)| !(model.id == "default" && model.is_default))
                .filter(|(_, model)| {
                    fuzzy_model_matches(&query, &model.name)
                        || fuzzy_model_matches(&query, &model.id)
                })
                .collect();
            self.choices = show_default
                .then_some(None)
                .into_iter()
                .chain(models.iter().map(|(_, model)| Some(model.id.clone())))
                .collect();
            let model_count = self.choices.len();
            self.highlighted = self.highlighted.filter(|index| *index < model_count);
            let list_height = zpx(model_count.max(1) as f32 * 42.)
                .min((available_height - zpx(84.)).clamp(zpx(60.), zpx(240.)));
            let scroll = self.model_scroll.clone();
            let mut list = v_flex().gap_0p5().when(show_default, |list| {
                list.child(
                    Button::new("model-default")
                        .size(crate::icons::button_size())
                        .p_0()
                        .accessibility_label("Default model")
                        .when(self.highlighted == Some(0), |button| {
                            button.bg(rgba(0xffffff14))
                        })
                        .ghost()
                        .w_full()
                        .h_auto()
                        .flex_none()
                        .py_1p5()
                        .text_size(zpx(12.))
                        .justify_start()
                        .disabled(locked)
                        .when(
                            selected.is_none() || selected.as_deref() == Some("default"),
                            |b| b.child(crate::icons::icon(IconName::Check)),
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
                                        .text_size(zpx(10.5))
                                        .text_color(muted)
                                        .child("Use the provider’s recommended model"),
                                ),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.state.apply(ModelChange::Model(None), cx);
                            this.focus.focus(window, cx);
                            this.models = false;
                            cx.notify();
                        })),
                )
            });
            if model_count == 0 && !catalog.loading {
                list = list.child(div().p_2().text_color(muted).child("No matching models"));
            }
            for (visible_index, (i, model)) in models.into_iter().enumerate() {
                let choice_index = visible_index + usize::from(show_default);
                let id = model.id.clone();
                list = list.child(
                    Button::new(("model-choice", i))
                        .size(crate::icons::button_size())
                        .p_0()
                        .accessibility_label(model.name.clone())
                        .when(self.highlighted == Some(choice_index), |button| {
                            button.bg(rgba(0xffffff14))
                        })
                        .ghost()
                        .w_full()
                        .h_auto()
                        .flex_none()
                        .py_1p5()
                        .text_size(zpx(12.))
                        .justify_start()
                        .disabled(locked)
                        .when(selected.as_deref() == Some(&id), |b| {
                            b.child(crate::icons::icon(IconName::Check))
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
                                            .text_size(zpx(10.5))
                                            .max_w(zpx(250.))
                                            .truncate()
                                            .text_color(muted)
                                            .child(model.description.clone()),
                                    )
                                }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state.apply(ModelChange::Model(Some(id.clone())), cx);
                            this.focus.focus(window, cx);
                            this.models = false;
                            cx.notify();
                        })),
                );
            }
            return root
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.platform
                        || event.keystroke.modifiers.control
                        || event.keystroke.modifiers.alt
                    {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "up" | "down" => {
                            window.prevent_default();
                            cx.stop_propagation();
                            if this.state.read(cx).locked {
                                return;
                            }
                            this.highlighted = next_model_index(
                                this.highlighted,
                                this.choices.len(),
                                event.keystroke.key == "down",
                            );
                            if let Some(index) = this.highlighted {
                                this.model_scroll.scroll_to_item(index);
                            }
                            cx.notify();
                        }
                        "enter" => {
                            window.prevent_default();
                            cx.stop_propagation();
                            if this.state.read(cx).locked {
                                return;
                            }
                            if let Some(model) =
                                this.choices.get(this.highlighted.unwrap_or(0)).cloned()
                            {
                                this.state.apply(ModelChange::Model(model), cx);
                                this.focus.focus(window, cx);
                                this.models = false;
                                cx.notify();
                            }
                        }
                        _ => {}
                    }
                }))
                .p_2p5()
                .gap_2()
                .text_size(zpx(12.))
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.search)
                                    .small()
                                    .appearance(false)
                                    .bordered(false)
                                    .cleanable(true)
                                    .prefix(crate::icons::icon(IconName::Search).text_color(muted)),
                            ),
                        )
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
                                    this.state.apply(ModelChange::Refresh(true), cx)
                                })),
                        ),
                )
                .when(catalog.loading, |col| {
                    col.child(div().text_color(muted).child("Loading available models…"))
                })
                .child(
                    div()
                        .relative()
                        .h(list_height)
                        .flex_none()
                        .overflow_hidden()
                        .child(
                            list.id("model-list")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.model_scroll),
                        )
                        .child(
                            canvas(
                                |bounds, _, _| bounds,
                                move |_, bounds, window, _| {
                                    // Read after layout so the fade is accurate on the
                                    // first frame, after refresh, and at the scroll end.
                                    if scroll.max_offset().y + scroll.offset().y > zpx(1.) {
                                        window.paint_quad(fill(
                                            bounds,
                                            linear_gradient(
                                                0.,
                                                linear_color_stop(
                                                    crate::appearance::popup_surface(),
                                                    0.,
                                                ),
                                                linear_color_stop(
                                                    {
                                                        let mut color =
                                                            crate::appearance::popup_surface();
                                                        color.a = 0.;
                                                        color
                                                    },
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
                            .h(zpx(18.)),
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
        let effort_label = effort_label(effort.as_deref(), info.as_ref());
        let model_name = model_label(selected.as_deref(), info.as_ref());
        root.child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    Button::new("fast-mode")
                        .ghost()
                        .size(crate::icons::button_size())
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
                        .child(crate::icons::path("icons/zap.svg").text_color(if fast {
                            accent
                        } else {
                            rgba(0xffffff70)
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.apply(ModelChange::Fast(!fast), cx);
                        })),
                )
                .child(
                    Button::new("choose-another-model")
                        .ghost()
                        .small()
                        .compact()
                        .text_size(zpx(16.))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(model_name)
                                .when_some(effort_label, |row, effort| {
                                    row.child(div().text_color(rgba(0xffffff72)).child(effort))
                                })
                                .child(
                                    crate::icons::icon(IconName::ChevronRight)
                                        .text_color(rgba(0xffffff62)),
                                ),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.models = true;
                            this.search
                                .update(cx, |search, cx| search.focus(window, cx));
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("reset-effort")
                        .ghost()
                        .size(crate::icons::button_size())
                        .flex_none()
                        .p_0()
                        .accessibility_label("Reset model settings and refresh")
                        .tooltip("Reset model settings and refresh")
                        .text_color(rgba(0xffffff70))
                        .disabled(locked || catalog.loading)
                        .child(crate::icons::icon(IconName::RefreshCw).text_color(rgba(0xffffff70)))
                        .on_click(
                            cx.listener(|this, _, _, cx| this.state.apply(ModelChange::Reset, cx)),
                        ),
                ),
        )
        .when(!self.levels.is_empty(), |col| {
            const TRACK_WIDTH: f32 = 254.0;
            const THUMB_SIZE: f32 = 28.0;
            const TRACK_INSET: f32 = THUMB_SIZE / 2.0;
            let travel = TRACK_WIDTH - THUMB_SIZE;
            let position = |value: usize| {
                if value == 0 {
                    // Cover the rounded fill cap at the minimum stop.
                    -2.0
                } else {
                    travel * value as f32 / max
                }
            };
            let old_position = position(self.previous_effort_index.min(self.levels.len() - 1));
            let new_position = position(self.effort_index);
            let old_fill = old_position + TRACK_INSET;
            let new_fill = new_position + TRACK_INSET;
            let stops = self.levels.len();

            col.child(
                div()
                    .id("effort-track")
                    .relative()
                    .w(zpx(TRACK_WIDTH))
                    .h(zpx(32.))
                    .cursor_grab()
                    .on_drag(EffortDrag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_drag_move(
                        cx.listener(|this, event: &DragMoveEvent<EffortDrag>, _, cx| {
                            let stops = this.levels.len().saturating_sub(1);
                            if stops == 0 {
                                return;
                            }
                            let x = del_core::zoom::unscaled(
                                (event.event.position.x - event.bounds.origin.x).as_f32(),
                            )
                            .clamp(0.0, TRACK_WIDTH);
                            let stop = (((x - TRACK_INSET) / (TRACK_WIDTH - THUMB_SIZE))
                                * stops as f32)
                                .round()
                                .clamp(0.0, stops as f32)
                                as usize;
                            this.choose_effort(stop, cx);
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(zpx(4.))
                            .w_full()
                            .h(zpx(24.))
                            .rounded(zpx(12.))
                            .overflow_hidden()
                            .bg(rgb(0x2a2a2d))
                            .child(
                                div().h_full().rounded(zpx(12.)).bg(accent).with_animation(
                                    SharedString::from(format!(
                                        "effort-fill-{old_fill}-{new_fill}"
                                    )),
                                    Animation::new(std::time::Duration::from_millis(180))
                                        .with_easing(ease_out_quint()),
                                    move |fill, t| {
                                        fill.w(zpx(old_fill + (new_fill - old_fill) * t))
                                    },
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .absolute()
                            .left(zpx(TRACK_INSET))
                            .right(zpx(TRACK_INSET))
                            .top_0()
                            .bottom_0()
                            .items_center()
                            .justify_between()
                            .children(
                                (0..stops).map(|_| {
                                    div().size(zpx(5.)).rounded_full().bg(rgba(0xffffff40))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(zpx(2.))
                            .size(zpx(THUMB_SIZE))
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
                                    thumb
                                        .left(zpx(old_position + (new_position - old_position) * t))
                                },
                            ),
                    )
                    .children((0..stops).map(|stop| {
                        div()
                            .id(("effort-stop", stop))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(stop as f32 / stops as f32))
                            .w(relative(1.0 / stops as f32))
                            .cursor_pointer()
                            .on_drag(EffortDrag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.choose_effort(stop, cx)),
                            )
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

fn next_effort_index(current: usize, count: usize, right: bool) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let current = current.min(last);
    Some(if right {
        current.saturating_add(1).min(last)
    } else {
        current.saturating_sub(1)
    })
}

fn next_model_index(current: Option<usize>, count: usize, down: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (current.filter(|index| *index < count), down) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(index), true) => (index + 1) % count,
        (Some(0), false) => count - 1,
        (Some(index), false) => index - 1,
    })
}

/// Case-insensitive subsequence matching, ignoring spaces and punctuation.
/// Names and provider IDs are searched separately to avoid cross-field matches.
fn fuzzy_model_matches(query: &str, candidate: &str) -> bool {
    let mut candidate = candidate
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric());
    query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .all(|needle| candidate.by_ref().any(|c| c == needle))
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_model_matches, next_effort_index, next_model_index};

    #[test]
    fn model_effort_label_shows_default_before_any_explicit_change() {
        let mut info = del_agent::ModelInfo {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            efforts: vec![
                del_agent::EffortOption::named("medium"),
                del_agent::EffortOption::named("high"),
            ],
            default_effort: Some("medium".into()),
            fast_tier: None,
            is_default: false,
        };
        assert_eq!(
            super::effort_label(None, Some(&info)).as_deref(),
            Some("Medium")
        );
        assert_eq!(
            super::effort_label(Some("high"), Some(&info)).as_deref(),
            Some("High")
        );
        info.default_effort = Some("high".into());
        assert_eq!(
            super::effort_label(None, Some(&info)).as_deref(),
            Some("High")
        );
        info.default_effort = None;
        assert_eq!(super::effort_label(None, Some(&info)), None);
    }

    #[test]
    fn effort_arrows_step_once_and_stop_at_endpoints() {
        assert_eq!(next_effort_index(2, 6, true), Some(3));
        assert_eq!(next_effort_index(2, 6, false), Some(1));
        assert_eq!(next_effort_index(0, 6, false), Some(0));
        assert_eq!(next_effort_index(5, 6, true), Some(5));
        assert_eq!(next_effort_index(0, 0, true), None);
        assert_eq!(next_effort_index(9, 1, true), Some(0));
    }

    #[test]
    fn model_navigation_wraps_and_handles_filtered_or_empty_results() {
        assert_eq!(next_model_index(None, 3, true), Some(0));
        assert_eq!(next_model_index(None, 3, false), Some(2));
        assert_eq!(next_model_index(Some(2), 3, true), Some(0));
        assert_eq!(next_model_index(Some(0), 3, false), Some(2));
        assert_eq!(next_model_index(Some(8), 2, true), Some(0));
        assert_eq!(next_model_index(Some(0), 0, true), None);
    }

    #[test]
    fn model_search_matches_abbreviations_case_and_separators() {
        assert!(fuzzy_model_matches("GPT 61", "gpt-6.1-sol"));
        assert!(fuzzy_model_matches("gptstr", "GPT-6-Astra"));
        assert!(fuzzy_model_matches("ops55", "Opus 5.5"));
        assert!(fuzzy_model_matches("", "Any model"));
        assert!(fuzzy_model_matches("recommended", "Default recommended"));
        assert!(!fuzzy_model_matches("opus", "GPT-6-Sol"));
        assert!(!fuzzy_model_matches("66", "GPT-6-Sol"));
        assert!(!fuzzy_model_matches("solgpt", "GPT-6-Sol"));
    }
}
