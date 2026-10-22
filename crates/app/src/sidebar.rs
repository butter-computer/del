//! The sessions sidebar: new-session button, session rows, archived list and
//! the providers entry. Borderless; rows are told apart by fill alone.

use crate::appearance::{rgb, rgba};
use crate::composer_ui::provider_logo;
use crate::sessions::Row;
use crate::state::{AppState, Panel, Selection};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::assets::IconName as ActionIcon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 28.0;
const PIN_ICON_SIZE: f32 = 15.0;

const TEXT: u32 = 0xf0f0f0;
const MUTED: u32 = 0xffffff8c;
const FAINT: u32 = 0xffffff52;

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
                dot.size(px(6.0 * value)).opacity(value)
            },
        )
        .into_any_element()
    } else {
        dot.size(px(6.0 * target))
            .opacity(target)
            .into_any_element()
    };
    div()
        .size(px(6.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(dot)
        .into_any_element()
}

/// The cup stays neutral. Only the three steam wisps rise and fade, staggered
/// across a seamless loop. Reduced motion retains the yellow active indicator.
fn coffee_icon(active: bool, cx: &App) -> Div {
    div()
        .relative()
        .size(px(18.0))
        .child(
            Icon::default()
                .path("icons/coffee-cup.svg")
                .with_size(px(18.0)),
        )
        .children((0..3).map(|i| {
            let steam = div()
                .absolute()
                .left(px(5.0 + i as f32 * 3.0))
                .top(px(0.0))
                .w(px(2.0))
                .h(px(5.0))
                .text_color(if active { rgb(0xf5a524) } else { rgba(MUTED) })
                .child(
                    Icon::default()
                        .path("icons/coffee-steam.svg")
                        .w(px(2.0))
                        .h(px(5.0)),
                );
            if active && !cx.reduce_motion() {
                steam
                    .with_animation(
                        SharedString::from(format!("coffee-steam-{i}")),
                        Animation::new(std::time::Duration::from_millis(1800)).repeat(),
                        move |steam, t| {
                            let phase = (t + i as f32 / 3.0) % 1.0;
                            steam
                                .top(px(2.0 - phase * 4.0))
                                .opacity((std::f32::consts::PI * phase).sin())
                        },
                    )
                    .into_any_element()
            } else {
                steam.into_any_element()
            }
        }))
}

/// Compact hover actions retain spoken names without visible text labels.
fn action_button(id: impl Into<ElementId>, icon: ActionIcon, label: &'static str) -> Button {
    Button::new(id)
        .ghost()
        .with_size(px(20.0))
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
        .text_color(rgba(MUTED))
}

fn pin_button(id: impl Into<ElementId>, pinned: bool) -> Button {
    Button::new(id)
        .text()
        .size(px(20.0))
        .p_0()
        .tooltip(if pinned { "Unpin" } else { "Pin" })
        .accessibility_label(if pinned { "Unpin" } else { "Pin" })
        .text_color(if pinned { rgb(0xf5f5f5) } else { rgba(MUTED) })
        .child(
            Icon::default()
                .path(if pinned {
                    "icons/pin-filled.svg"
                } else {
                    "icons/pin-outline.svg"
                })
                .with_size(px(PIN_ICON_SIZE)),
        )
}

fn archive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .text()
        .size(px(20.0))
        .p_0()
        .tooltip("Archive")
        .accessibility_label("Archive")
        .text_color(rgba(MUTED))
        .child(
            Icon::default()
                .path("icons/archive.svg")
                .with_size(px(15.0)),
        )
}

fn session_menu_item(
    label: &'static str,
    icon: Icon,
    motion: &crate::hover_motion::HoverMotion,
) -> PopupMenuItem {
    let motion = motion.clone();
    PopupMenuItem::element(move |_, cx| {
        let destructive = label == "Permanently delete";
        let background = cx.theme().popover;
        let hover = motion.clone();
        h_flex()
            .id(label)
            .role(gpui::Role::MenuItem)
            .aria_label(label)
            .items_center()
            .h(px(28.0))
            .gap(px(10.0))
            .relative()
            .flex_1()
            .mx_neg_2()
            .px_2()
            .rounded_md()
            .text_size(px(TEXT_MD))
            .when(destructive, |item| item.text_color(rgb(0xff646b)))
            .on_hover(move |entered, window, cx| hover.hover(label, *entered, window, cx))
            .child(motion.background(
                label.into(),
                background.into(),
                if destructive {
                    rgba(0xff45451f)
                } else {
                    rgba(0xffffff14)
                },
            ))
            .child(
                icon.clone()
                    .with_size(px(16.0))
                    .when(destructive, |icon| icon.text_color(rgb(0xff646b))),
            )
            .child(label)
    })
}

fn pinned_mark(pinned: bool, group: SharedString, right: f32) -> AnyElement {
    div()
        .absolute()
        .right(px(right))
        .top(px((ROW_HEIGHT - 20.0) / 2.0))
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .group_hover(group, |mark| mark.opacity(0.0))
        .when(pinned, |mark| {
            mark.child(
                Icon::default()
                    .path("icons/pin-filled.svg")
                    .with_size(px(PIN_ICON_SIZE))
                    .text_color(rgba(MUTED)),
            )
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
    let id = row.id.clone();
    let key = SharedString::from(id.0.clone());
    let group = SharedString::from(format!("row-{}", id.0));

    let action_right = if state.read(cx).terminal_unread(&id) {
        20.0
    } else {
        8.0
    };
    let motion = state.read(cx).sidebar_hover.clone();
    let hover = motion.clone();
    let hover_key = id.0.clone();
    let mut item = h_flex()
        .id(key.clone())
        .relative()
        .group(group.clone())
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_1p5()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(TEXT_MD))
        .text_color(rgb(TEXT))
        .on_hover(move |entered, window, cx| hover.hover(&hover_key, *entered, window, cx))
        .child(motion.background(
            id.0.clone(),
            if selected {
                rgba(0xffffff14)
            } else {
                rgba(0xffffff00)
            },
            rgba(0xffffff0a),
        ))
        .child(provider_logo(row.provider, 14.0));

    item = if renaming {
        item.child(
            div().flex_1().min_w_0().child(
                Input::new(rename)
                    .appearance(false)
                    .bordered(false)
                    .text_size(px(TEXT_MD)),
            ),
        )
    } else {
        let state_click = state.clone();
        let click_id = id.clone();
        item.on_click(move |_, _, cx| {
            let id = click_id.clone();
            if archived {
                state_click.update(cx, |s, cx| s.restore(id, cx));
            } else {
                state_click.update(cx, |s, cx| s.select(id, cx));
            }
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(if selected {
                    rgb(0xf5f5f5)
                } else {
                    rgba(0xffffffcc)
                })
                .font_weight(FontWeight::NORMAL)
                .child(row.title.clone()),
        )
    };

    if !renaming {
        let (pin_state, archive_state) = (state.clone(), state.clone());
        let (pin_id, archive_id) = (id.clone(), id.clone());
        let hover_group = group.clone();
        item = item
            .child(pinned_mark(row.pinned, group.clone(), action_right))
            .child(
                // Reserve space so titles never overlap the hover actions.
                div().flex_none().w(px(44.0)),
            )
            .child(unread_dot(&row.id, state.read(cx), cx))
            .child(
                // Always laid out and painted, only faded in on hover: toggling
                // `display` under the pointer made GPUI paint children it had
                // not prepainted, which aborted the app.
                h_flex()
                    .absolute()
                    .right(px(action_right))
                    .top(px((ROW_HEIGHT - 20.0) / 2.0))
                    .h(px(20.0))
                    .items_center()
                    .gap(px(4.0))
                    .when(!archived, |actions| actions.w(px(44.0)))
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
                    .when(archived, |a| {
                        a.child(
                            div()
                                .text_size(px(TEXT_SM))
                                .text_color(rgba(MUTED))
                                .child("Restore"),
                        )
                    }),
            );

        if !archived {
            let state = state.clone();
            let id = id.clone();
            let pinned = row.pinned;
            return item
                .context_menu(move |menu, _, cx| session_menu(menu, &state, &id, pinned, cx))
                .into_any_element();
        }
    }
    item.into_any_element()
}

fn session_menu(
    menu: PopupMenu,
    state: &Entity<AppState>,
    id: &del_agent::SessionId,
    pinned: bool,
    cx: &mut App,
) -> PopupMenu {
    let motion = crate::hover_motion::HoverMotion::menu(cx);
    let rename_state = state.clone();
    let pin_state = state.clone();
    let unread_state = state.clone();
    let archive_state = state.clone();
    let delete_state = state.clone();
    let rename_id = id.clone();
    let pin_id = id.clone();
    let unread_id = id.clone();
    let archive_id = id.clone();
    let delete_id = id.clone();
    menu.item(
        session_menu_item("Rename", Icon::new(ActionIcon::Pencil), &motion).on_click({
            let state = rename_state.clone();
            let id = rename_id.clone();
            move |_, _, cx| state.update(cx, |s, cx| s.start_rename(id.clone(), cx))
        }),
    )
    .item(
        session_menu_item(
            if pinned { "Unpin" } else { "Pin" },
            Icon::default().path(if pinned {
                "icons/pin-filled.svg"
            } else {
                "icons/pin-outline.svg"
            }),
            &motion,
        )
        .on_click({
            let state = pin_state.clone();
            let id = pin_id.clone();
            move |_, _, cx| state.update(cx, |s, cx| s.toggle_session_pin(&id, cx))
        }),
    )
    .item(
        session_menu_item("Mark as unread", Icon::new(ActionIcon::Eye), &motion).on_click({
            let state = unread_state.clone();
            let id = unread_id.clone();
            move |_, _, cx| state.update(cx, |s, cx| s.mark_session_unread(&id, cx))
        }),
    )
    .separator()
    .item(
        session_menu_item(
            "Archive",
            Icon::default().path("icons/archive.svg"),
            &motion,
        )
        .on_click({
            let state = archive_state.clone();
            let id = archive_id.clone();
            move |_, _, cx| state.update(cx, |s, cx| s.archive(id.clone(), cx))
        }),
    )
    .item(
        session_menu_item("Permanently delete", Icon::new(ActionIcon::Trash), &motion).on_click({
            let state = delete_state.clone();
            let id = delete_id.clone();
            move |_, _, cx| state.update(cx, |s, cx| s.permanently_delete(id.clone(), cx))
        }),
    )
}

pub fn render(state: &Entity<AppState>, rename: &Entity<InputState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let rows = s.rows();
    let archived = s.archived_rows();
    let archived_terminals: Vec<_> = s
        .terminal_rows
        .iter()
        .filter(|row| s.terminal_archived(&row.id))
        .collect();
    let active = match &s.selection {
        Selection::Session(id) => Some(id.clone()),
        Selection::Draft(_) | Selection::Terminal(_) => None,
    };
    let draft_selected = matches!(s.selection, Selection::Draft(_)) && s.panel == Panel::Chat;
    let providers_open = s.panel == Panel::Providers;
    let show_archived = s.show_archived;

    let new_state = state.clone();
    let prov_state = state.clone();
    let caffeine_state = state.clone();
    let arch_state = state.clone();

    v_flex()
        .w_full()
        .h_full()
        .flex_none()
        .gap_0p5()
        .pl_2()
        .pb_2()
        .child(
            h_flex()
                .id("new-session")
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_1p5()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(TEXT_MD))
                .text_color(rgb(TEXT))
                .when(draft_selected, |r| r.bg(rgba(0xffffff14)))
                .when(!draft_selected, |r| r.hover(|r| r.bg(rgba(0xffffff0a))))
                .on_click(move |_, _, cx| new_state.update(cx, |s, cx| s.new_session(cx)))
                .child(Icon::new(IconName::Plus).small().text_color(rgba(MUTED)))
                .child(div().flex_1().child("New session"))
                .child(
                    div()
                        .text_size(px(TEXT_SM))
                        .text_color(rgba(FAINT))
                        .child("⌘N"),
                ),
        )
        .child(
            v_flex()
                .id("session-list")
                .flex_1()
                .min_h_0()
                .gap_0p5()
                .overflow_y_scroll()
                .children(rows.iter().map(|row| {
                    let selected = active.as_ref() == Some(&row.id) && !providers_open;
                    let renaming = s.renaming.as_ref() == Some(&row.id);
                    let animation_id = SharedString::from(format!("session-row-{}", row.id.0));
                    div()
                        .overflow_hidden()
                        .child(row_item(row, selected, renaming, rename, state, false, cx))
                        .with_animation(
                            animation_id,
                            Animation::new(std::time::Duration::from_millis(140))
                                .with_easing(ease_out_quint()),
                            |row, t| row.h(px(ROW_HEIGHT * t)).opacity(t),
                        )
                }))
                .child(
                    div()
                        .mt_3()
                        .px_2()
                        .text_size(px(TEXT_SM))
                        .text_color(rgba(FAINT))
                        .child("TERMINALS"),
                )
                .children(
                    s.terminal_rows
                        .iter()
                        .filter(|row| !s.terminal_archived(&row.id))
                        .map(|row| {
                            let menu_state = state.clone();
                            let menu_id = row.id.clone();
                            let pinned = row.pinned;
                            let renaming = s.renaming.as_ref() == Some(&row.id);
                            let title = s.current_title(&row.id);
                            let title_loading = s.title_loading(&row.id);
                            let selected = s.selection == Selection::Terminal(row.id.clone())
                                && !providers_open;
                            let state = state.clone();
                            let pin_state = state.clone();
                            let close_state = state.clone();
                            let id = row.id.clone();
                            let pin_id = row.id.clone();
                            let close_id = row.id.clone();
                            let group = SharedString::from(format!("terminal-actions-{}", id.0));
                            let action_right = if s.terminal_unread(&id) { 20.0 } else { 8.0 };
                            let motion = s.sidebar_hover.clone();
                            let hover = motion.clone();
                            let hover_key = id.0.clone();
                            h_flex()
                                .id(SharedString::from(format!("terminal-row-{}", id.0)))
                                .relative()
                                .group(group.clone())
                                .h(px(ROW_HEIGHT))
                                .px_2()
                                .gap_2()
                                .rounded_md()
                                .cursor_pointer()
                                .on_hover(move |entered, window, cx| {
                                    hover.hover(&hover_key, *entered, window, cx)
                                })
                                .child(motion.background(
                                    row.id.0.clone(),
                                    if selected {
                                        rgba(0xffffff14)
                                    } else {
                                        rgba(0xffffff00)
                                    },
                                    rgba(0xffffff0a),
                                ))
                                .on_click(move |_, _, cx| {
                                    if !renaming {
                                        state.update(cx, |s, cx| s.select_terminal(id.clone(), cx));
                                    }
                                })
                                .child(match row.provider {
                                    Some(provider) => {
                                        provider_logo(provider, 14.0).into_any_element()
                                    }
                                    None => div()
                                        .text_size(px(TEXT_SM))
                                        .text_color(rgba(MUTED))
                                        .child(">_")
                                        .into_any_element(),
                                })
                                .child(if renaming {
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            Input::new(rename)
                                                .appearance(false)
                                                .bordered(false)
                                                .text_size(px(TEXT_MD)),
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(TEXT_MD))
                                        .font_weight(FontWeight::NORMAL)
                                        .child(if title_loading {
                                            gpui_kit::component::shimmer::ShimmerText::new(title)
                                                .id(SharedString::from(format!(
                                                    "sidebar-title-{}",
                                                    row.id.0
                                                )))
                                                .into_any_element()
                                        } else {
                                            div().truncate().child(title).into_any_element()
                                        })
                                        .into_any_element()
                                })
                                .child(div().flex_none().w(px(44.0)))
                                .child(unread_dot(&row.id, s, cx))
                                .child(pinned_mark(row.pinned, group.clone(), action_right + 24.0))
                                .child(
                                    h_flex()
                                        .absolute()
                                        .right(px(action_right))
                                        .top(px((ROW_HEIGHT - 20.0) / 2.0))
                                        .h(px(20.0))
                                        .w(px(44.0))
                                        .items_center()
                                        .gap(px(4.0))
                                        .rounded_sm()
                                        .opacity(0.0)
                                        .group_hover(group, |actions| actions.opacity(1.0))
                                        .child(
                                            pin_button(
                                                SharedString::from(format!(
                                                    "pin-terminal-{}",
                                                    pin_id.0
                                                )),
                                                row.pinned,
                                            )
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(
                                                move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    pin_state.update(cx, |s, cx| {
                                                        s.toggle_terminal_pin(&pin_id, cx)
                                                    });
                                                },
                                            ),
                                        )
                                        .child(
                                            action_button(
                                                SharedString::from(format!(
                                                    "close-terminal-{}",
                                                    close_id.0
                                                )),
                                                ActionIcon::Close,
                                                "Close terminal",
                                            )
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(
                                                move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    close_state.update(cx, |s, cx| {
                                                        s.remove_terminal(&close_id, cx)
                                                    });
                                                },
                                            ),
                                        ),
                                )
                                .context_menu(move |menu, _, cx| {
                                    session_menu(menu, &menu_state, &menu_id, pinned, cx)
                                })
                        }),
                )
                .when(
                    !archived.is_empty() || !archived_terminals.is_empty(),
                    |list| {
                        list.child(
                            h_flex()
                                .id("archived-toggle")
                                .h(px(ROW_HEIGHT))
                                .px_2()
                                .mt_1()
                                .items_center()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(px(TEXT_SM))
                                .text_color(rgba(FAINT))
                                .hover(|r| r.bg(rgba(0xffffff0a)))
                                .on_click(move |_, _, cx| {
                                    arch_state.update(cx, |s, cx| {
                                        s.show_archived = !s.show_archived;
                                        cx.notify();
                                    })
                                })
                                .child(format!(
                                    "Archived ({})",
                                    archived.len() + archived_terminals.len()
                                )),
                        )
                        .when(show_archived, |list| {
                            list.children(
                                archived.iter().map(|row| {
                                    row_item(row, false, false, rename, state, true, cx)
                                }),
                            )
                            .children(archived_terminals.iter().map(|row| {
                                let state = state.clone();
                                let id = row.id.clone();
                                h_flex()
                                    .id(SharedString::from(format!("archived-terminal-{}", id.0)))
                                    .h(px(ROW_HEIGHT))
                                    .px_2()
                                    .gap_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|row| row.bg(rgba(0xffffff0a)))
                                    .child(div().flex_1().truncate().child(s.current_title(&id)))
                                    .child(
                                        div()
                                            .text_size(px(TEXT_SM))
                                            .text_color(rgba(MUTED))
                                            .child("Restore"),
                                    )
                                    .on_click(move |_, _, cx| {
                                        state.update(cx, |s, cx| s.restore(id.clone(), cx))
                                    })
                            }))
                        })
                    },
                )
                .child(
                    div()
                        .id("sidebar-window-drag")
                        .flex_1()
                        .min_h(px(12.0))
                        .w_full()
                        .window_control_area(WindowControlArea::Drag)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        }),
                ),
        )
        .child(
            h_flex()
                .h(px(ROW_HEIGHT))
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
                        .text_size(px(TEXT_MD))
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
                        .child("Settings"),
                )
                .child(
                    Button::new("caffeine-toggle")
                        .ghost()
                        .size(px(28.0))
                        .p_0()
                        .child(coffee_icon(s.caffeine_enabled, cx))
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
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            caffeine_state.update(cx, |s, cx| s.toggle_caffeine(cx));
                        }),
                ),
        )
}
