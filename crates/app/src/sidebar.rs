//! The sessions sidebar: new-session button, session rows, archived list and
//! the providers entry. Borderless; rows are told apart by fill alone.

use crate::appearance::{rgb, rgba};
use crate::composer_ui::provider_logo;
use crate::sessions::Row;
use crate::state::{AppState, Panel, Selection};
use crate::zoom::px as zpx;
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::assets::IconName as ActionIcon;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 180.0;
const ROW_HEIGHT: f32 = 28.0;
const TOOLBAR_HEIGHT: f32 = crate::icons::TOOL_TAB_HEIGHT;
const TOOLBAR_BACKGROUND: u8 = 0x0e;
const TOOLBAR_HOVER: u8 = 0x1a;
const TOOLBAR_PRESSED: u8 = 0x24;

#[derive(Default)]
pub struct DragMotion {
    pub active: Option<del_agent::SessionId>,
    pub shifts: std::collections::HashMap<String, f32>,
    pub generation: usize,
}

fn moving_row(
    item: impl IntoElement,
    id: &del_agent::SessionId,
    state: &Entity<AppState>,
    cx: &App,
) -> AnyElement {
    let motion = &state.read(cx).sidebar_drag;
    let offset = motion.shifts.get(&id.0).copied().unwrap_or(0.);
    let hidden = motion.active.as_ref() == Some(id) && cx.has_active_drag();
    div()
        .flex_none()
        .opacity(if hidden { 0. } else { 1. })
        .child(item)
        .with_animation(
            SharedString::from(format!("sidebar-shift-{}-{}", id.0, motion.generation)),
            Animation::new(std::time::Duration::from_millis(if cx.reduce_motion() {
                0
            } else {
                90
            }))
            .with_easing(ease_out_quint()),
            move |row, t| row.relative().top(zpx(offset * (1. - t))),
        )
        .into_any_element()
}

#[derive(Clone)]
struct SidebarDrag {
    id: del_agent::SessionId,
    title: String,
    provider: Option<del_agent::Provider>,
    terminal: bool,
    pinned: bool,
    start_pointer_x: Pixels,
}

impl Render for SidebarDrag {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Cancel horizontal pointer movement, retaining the original grab offset.
        div().w(zpx(SIDEBAR_WIDTH - 12.)).h(zpx(ROW_HEIGHT)).child(
            div()
                .relative()
                .left(self.start_pointer_x - window.mouse_position().x)
                .h(zpx(ROW_HEIGHT))
                .w(zpx(SIDEBAR_WIDTH - 12.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .bg(crate::appearance::popup_surface())
                .text_size(zpx(TEXT_MD))
                .text_color(rgb(TEXT))
                .child(if self.terminal {
                    crate::mode_ui::terminal_mark(self.provider)
                } else {
                    crate::mode_ui::tab_mark(self.provider)
                })
                .child(div().truncate().child(self.title.clone())),
        )
    }
}

fn draggable_row(
    item: Stateful<Div>,
    drag: SidebarDrag,
    state: &Entity<AppState>,
) -> Stateful<Div> {
    let target = drag.clone();
    let moving_target = drag.clone();
    let moving_state = state.clone();
    let state = state.clone();
    let starting_state = state.clone();
    item.on_drag(drag, move |drag, _, window, cx| {
        starting_state.update(cx, |state, cx| {
            state.sidebar_drag.active = Some(drag.id.clone());
            state.sidebar_drag.shifts.clear();
            cx.notify();
        });
        let mut preview = drag.clone();
        preview.start_pointer_x = window.mouse_position().x;
        cx.new(|_| preview)
    })
    .on_drag_move(move |event: &DragMoveEvent<SidebarDrag>, _, cx| {
        let drag = event.drag(cx).clone();
        if event.event.position.y < event.bounds.top()
            || event.event.position.y > event.bounds.bottom()
            || drag.id == moving_target.id
            || drag.pinned != moving_target.pinned
        {
            return;
        }
        let state = moving_state.read(cx);
        let ids: Vec<_> = state.unified_rows().into_iter().map(|row| row.id).collect();
        let from = ids.iter().position(|id| id == &drag.id);
        let to = ids.iter().position(|id| id == &moving_target.id);
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        let midpoint = event.bounds.top() + event.bounds.size.height / 2.;
        let dead_zone = zpx(3.);
        if (from < to && event.event.position.y < midpoint + dead_zone)
            || (from > to && event.event.position.y > midpoint - dead_zone)
        {
            return;
        }
        moving_state.update(cx, |state, cx| {
            state.reorder_sidebar(&drag.id, &moving_target.id, drag.terminal, cx)
        });
    })
    .on_drop(move |drag: &SidebarDrag, _, cx| {
        if drag.pinned == target.pinned {
            state.update(cx, |state, cx| {
                state.reorder_sidebar(&drag.id, &target.id, drag.terminal, cx)
            });
        }
    })
}

fn loading_title(title: String, id: &del_agent::SessionId) -> AnyElement {
    h_flex()
        .min_w_0()
        .overflow_hidden()
        .child(
            gpui_kit::component::shimmer::ShimmerText::new(title)
                .id(SharedString::from(format!("sidebar-title-{}", id.0)))
                .flex_none()
                .text_color(rgba(0xffffff66))
                .highlight_color(rgb(0xf5f5f5))
                .spread(0.22)
                .duration(std::time::Duration::from_millis(1200)),
        )
        .into_any_element()
}

fn toolbar_tint(alpha: u8) -> Rgba {
    let tint = if crate::appearance::light() {
        0x00000000
    } else {
        0xffffff00
    };
    gpui_kit::rgba(tint | u32::from(alpha))
}

fn toolbar_row() -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .gap(zpx(6.))
        .pr(zpx(6.))
        .py(zpx((crate::icons::TOOL_ROW_HEIGHT - TOOLBAR_HEIGHT) / 2.))
        .flex_none()
}

fn toolbar_button(id: &'static str, cx: &App) -> Button {
    let background = toolbar_tint(TOOLBAR_BACKGROUND);
    Button::new(id)
        .tooltip_placement(Placement::Bottom)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(background.into())
                .hover(toolbar_tint(TOOLBAR_HOVER).into())
                .active(toolbar_tint(TOOLBAR_PRESSED).into())
                .shadow(false),
        )
        // One translucent fill per state; no extra border tint or base layer.
        // Retain the component's keyboard focus ring.
        .border_0()
        .w_0()
        .min_w_0()
        .flex_1()
        .h(zpx(TOOLBAR_HEIGHT))
        .p_0()
        .rounded(zpx(9.))
}

fn toolbar_trigger_style() -> StyleRefinement {
    StyleRefinement::default().w_0().min_w_0().flex_1()
}

const TEXT: u32 = 0xf0f0f0;
const MUTED: u32 = 0xffffff8c;
const FAINT: u32 = 0xffffff52;

pub(crate) fn fading_title(text: String) -> impl IntoElement {
    let label = text.clone();
    let text: SharedString = text.replace(['\n', '\r'], " ").into();
    div()
        .id("fading-title")
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .aria_label(label)
        .child(
            canvas(
                move |_, window, _| {
                    let style = window.text_style();
                    let line = window.text_system().shape_line(
                        text.clone(),
                        style.font_size.to_pixels(window.rem_size()),
                        &[style.to_run(text.len())],
                        None,
                    );
                    (line, style)
                },
                |bounds, (line, style), window, cx| {
                    let fade = zpx(24.).min(bounds.size.width);
                    let overflow = line.width() > bounds.size.width;
                    let solid = if overflow {
                        bounds.size.width - fade
                    } else {
                        bounds.size.width
                    };
                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: Bounds::new(bounds.origin, size(solid, bounds.size.height)),
                        }),
                        |window| {
                            let _ = line.paint(
                                bounds.origin,
                                bounds.size.height,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        },
                    );
                    if overflow {
                        // Each slice repaints the shaped line under its own clip. Twelve
                        // 2px slices (24px fade) keep the ramp visually smooth at a
                        // quarter of the previous paint cost; GPUI has no gradient text mask.
                        const SLICES: usize = 12;
                        for step in 0..SLICES {
                            let start = solid + fade * (step as f32 / SLICES as f32);
                            let end = solid + fade * ((step + 1) as f32 / SLICES as f32);
                            let clip = Bounds::new(
                                point(bounds.left() + start, bounds.top()),
                                size(end - start, bounds.size.height),
                            );
                            let decoration = DecorationRun {
                                len: line.len() as u32,
                                color: style
                                    .color
                                    .opacity(1. - (step as f32 + 0.5) / SLICES as f32),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            window.with_content_mask(
                                Some(ContentMask { bounds: clip }),
                                |window| {
                                    let _ = LineLayout::paint(
                                        &line,
                                        bounds.origin,
                                        bounds.size.height,
                                        TextAlign::Left,
                                        None,
                                        &[decoration],
                                        window,
                                        cx,
                                    );
                                },
                            );
                        }
                    }
                },
            )
            .w_full()
            .h(zpx(20.)),
        )
}

pub fn unread_dot(id: &del_agent::SessionId, state: &AppState, cx: &App) -> AnyElement {
    let (from, target, start) = state.unread_motion(id);
    let dot = div().rounded_full().bg(rgb(0x4a9eff));
    let dot = if !cx.reduce_motion()
        && let Some(start) = start.filter(|start| start.elapsed().as_millis() < 180)
    {
        dot.with_animation(
            SharedString::from(format!("unread-{}-{target}", id.0)),
            Animation::new(std::time::Duration::from_millis(180)),
            move |dot, _| {
                let t = (start.elapsed().as_secs_f32() / 0.18).min(1.0);
                let value = from + (target - from) * (1.0 - (1.0 - t).powi(3));
                dot.size(zpx(6.0 * value)).opacity(value)
            },
        )
        .into_any_element()
    } else {
        dot.size(zpx(6.0 * target))
            .opacity(target)
            .into_any_element()
    };
    div()
        .size(zpx(6.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(dot)
        .into_any_element()
}

/// Keep the cup steady while two small wisps rise above it. The wisps only
/// move while the window is active, so an idle background Del draws no frames.
fn coffee_icon(active: bool, animate: bool, cx: &App) -> AnyElement {
    let color = if active { rgb(0xf5a524) } else { rgba(MUTED) };
    div()
        .relative()
        .size(crate::icons::size())
        .child(crate::icons::path("icons/coffee-cup.svg").text_color(color))
        .when(active, |cup| {
            cup.children((0..2).map(|i| {
                let steam = svg()
                    .path("icons/coffee-steam.svg")
                    .absolute()
                    .left(zpx(4. + i as f32 * 5.))
                    .top(zpx(-5.))
                    .w(zpx(3.))
                    .h(zpx(7.))
                    .text_color(color);
                if cx.reduce_motion() || !animate {
                    steam.opacity(0.6).into_any_element()
                } else {
                    steam
                        .with_animation(
                            ("coffee-steam", i as usize),
                            Animation::new(std::time::Duration::from_millis(1800)).repeat(),
                            move |steam, t| {
                                let phase = (t + i as f32 * 0.5) % 1.;
                                steam
                                    .top(zpx(-2. - phase * 6.))
                                    .opacity((std::f32::consts::PI * phase).sin() * 0.75)
                            },
                        )
                        .into_any_element()
                }
            }))
        })
        .into_any_element()
}

/// Compact hover actions retain spoken names without visible text labels.
fn pin_button(id: impl Into<ElementId>, pinned: bool) -> Button {
    Button::new(id)
        .text()
        .size(crate::icons::button_size())
        .p_0()
        .tooltip(if pinned { "Unpin" } else { "Pin" })
        .accessibility_label(if pinned { "Unpin" } else { "Pin" })
        .text_color(if pinned { rgb(0xf5f5f5) } else { rgba(MUTED) })
        .child(if pinned {
            crate::icons::path("icons/pin-filled.svg")
        } else {
            crate::icons::icon(ActionIcon::Pin)
        })
}

fn archive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .text()
        .size(crate::icons::button_size())
        .p_0()
        .tooltip("Archive")
        .accessibility_label("Archive")
        .text_color(rgba(MUTED))
        .child(crate::icons::icon(ActionIcon::Archive))
}

fn archived_actions(id: &del_agent::SessionId, state: &Entity<AppState>) -> Div {
    let restore_state = state.clone();
    let restore_id = id.clone();
    let delete_state = state.clone();
    let delete_id = id.clone();
    h_flex()
        .items_center()
        .gap(zpx(4.0))
        .child(
            Button::new(SharedString::from(format!("restore-{}", id.0)))
                .text()
                .size(crate::icons::button_size())
                .p_0()
                .tooltip("Restore")
                .accessibility_label("Restore")
                .text_color(rgba(MUTED))
                .child(crate::icons::path("icons/restore.svg"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    restore_state.update(cx, |s, cx| s.restore(restore_id.clone(), cx));
                }),
        )
        .child(
            Button::new(SharedString::from(format!("delete-{}", id.0)))
                .ghost()
                .size(crate::icons::button_size())
                .p_0()
                .tooltip("Permanently delete")
                .accessibility_label("Permanently delete")
                .child(crate::icons::icon(ActionIcon::Trash).text_color(rgb(0xff646b)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    delete_state.update(cx, |s, cx| s.permanently_delete(delete_id.clone(), cx));
                }),
        )
}

fn session_menu_item(
    label: &'static str,
    icon: Icon,
    motion: &crate::hover_motion::HoverMotion,
) -> impl IntoElement {
    let destructive = label == "Permanently delete";
    let hover = motion.clone();
    h_flex()
        .id(label)
        .debug_selector(|| format!("session-menu-{label}"))
        .role(gpui::Role::MenuItem)
        .aria_label(label)
        .items_center()
        .h(zpx(28.))
        .gap(zpx(10.))
        .relative()
        .px_2()
        .text_size(zpx(TEXT_MD))
        .cursor_pointer()
        .when(destructive, |item| item.text_color(rgb(0xff646b)))
        .on_hover(move |entered, window, cx| hover.hover(label, *entered, window, cx))
        .child(motion.marker(
            label.into(),
            rgba(0xffffff00),
            if destructive {
                rgba(0xff45452e)
            } else {
                rgba(0xffffff24)
            },
        ))
        .child(
            icon.with_size(crate::icons::size())
                .when(destructive, |icon| icon.text_color(rgb(0xff646b))),
        )
        .child(label)
}

fn working_mark(working: bool, group: SharedString, right: f32) -> AnyElement {
    div()
        .absolute()
        .right(zpx(right))
        .top(zpx((ROW_HEIGHT - crate::icons::BUTTON_SIZE) / 2.0))
        .size(crate::icons::button_size())
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .group_hover(group, |mark| mark.opacity(0.0))
        .when(working, |mark| {
            mark.child(crate::icons::working_spinner().color(rgba(MUTED).into()))
        })
        .into_any_element()
}

fn row_item(
    row: &Row,
    selected: bool,
    renaming: bool,
    rename: &Entity<InputState>,
    state: &Entity<AppState>,
    archived: bool,
    cx: &App,
) -> AnyElement {
    let terminal = state.read(cx).terminal_rows.iter().find(|r| r.id == row.id);
    let is_terminal = terminal.is_some();
    let logo = terminal.map(|r| r.provider).unwrap_or(Some(row.provider));
    let working = state.read(cx).session_working(&row.id);
    let id = row.id.clone();
    let key = SharedString::from(id.0.clone());
    let group = SharedString::from(format!("row-{}", id.0));

    let mark_right = if state.read(cx).terminal_unread(&id) {
        20.0
    } else {
        8.0
    };
    let action_right = 8.0;
    let motion = state.read(cx).sidebar_hover.clone();
    let hover = motion.clone();
    let hover_key = id.0.clone();
    let mut item = h_flex()
        .id(key.clone())
        .relative()
        .group(group.clone())
        .h(zpx(ROW_HEIGHT))
        .min_h(zpx(ROW_HEIGHT))
        .flex_none()
        .px_2()
        .gap_1p5()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_size(zpx(TEXT_MD))
        .text_color(rgb(TEXT))
        .on_hover(move |entered, window, cx| hover.hover(&hover_key, *entered, window, cx))
        .child(motion.marker_with_opacity(
            id.0.clone(),
            if selected {
                rgba(0xffffff14)
            } else {
                rgba(0xffffff00)
            },
            rgba(0xffffff0a),
            Some(std::rc::Rc::new(std::cell::Cell::new(
                if state.read(cx).sidebar_drag.active.as_ref() == Some(&id) && cx.has_active_drag()
                {
                    0.
                } else {
                    1.
                },
            ))),
        ))
        .child(if is_terminal {
            crate::mode_ui::terminal_mark(logo)
        } else {
            provider_logo(row.provider).into_any_element()
        });

    if !archived && !renaming {
        item = draggable_row(
            item,
            SidebarDrag {
                id: id.clone(),
                title: row.title.clone(),
                provider: logo,
                terminal: is_terminal,
                pinned: row.pinned,
                start_pointer_x: Pixels::ZERO,
            },
            state,
        );
    }
    item = if renaming {
        item.child(
            div().flex_1().min_w_0().child(
                Input::new(rename)
                    .appearance(false)
                    .bordered(false)
                    .text_size(zpx(TEXT_MD)),
            ),
        )
    } else {
        let state_click = state.clone();
        let click_id = id.clone();
        item.on_click(move |event, _, cx| {
            let id = click_id.clone();
            state_click.update(cx, |s, cx| {
                if archived {
                    s.restore(id.clone(), cx);
                } else {
                    if is_terminal {
                        s.select_terminal(id.clone(), cx);
                    } else {
                        s.select(id.clone(), cx);
                    }
                }
                if event.click_count() == 2 {
                    s.keep_tab(
                        &if is_terminal {
                            crate::state::Tab::Terminal(id)
                        } else {
                            crate::state::Tab::Session(id)
                        },
                        cx,
                    );
                }
            });
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_color(if selected {
                    rgb(0xf5f5f5)
                } else {
                    rgba(0xffffffcc)
                })
                .font_weight(FontWeight::NORMAL)
                .child(if state.read(cx).title_loading(&id) {
                    loading_title(row.title.clone(), &id)
                } else {
                    fading_title(row.title.clone()).into_any_element()
                }),
        )
    };

    if !renaming {
        let (pin_state, archive_state) = (state.clone(), state.clone());
        let (pin_id, archive_id) = (id.clone(), id.clone());
        let hover_group = group.clone();
        item = item
            .child(working_mark(working, group.clone(), mark_right))
            .child(
                // Reserve space so titles never overlap the hover actions.
                div().flex_none().w(zpx(if motion.is_hovered(&row.id.0) {
                    crate::icons::BUTTON_SIZE * 2.0 + 4.0
                } else if working {
                    crate::icons::BUTTON_SIZE
                } else {
                    0.0
                })),
            )
            .child(
                div()
                    .flex_none()
                    .group_hover(group.clone(), |dot| dot.opacity(0.))
                    .child(unread_dot(&row.id, state.read(cx), cx)),
            )
            .child(
                // Always laid out and painted, only faded in on hover: toggling
                // `display` under the pointer made GPUI paint children it had
                // not prepainted, which aborted the app.
                h_flex()
                    .absolute()
                    .right(zpx(action_right))
                    .top(zpx((ROW_HEIGHT - crate::icons::BUTTON_SIZE) / 2.0))
                    .h(crate::icons::button_size())
                    .items_center()
                    .gap(zpx(4.0))
                    .w(zpx(crate::icons::BUTTON_SIZE * 2.0 + 4.0))
                    .rounded_sm()
                    .opacity(0.0)
                    .group_hover(hover_group, |a| a.opacity(1.0))
                    .when(!archived, |a| {
                        a.child(
                            archive_button(SharedString::from(format!("ar-{}", id.0)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let id = archive_id.clone();
                                    archive_state.update(cx, |s, cx| s.archive(id, cx));
                                }),
                        )
                        .child(
                            pin_button(
                                SharedString::from(format!("pin-session-{}", id.0)),
                                row.pinned,
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                pin_state.update(cx, |s, cx| s.toggle_session_pin(&pin_id, cx));
                            }),
                        )
                    })
                    .when(archived, |a| a.child(archived_actions(&id, state))),
            );

        if !archived {
            let state = state.clone();
            let id = id.clone();
            let pinned = row.pinned;
            let motion_state = state.clone();
            let motion_id = id.clone();
            return moving_row(
                item.context_menu(move |menu, _, cx| session_menu(menu, &state, &id, pinned, cx)),
                &motion_id,
                &motion_state,
                cx,
            );
        }
    }
    moving_row(item, &id, state, cx)
}

fn session_menu_content(motion: &crate::hover_motion::HoverMotion, pinned: bool, cx: &App) -> Div {
    let background: Rgba = cx.theme().popover.into();
    // One native menu item wraps the custom group, so native row hover
    // backgrounds cannot peek out around the moving highlight.
    v_flex()
        .relative()
        .flex_1()
        .min_w(zpx(176.))
        // PopupMenu's wrapper uses 8 logical pixels of padding.
        // A rem-based negative margin leaves strips at smaller font sizes.
        .mx(gpui::px(-8.))
        .gap_0p5()
        .bg(background)
        .child(motion.layer(Some(background), cx.theme().border.into()))
        .child(session_menu_item(
            "Rename",
            crate::icons::icon(ActionIcon::Pencil),
            motion,
        ))
        .child(session_menu_item(
            if pinned { "Unpin" } else { "Pin" },
            crate::icons::path(if pinned {
                "icons/pin-filled.svg"
            } else {
                "icons/pin-outline.svg"
            }),
            motion,
        ))
        .child(session_menu_item(
            "Mark as unread",
            crate::icons::icon(ActionIcon::Eye),
            motion,
        ))
        .child(session_menu_item(
            "Unload",
            crate::icons::icon(ActionIcon::Close),
            motion,
        ))
        .child(motion.divider())
        .child(session_menu_item(
            "Archive",
            crate::icons::path("icons/archive.svg"),
            motion,
        ))
        .child(session_menu_item(
            "Permanently delete",
            crate::icons::icon(ActionIcon::Trash),
            motion,
        ))
}

fn project_menu_content(
    motion: &crate::hover_motion::HoverMotion,
    archived: bool,
    cx: &App,
) -> Div {
    let background: Rgba = cx.theme().popover.into();
    v_flex()
        .relative()
        .flex_1()
        .min_w(zpx(176.))
        .mx(gpui::px(-8.))
        .gap_0p5()
        .bg(background)
        .child(motion.layer(Some(background), cx.theme().border.into()))
        .child(session_menu_item(
            "Rename",
            crate::icons::icon(ActionIcon::Pencil),
            motion,
        ))
        .child(session_menu_item(
            if archived {
                "Restore project"
            } else {
                "Archive"
            },
            crate::icons::path(if archived {
                "icons/restore.svg"
            } else {
                "icons/archive.svg"
            }),
            motion,
        ))
        .child(motion.divider())
        .child(session_menu_item(
            "Permanently delete",
            crate::icons::icon(ActionIcon::Trash),
            motion,
        ))
}

fn project_menu(
    menu: PopupMenu,
    state: &Entity<AppState>,
    path: &std::path::Path,
    cx: &mut App,
) -> PopupMenu {
    let archived_only = state.read(cx).show_archived;
    let archived = archived_only;
    let motion = crate::hover_motion::HoverMotion::menu(cx);
    let click_motion = motion.clone();
    let state = state.clone();
    let path = path.to_path_buf();
    menu.item(PopupMenuItem::element(move |_, cx| project_menu_content(&motion, archived, cx))
        .on_click(move |event, window, cx| {
            let Some(action) = click_motion.clicked(event) else { return; };
            if action == "Restore project" { state.update(cx, |s, cx| s.restore_project(path.clone(), cx)); return; }
            if action == "Archive" { state.update(cx, |s, cx| s.archive_project(path.clone(), false, cx)); return; }
            if action == "Rename" {
                state.update(cx, |s, cx| s.start_project_rename(path.clone(), cx));
                return;
            }
            let state = state.clone(); let path = path.clone();
            window.open_dialog(cx, move |dialog, _, _| {
                let state = state.clone(); let path = path.clone();
                dialog.title("Permanently delete project?")
                    .bg(crate::appearance::popup_surface())
                    .entrance(gpui_kit::component::dialog::DialogEntrance::FadeSlide)
                    .footer(gpui_kit::component::dialog::DialogFooter::new()
                        .child(Button::new("project-cancel").label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("project-confirm").label("Permanently delete").danger()
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(gpui_kit::component::dialog::Confirm { secondary: false }), cx))))
                    .child(div().child(if archived_only { "Permanently delete this project’s archived session histories. Active sessions, your directory, and files will stay untouched." } else { "Permanently delete this project and all its session histories. Your directory and files will stay untouched." }))
                    .on_ok(move |_, _, cx| {
                        state.update(cx, |s, cx| s.archive_project(path.clone(), true, cx));
                        true
                    })
            });
        }))
}

fn session_menu(
    menu: PopupMenu,
    state: &Entity<AppState>,
    id: &del_agent::SessionId,
    pinned: bool,
    cx: &mut App,
) -> PopupMenu {
    let motion = crate::hover_motion::HoverMotion::menu(cx);
    let click_motion = motion.clone();
    let state = state.clone();
    let id = id.clone();
    menu.item(
        PopupMenuItem::element(move |_, cx| session_menu_content(&motion, pinned, cx)).on_click(
            move |event, _, cx| {
                let Some(action) = click_motion.clicked(event) else {
                    return;
                };
                state.update(cx, |s, cx| match action.as_str() {
                    "Rename" => s.start_rename(id.clone(), cx),
                    "Pin" | "Unpin" => s.toggle_session_pin(&id, cx),
                    "Mark as unread" => s.mark_session_unread(&id, cx),
                    "Unload" => s.unload(id.clone(), cx),
                    "Archive" => s.archive(id.clone(), cx),
                    "Permanently delete" => s.permanently_delete(id.clone(), cx),
                    _ => {}
                });
            },
        ),
    )
}

pub fn render(
    state: &Entity<AppState>,
    rename: &Entity<InputState>,
    usage: &Entity<crate::usage::Usage>,
    updater: &Entity<crate::app_updates::AppUpdater>,
    cx: &App,
) -> AnyElement {
    let s = state.read(cx);
    let mut rows = s.unified_rows();
    // Archived rows are only cloned while the archive is shown.
    let archived = if s.show_archived {
        s.archived_rows()
    } else {
        Vec::new()
    };
    let archived_terminals: Vec<_> = s
        .terminal_rows
        .iter()
        .filter(|row| s.terminal_archived(&row.id))
        .collect();
    let active = match &s.selection {
        Selection::Session(id) | Selection::Terminal(id) => Some(id.clone()),
        Selection::Draft(_) => None,
    };
    let providers_open = s.panel == Panel::Providers;
    let show_archived = s.show_archived;
    let has_archived = s.has_archived_chats() || !archived_terminals.is_empty();
    if show_archived {
        rows.clear();
        rows.extend(archived.iter().cloned());
        rows.extend(archived_terminals.iter().map(|r| Row {
            id: r.id.clone(),
            provider: r.provider.unwrap_or(del_agent::Provider::Claude),
            model: None,
            cwd: r.cwd.clone(),
            title: s.current_title(&r.id),
            status: del_agent::SessionStatus::Idle,
            updated_secs: 0,
            archived: true,
            pinned: false,
            unread: false,
        }));
    }

    let pinned_state = state.clone();
    let section_state = state.clone();
    let section = if show_archived {
        "Archived"
    } else {
        "Projects"
    };
    let pinned_open = !s.project_preferences.collapsed_sections.contains("Pinned");
    let projects_open = !s.project_preferences.collapsed_sections.contains(section);
    let add_project_state = state.clone();
    let new_state = state.clone();
    let prov_state = state.clone();
    let caffeine_state = state.clone();
    let arch_state = state.clone();
    v_flex()
        .w_full()
        .min_w_0()
        .whitespace_nowrap()
        .h_full()
        .flex_none()
        .gap_0p5()
        .pl_2()
        .pb_2()
        .child(
            toolbar_row()
                .child(
                    toolbar_button("search-conversations", cx)
                        .tooltip("Search conversations (⌘K)")
                        .accessibility_label("Search conversations")
                        .child(crate::icons::icon(IconName::Search))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::SearchConversations), cx)
                        }),
                )
                .child(
                    toolbar_button("new-session", cx)
                        .tooltip("New session (⌘N)")
                        .accessibility_label("New session")
                        .child(crate::icons::icon(IconName::Plus))
                        .on_click(move |_, _, cx| new_state.update(cx, |s, cx| s.new_session(cx))),
                )
                .child(
                    toolbar_button("caffeine-toggle", cx)
                        .child(coffee_icon(s.caffeine_enabled, s.window_active, cx))
                        .tooltip(if s.caffeine_enabled {
                            "Allow Mac to sleep"
                        } else {
                            "Keep Mac awake"
                        })
                        .accessibility_label(if s.caffeine_enabled {
                            "Keep Mac awake: on"
                        } else {
                            "Keep Mac awake: off"
                        })
                        .text_color(rgba(MUTED))
                        .on_click(move |_, _, cx| {
                            caffeine_state.update(cx, |s, cx| s.toggle_caffeine(cx))
                        }),
                )
                .child(
                    gpui_kit::component::popover::Popover::new("session-usage")
                        .trigger_style(toolbar_trigger_style())
                        .open(usage.read(cx).open)
                        .arrow(true)
                        .anchor(Anchor::TopLeft)
                        .on_open_change({
                            let usage = usage.clone();
                            move |open, _, cx| {
                                usage.update(cx, |usage, cx| usage.set_open(*open, cx))
                            }
                        })
                        .trigger(
                            toolbar_button("session-usage-trigger", cx)
                                .w_full()
                                .flex_none()
                                .tooltip("Memory usage")
                                .accessibility_label("Memory usage")
                                .child(crate::icons::icon(ActionIcon::Activity)),
                        )
                        .content({
                            let usage = usage.clone();
                            move |_, _, _| usage.clone()
                        }),
                ),
        )
        .child(
            v_flex()
                .id("session-list")
                .relative()
                .child(s.sidebar_hover.layer(None, rgba(0xffffff00)))
                .flex_1()
                .min_h_0()
                .gap_0p5()
                .overflow_y_scroll()
                .when(rows.iter().any(|r| r.pinned && !r.archived), |list| list.child(
                    gpui_kit::component::collapsible::Collapsible::new()
                        .motion_id("pinned-section-reveal").open(pinned_open).flex_none()
                        .child(div().id("pinned-section-heading").debug_selector(|| "pinned-heading".into())
                            .role(Role::Button).aria_label(if pinned_open { "Collapse Pinned" } else { "Expand Pinned" })
                            .px_2().pt_2().pb_1().text_size(zpx(TEXT_SM)).text_color(rgba(FAINT)).cursor_pointer()
                            .hover(|heading| heading.text_color(rgba(MUTED))).child("Pinned")
                            .on_click(move |_, _, cx| pinned_state.update(cx, |s, cx| s.toggle_sidebar_section("Pinned", cx))))
                        .content(v_flex().w_full().gap_0p5()
                            .children(rows.iter().filter(|r| r.pinned && !r.archived).map(|row| row_item(row, active.as_ref() == Some(&row.id) && !providers_open, s.renaming.as_ref() == Some(&row.id), rename, state, false, cx))))))
                .child(gpui_kit::component::collapsible::Collapsible::new()
                    .motion_id(SharedString::from(format!("{section}-section-reveal"))).open(projects_open).flex_none()
                    .child(h_flex().id("projects-section-heading").debug_selector(|| "projects-heading".into())
                        .role(Role::Button).aria_label(format!("{} {section}", if projects_open { "Collapse" } else { "Expand" }))
                        .px_2().pt_0().pb_1().items_center().text_size(zpx(TEXT_SM)).text_color(rgba(FAINT)).cursor_pointer()
                        .hover(|heading| heading.text_color(rgba(MUTED)))
                        .on_click(move |_, _, cx| section_state.update(cx, |s, cx| s.toggle_sidebar_section(section, cx)))
                        .child(div().flex_1().child(section))
                        .when(!show_archived, |heading| heading.child(Button::new("add-project").ghost().compact().icon(IconName::Plus).tooltip("Add project directory").accessibility_label("Add project directory")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| { cx.stop_propagation(); add_project_state.update(cx, |s, cx| s.add_project(cx)); }))))
                    .content(v_flex().w_full().gap_0p5()
                .children(s.project_paths_for(rows.iter().map(|row| row.cwd.as_path())).into_iter().map(|path| {
                    let collapsed = s.project_preferences.collapsed.contains(&path);
                    let project_renaming = s.project_renaming.as_ref() == Some(&path);
                    let toggle_state = state.clone();
                    let toggle_path = path.clone();
                    let new_state = state.clone();
                    let new_path = path.clone();
                    let menu_state = state.clone();
                    let menu_path = path.clone();
                    let cleanup_state = state.clone();
                    let cleanup_path = path.clone();
                    let group = SharedString::from(format!("project-{}", path.display()));
                    gpui_kit::component::collapsible::Collapsible::new()
                        .motion_id(SharedString::from(format!("project-reveal-{}", path.display())))
                        .open(!collapsed).flex_none().gap_0p5()
                        .child(h_flex().id(group.clone()).group(group.clone()).h(zpx(32.)).px_2().gap_2().items_center().rounded_md().cursor_pointer()
                            .text_color(rgba(0xffffffcc)).text_size(zpx(TEXT_MD))
                            .hover(|row| row.bg(rgba(0xffffff0a)))
                            .aria_label(format!("{} {}", if collapsed { "Expand" } else { "Collapse" }, path.display()))
                            .on_click(move |_, _, cx| { if !project_renaming { toggle_state.update(cx, |s, cx| s.toggle_project(toggle_path.clone(), cx)); } })
                            .context_menu(move |menu, _, _| {
                                let state = cleanup_state.clone(); let path = cleanup_path.clone();
                                menu.item(PopupMenuItem::new("Remove unused clean worktrees").on_click(move |_, _, cx| state.update(cx, |s, cx| s.cleanup_project_worktrees(path.clone(), cx))))
                            })
                            .child(if collapsed { crate::icons::icon(IconName::Folder) } else { crate::icons::path("icons/project-folder-open.svg") })
                            .child(div().flex_1().min_w_0().child(if project_renaming {
                                Input::new(rename).appearance(false).bordered(false).text_size(zpx(TEXT_MD)).into_any_element()
                            } else { fading_title(s.project_name(&path)).into_any_element() }))
                            .child(Button::new(SharedString::from(format!("project-menu-{}", path.display()))).ghost().compact().icon(crate::icons::path("icons/more-horizontal.svg"))
                                .accessibility_label("Project actions").opacity(0.).group_hover(group.clone(), |button| button.opacity(1.))
                                .dropdown_menu(move |menu, _, cx| project_menu(menu, &menu_state, &menu_path, cx)))
                            .when(!show_archived, |row| row.child(Button::new(SharedString::from(format!("project-new-{}", path.display()))).ghost().compact().icon(IconName::Plus)
                                .opacity(0.).group_hover(group.clone(), |button| button.opacity(1.))
                                .tooltip(format!("New session in {}", path.display())).accessibility_label("New session in project")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| { cx.stop_propagation(); new_state.update(cx, |s, cx| s.new_project_session(new_path.clone(), cx)); }))))
                        .content(v_flex().w_full().debug_selector(|| "project-session-content".into()).gap_0p5()
                            .children(s.project_drafts(&path).into_iter().filter(|_| !show_archived).map(|key| {
                                let state = state.clone();
                                div().id(SharedString::from(format!("project-draft-{key}"))).ml(zpx(18.)).px_2().h(zpx(ROW_HEIGHT)).flex().items_center().rounded_md()
                                    .text_size(zpx(TEXT_MD)).text_color(rgba(MUTED)).cursor_pointer()
                                    .when(s.selection == Selection::Draft(key), |row| row.bg(rgba(0xffffff14)))
                                    .hover(|row| row.bg(rgba(0xffffff0a))).child(div().flex_1().min_w_0().truncate().child("New session"))
                                    .when(s.tab_working(&crate::state::Tab::Draft(key)), |row| row.child(crate::icons::working_spinner().color(rgba(MUTED).into())))
                                    .on_click(move |_, _, cx| state.update(cx, |s, cx| s.select_tab(&crate::state::Tab::Draft(key), cx)))
                            }))
                            .children(rows.iter().filter(|r| (!r.pinned || r.archived) && s.project_root(&r.cwd) == path).map(|row|
                                div().pl(zpx(18.)).opacity(if row.archived { 0.6 } else { 1. }).child(row_item(row, active.as_ref() == Some(&row.id) && !providers_open, s.renaming.as_ref() == Some(&row.id), rename, state, row.archived, cx)))))
                }))))
                .child(
                    div()
                        .id("sidebar-empty-space")
                        .flex_1()
                        .min_h(zpx(12.0))
                        .w_full(),
                ),
        )
        .child(crate::app_updates::render(updater, cx))
        .child(
            h_flex()
                .h(zpx(ROW_HEIGHT))
                .min_h(zpx(ROW_HEIGHT))
                .flex_none()
                .gap_1()
                .items_center()
                .child(
                    h_flex()
                        .id("settings-entry")
                        .role(gpui::Role::Button)
                        .aria_label("Settings")
                        .flex_1()
                        .h_full()
                        .px_2()
                        .items_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(zpx(TEXT_MD))
                        .text_color(if providers_open {
                            rgb(0xf5f5f5)
                        } else {
                            rgba(MUTED)
                        })
                        .when(providers_open, |r| r.bg(rgba(0xffffff14)))
                        .when(!providers_open, |r| r.hover(|r| r.bg(rgba(0xffffff0a))))
                        .on_click(move |_, _, cx| {
                            prov_state.update(cx, |s, cx| s.toggle_providers(cx))
                        })
                        .gap_1p5()
                        .child(crate::icons::icon(IconName::Settings))
                        .child("Settings"),
                )
                .when(show_archived || has_archived, |footer| footer.child(
                    Button::new("show-archived").ghost().compact().icon(ActionIcon::Archive)
                        .selected(show_archived)
                        .when(show_archived, |button| button.bg(rgba(0xffffff20)).text_color(rgb(0xffffff)))
                        .tooltip(if show_archived { "Show active sessions" } else { "Show archived sessions and projects" })
                        .accessibility_label("Toggle archived sessions")
                        .on_click(move |_, _, cx| arch_state.update(cx, |s, cx| { s.show_archived = !s.show_archived; cx.notify(); }))
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod toolbar_tests {
    use super::{toolbar_button, toolbar_row, toolbar_trigger_style};
    use crate::zoom::px as zpx;
    use gpui::{
        Context, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement, Render,
        Styled, Window, div, px, size,
    };
    use gpui_kit::component::{IconName, popover};
    use std::{cell::Cell, rc::Rc};

    struct ToolbarFixture {
        width: f32,
        open: Rc<Cell<bool>>,
    }

    impl Render for ToolbarFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let open = self.open.clone();
            let glyph = |name: &'static str, icon| {
                div()
                    .debug_selector(move || name.into())
                    .flex_none()
                    .child(crate::icons::icon(icon))
            };
            div().w(zpx(self.width)).pl_2().child(
                toolbar_row()
                    .debug_selector(|| "toolbar-row".into())
                    .child(
                        toolbar_button("toolbar-search", cx)
                            .debug_selector(|| "toolbar-search".into())
                            .child(glyph("search-glyph", IconName::Search)),
                    )
                    .child(
                        toolbar_button("toolbar-new", cx)
                            .debug_selector(|| "toolbar-new".into())
                            .child(glyph("new-glyph", IconName::Plus)),
                    )
                    .child(
                        toolbar_button("toolbar-coffee", cx)
                            .debug_selector(|| "toolbar-coffee".into())
                            .child(glyph("coffee-glyph", IconName::Settings)),
                    )
                    .child(
                        popover::Popover::new("toolbar-memory")
                            .trigger_style(toolbar_trigger_style())
                            .open(self.open.get())
                            .on_open_change(move |value, _, _| open.set(*value))
                            .trigger(
                                toolbar_button("toolbar-activity", cx)
                                    .w_full()
                                    .flex_none()
                                    .debug_selector(|| "toolbar-activity".into())
                                    .child(glyph("activity-glyph", IconName::Info)),
                            )
                            .content(|_, _, _| div().child("Memory")),
                    ),
            )
        }
    }

    #[gpui::test]
    fn toolbar_fills_row_with_equal_padded_targets_across_widths_and_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let open = Rc::new(Cell::new(false));
        let fixture_open = open.clone();
        let (root, cx) = cx.add_window_view(|_, _| ToolbarFixture {
            width: 216.,
            open: fixture_open,
        });
        for zoom in [100, 150] {
            for width in [160., 216., 360.] {
                cx.update(|window, cx| {
                    del_core::zoom::set(zoom);
                    window.set_rem_size(px(14. * zoom as f32 / 100.));
                    root.update(cx, |fixture, cx| {
                        fixture.width = width;
                        cx.notify();
                    });
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
                let row = cx.debug_bounds("toolbar-row").unwrap();
                let targets = [
                    "toolbar-search",
                    "toolbar-new",
                    "toolbar-coffee",
                    "toolbar-activity",
                ]
                .map(|name| cx.debug_bounds(name).unwrap());
                let glyphs = [
                    "search-glyph",
                    "new-glyph",
                    "coffee-glyph",
                    "activity-glyph",
                ]
                .map(|name| cx.debug_bounds(name).unwrap());
                let expected_width = (row.size.width - zpx(24.)) / 4.;
                for (target, glyph) in targets.iter().zip(glyphs) {
                    assert!(
                        (target.size.width - expected_width).abs() <= px(0.5),
                        "unequal quarter at width={width},zoom={zoom}: {target:?}, row={row:?}"
                    );
                    assert_eq!(target.size.height, zpx(super::TOOLBAR_HEIGHT));
                    assert_eq!(glyph.size, size(zpx(16.), zpx(16.)));
                    assert!((target.center().x - glyph.center().x).abs() <= px(0.5));
                    assert!((target.center().y - glyph.center().y).abs() <= px(0.5));
                }
                assert_eq!(targets[0].left(), row.left());
                assert!((targets[3].right() + zpx(6.) - row.right()).abs() <= px(0.5));
                for pair in targets.windows(2) {
                    assert!((pair[1].left() - pair[0].right() - zpx(6.)).abs() <= px(0.5));
                }
            }
        }
        let activity = cx.debug_bounds("toolbar-activity").unwrap().center();
        cx.simulate_mouse_move(activity, None, Modifiers::default());
        cx.simulate_mouse_down(activity, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(activity, MouseButton::Left, Modifiers::default());
        assert!(open.get(), "stretched popover target did not open");
        cx.update(|_, _| del_core::zoom::set(100));
    }
}

#[cfg(test)]
mod menu_tests {
    use super::{PopupMenu, PopupMenuItem, session_menu_content};
    use gpui::{
        Context, Entity, Focusable, InteractiveElement, IntoElement, Modifiers, MouseButton,
        ParentElement, Render, Styled, Window, div, point, px,
    };
    use std::{cell::RefCell, rc::Rc};

    struct MenuFixture(Entity<PopupMenu>);
    impl Render for MenuFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .w(px(220.))
                .debug_selector(|| "menu-fixture".into())
                .child(self.0.clone())
        }
    }

    #[gpui::test]
    fn shared_menu_surface_preserves_mouse_and_keyboard_actions(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let clicked = Rc::new(RefCell::new(Vec::new()));
        let recorded = clicked.clone();
        let (root, cx) = cx.add_window_view(move |window, cx| {
            window.set_rem_size(px(14.));
            let motion = crate::hover_motion::HoverMotion::menu(cx);
            let selection = motion.clone();
            let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                menu.item(
                    PopupMenuItem::element(move |_, cx| session_menu_content(&motion, false, cx))
                        .on_click(move |event, _, _| {
                            if let Some(action) = selection.clicked(event) {
                                recorded.borrow_mut().push(action);
                            }
                        }),
                )
            });
            menu.read(cx).focus_handle(cx).focus(window, cx);
            MenuFixture(menu)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let row = cx.debug_bounds("session-menu-Pin").unwrap();
        let surface = cx.debug_bounds("menu-fixture").unwrap();
        // Only the popup's 0.25rem outer padding should remain on either side.
        assert_eq!(row.left() - surface.left(), px(3.5));
        assert_eq!(surface.right() - row.right(), px(3.5));
        let position = row.center();
        cx.simulate_mouse_move(position, None, Modifiers::default());
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(&*clicked.borrow(), &["Pin"]);
        cx.simulate_mouse_move(point(px(900.), px(900.)), None, Modifiers::default());
        // A dismissed menu is inert during its exit; keyboard use opens a fresh menu.
        cx.update(|window, cx| {
            let motion = crate::hover_motion::HoverMotion::menu(cx);
            let selection = motion.clone();
            let recorded = clicked.clone();
            let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                menu.item(
                    PopupMenuItem::element(move |_, cx| session_menu_content(&motion, false, cx))
                        .on_click(move |event, _, _| {
                            if let Some(action) = selection.clicked(event) {
                                recorded.borrow_mut().push(action);
                            }
                        }),
                )
            });
            menu.read(cx).focus_handle(cx).focus(window, cx);
            root.update(cx, |root, cx| {
                root.0 = menu;
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        assert_eq!(&*clicked.borrow(), &["Pin", "Mark as unread"]);
    }
}
