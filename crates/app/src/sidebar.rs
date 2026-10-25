//! The sessions sidebar: new-session button, session rows, archived list and
//! the providers entry. Borderless; rows are told apart by fill alone.

use crate::appearance::{rgb, rgba};
use crate::composer_ui::provider_logo;
use crate::sessions::Row;
use crate::state::{AppState, Panel, Selection};
use crate::zoom::px as zpx;
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
pub const CONTROL_ICON_SIZE: f32 = 15.0;

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
                    let fade = zpx(18.).min(bounds.size.width);
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
                        for step in 0..18 {
                            let start = solid + fade * (step as f32 / 18.);
                            let end = solid + fade * ((step + 1) as f32 / 18.);
                            let clip = Bounds::new(
                                point(bounds.left() + start, bounds.top()),
                                size(end - start, bounds.size.height),
                            );
                            let decoration = DecorationRun {
                                len: line.len() as u32,
                                color: style.color.opacity(1. - (step as f32 + 0.5) / 18.),
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

/// The cup stays neutral. Only the three steam wisps rise and fade, staggered
/// across a seamless loop. Reduced motion retains the yellow active indicator.
fn coffee_icon(active: bool, cx: &App) -> Div {
    div()
        .relative()
        .size(zpx(16.0))
        .child(
            Icon::default()
                .path("icons/coffee-cup.svg")
                .with_size(zpx(16.0)),
        )
        .children((0..3).map(|i| {
            let steam = div()
                .absolute()
                .left(zpx(4.0 + i as f32 * 2.7))
                .top(zpx(0.0))
                .w(zpx(2.0))
                .h(zpx(5.0))
                .text_color(if active { rgb(0xf5a524) } else { rgba(MUTED) })
                .child(
                    Icon::default()
                        .path("icons/coffee-steam.svg")
                        .w(zpx(2.0))
                        .h(zpx(5.0)),
                );
            if active && !cx.reduce_motion() {
                steam
                    .with_animation(
                        SharedString::from(format!("coffee-steam-{i}")),
                        Animation::new(std::time::Duration::from_millis(1800)).repeat(),
                        move |steam, t| {
                            let phase = (t + i as f32 / 3.0) % 1.0;
                            steam
                                .top(zpx(2.0 - phase * 4.0))
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
        .with_size(zpx(20.0))
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
        .text_color(rgba(MUTED))
}

fn pin_button(id: impl Into<ElementId>, pinned: bool) -> Button {
    Button::new(id)
        .text()
        .size(zpx(20.0))
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
                .with_size(zpx(CONTROL_ICON_SIZE)),
        )
}

fn archive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .text()
        .size(zpx(20.0))
        .p_0()
        .tooltip("Archive")
        .accessibility_label("Archive")
        .text_color(rgba(MUTED))
        .child(
            Icon::default()
                .path("icons/archive.svg")
                .with_size(zpx(CONTROL_ICON_SIZE)),
        )
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
                .size(zpx(20.0))
                .p_0()
                .tooltip("Restore")
                .accessibility_label("Restore")
                .text_color(rgba(MUTED))
                .child(
                    Icon::default()
                        .path("icons/restore.svg")
                        .with_size(zpx(CONTROL_ICON_SIZE)),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    restore_state.update(cx, |s, cx| s.restore(restore_id.clone(), cx));
                }),
        )
        .child(
            Button::new(SharedString::from(format!("delete-{}", id.0)))
                .ghost()
                .with_size(zpx(20.0))
                .tooltip("Permanently delete")
                .accessibility_label("Permanently delete")
                .child(
                    Icon::new(ActionIcon::Trash)
                        .with_size(zpx(CONTROL_ICON_SIZE))
                        .text_color(rgb(0xff646b)),
                )
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
            icon.with_size(zpx(16.))
                .when(destructive, |icon| icon.text_color(rgb(0xff646b))),
        )
        .child(label)
}

fn pinned_mark(pinned: bool, group: SharedString, right: f32) -> AnyElement {
    div()
        .absolute()
        .right(zpx(right))
        .top(zpx((ROW_HEIGHT - 20.0) / 2.0))
        .size(zpx(20.0))
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .group_hover(group, |mark| mark.opacity(0.0))
        .when(pinned, |mark| {
            mark.child(
                Icon::default()
                    .path("icons/pin-filled.svg")
                    .with_size(zpx(CONTROL_ICON_SIZE))
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
        .child(motion.marker(
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
                    s.select(id.clone(), cx);
                }
                if event.click_count() == 2 {
                    s.keep_tab(&crate::state::Tab::Session(id), cx);
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
                    gpui_kit::component::shimmer::ShimmerText::new(row.title.clone())
                        .id(SharedString::from(format!("sidebar-title-{}", id.0)))
                        .into_any_element()
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
            .child(pinned_mark(row.pinned, group.clone(), action_right))
            .child(
                // Reserve space so titles never overlap the hover actions.
                div().flex_none().w(zpx(if motion.is_hovered(&row.id.0) {
                    44.0
                } else if row.pinned {
                    20.0
                } else {
                    0.0
                })),
            )
            .child(unread_dot(&row.id, state.read(cx), cx))
            .child(
                // Always laid out and painted, only faded in on hover: toggling
                // `display` under the pointer made GPUI paint children it had
                // not prepainted, which aborted the app.
                h_flex()
                    .absolute()
                    .right(zpx(action_right))
                    .top(zpx((ROW_HEIGHT - 20.0) / 2.0))
                    .h(zpx(20.0))
                    .items_center()
                    .gap(zpx(4.0))
                    .w(zpx(44.0))
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
            return item
                .context_menu(move |menu, _, cx| session_menu(menu, &state, &id, pinned, cx))
                .into_any_element();
        }
    }
    item.into_any_element()
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
            Icon::new(ActionIcon::Pencil),
            motion,
        ))
        .child(session_menu_item(
            if pinned { "Unpin" } else { "Pin" },
            Icon::default().path(if pinned {
                "icons/pin-filled.svg"
            } else {
                "icons/pin-outline.svg"
            }),
            motion,
        ))
        .child(session_menu_item(
            "Mark as unread",
            Icon::new(ActionIcon::Eye),
            motion,
        ))
        .child(session_menu_item(
            "Unload",
            Icon::new(ActionIcon::Close),
            motion,
        ))
        .child(motion.divider())
        .child(session_menu_item(
            "Archive",
            Icon::default().path("icons/archive.svg"),
            motion,
        ))
        .child(session_menu_item(
            "Permanently delete",
            Icon::new(ActionIcon::Trash),
            motion,
        ))
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
    cx: &App,
) -> impl IntoElement {
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
    let terminal_state = state.clone();
    // Shared with the highlight markers: the group canvas lives outside this
    // animated section, so it needs the exact opacity sampled for this frame.
    let terminal_opacity = std::rc::Rc::new(std::cell::Cell::new(1.0_f32));

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
            h_flex()
                .w_full()
                .gap_1p5()
                .py_1()
                .flex_none()
                .child(
                    Button::new("search-conversations")
                        .ghost()
                        .flex_none()
                        .size(zpx(34.))
                        .rounded(zpx(9.))
                        .p_0()
                        .bg(rgb(0x303033))
                        .tooltip("Search conversations (⌘K)")
                        .accessibility_label("Search conversations")
                        .child(Icon::new(IconName::Search).with_size(zpx(15.)))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::SearchConversations), cx)
                        }),
                )
                .child(
                    Button::new("new-session")
                        .ghost()
                        .flex_none()
                        .size(zpx(34.))
                        .rounded(zpx(9.))
                        .p_0()
                        .bg(rgb(if draft_selected { 0x3b3b3e } else { 0x303033 }))
                        .tooltip("New session (⌘N)")
                        .accessibility_label("New session")
                        .child(Icon::new(IconName::Plus).with_size(zpx(16.)))
                        .on_click(move |_, _, cx| new_state.update(cx, |s, cx| s.new_session(cx))),
                )
                .child(
                    Button::new("caffeine-toggle")
                        .ghost()
                        .flex_none()
                        .size(zpx(34.))
                        .rounded(zpx(9.))
                        .p_0()
                        .bg(if s.caffeine_enabled {
                            let hover: Background = cx.theme().tokens.accent.into();
                            if cx.theme().mode.is_dark() {
                                hover.opacity(0.5)
                            } else {
                                hover
                            }
                        } else {
                            rgb(0x303033).into()
                        })
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
                        .on_click(move |_, _, cx| {
                            caffeine_state.update(cx, |s, cx| s.toggle_caffeine(cx))
                        }),
                )
                .child(
                    gpui_kit::component::popover::Popover::new("session-usage")
                        .open(usage.read(cx).open)
                        .arrow(true)
                        .anchor(Anchor::LeftCenter)
                        .on_open_change({
                            let usage = usage.clone();
                            move |open, _, cx| {
                                usage.update(cx, |usage, cx| usage.set_open(*open, cx))
                            }
                        })
                        .trigger(
                            Button::new("session-usage-trigger")
                                .ghost()
                                .flex_none()
                                .size(zpx(34.))
                                .rounded(zpx(9.))
                                .p_0()
                                .bg(rgb(0x303033))
                                .tooltip("Session memory")
                                .accessibility_label("Session memory")
                                .child(Icon::new(ActionIcon::Activity).with_size(zpx(16.))),
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
                .children(rows.iter().map(|row| {
                    let selected = active.as_ref() == Some(&row.id) && !providers_open;
                    let renaming = s.renaming.as_ref() == Some(&row.id);
                    let animation_id = SharedString::from(format!("session-row-{}", row.id.0));
                    div()
                        .flex_none()
                        .overflow_hidden()
                        .child(row_item(row, selected, renaming, rename, state, false, cx))
                        .with_animation(
                            animation_id,
                            Animation::new(std::time::Duration::from_millis(140))
                                .with_easing(ease_out_quint()),
                            |row, t| row.h(zpx(ROW_HEIGHT * t)).opacity(t),
                        )
                }))
                .child(
                    div()
                        .id("terminals-toggle")
                        .flex_none()
                        .min_h(zpx(ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .role(gpui::Role::Button)
                        .aria_label(if s.terminals_collapsed {
                            "Expand terminals"
                        } else {
                            "Collapse terminals"
                        })
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            terminal_state.update(cx, |s, cx| s.toggle_terminals(cx))
                        })
                        .mt_3()
                        .px_2()
                        .text_size(zpx(TEXT_SM))
                        .text_color(rgba(FAINT))
                        .child("TERMINALS"),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .gap_0p5()
                        .overflow_hidden()
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
                                    let selected = s.selection
                                        == Selection::Terminal(row.id.clone())
                                        && !providers_open;
                                    let state = state.clone();
                                    let pin_state = state.clone();
                                    let close_state = state.clone();
                                    let id = row.id.clone();
                                    let pin_id = row.id.clone();
                                    let close_id = row.id.clone();
                                    let group =
                                        SharedString::from(format!("terminal-actions-{}", id.0));
                                    let action_right =
                                        if s.terminal_unread(&id) { 20.0 } else { 8.0 };
                                    let motion = s.sidebar_hover.clone();
                                    let hover = motion.clone();
                                    let hover_key = id.0.clone();
                                    h_flex()
                                        .id(SharedString::from(format!("terminal-row-{}", id.0)))
                                        .relative()
                                        .group(group.clone())
                                        .h(zpx(ROW_HEIGHT))
                                        .min_h(zpx(ROW_HEIGHT))
                                        .flex_none()
                                        .px_2()
                                        .gap_2()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .on_hover(move |entered, window, cx| {
                                            hover.hover(&hover_key, *entered, window, cx)
                                        })
                                        .child(motion.marker_with_opacity(
                                            row.id.0.clone(),
                                            if selected {
                                                rgba(0xffffff14)
                                            } else {
                                                rgba(0xffffff00)
                                            },
                                            rgba(0xffffff0a),
                                            Some(terminal_opacity.clone()),
                                        ))
                                        .on_click(move |_, _, cx| {
                                            if !renaming {
                                                state.update(cx, |s, cx| {
                                                    s.select_terminal(id.clone(), cx)
                                                });
                                            }
                                        })
                                        .child(match row.provider {
                                            Some(provider) => {
                                                provider_logo(provider, 14.0).into_any_element()
                                            }
                                            None => div()
                                                .text_size(zpx(TEXT_SM))
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
                                                        .text_size(zpx(TEXT_MD)),
                                                )
                                                .into_any_element()
                                        } else {
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_size(zpx(TEXT_MD))
                                                .font_weight(FontWeight::NORMAL)
                                                .child(if title_loading {
                                                    gpui_kit::component::shimmer::ShimmerText::new(
                                                        title,
                                                    )
                                                    .id(SharedString::from(format!(
                                                        "sidebar-title-{}",
                                                        row.id.0
                                                    )))
                                                    .into_any_element()
                                                } else {
                                                    fading_title(title).into_any_element()
                                                })
                                                .into_any_element()
                                        })
                                        .child(div().flex_none().w(zpx(
                                            if motion.is_hovered(&row.id.0) {
                                                44.0
                                            } else if row.pinned {
                                                20.0
                                            } else {
                                                0.0
                                            },
                                        )))
                                        .child(unread_dot(&row.id, s, cx))
                                        .child(pinned_mark(row.pinned, group.clone(), action_right))
                                        .child(
                                            h_flex()
                                                .absolute()
                                                .right(zpx(action_right))
                                                .top(zpx((ROW_HEIGHT - 20.0) / 2.0))
                                                .h(zpx(20.0))
                                                .w(zpx(44.0))
                                                .items_center()
                                                .gap(zpx(4.0))
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
                                                    .on_click(move |_, _, cx| {
                                                        cx.stop_propagation();
                                                        pin_state.update(cx, |s, cx| {
                                                            s.toggle_terminal_pin(&pin_id, cx)
                                                        });
                                                    }),
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
                                                    .on_click(move |_, _, cx| {
                                                        cx.stop_propagation();
                                                        close_state.update(cx, |s, cx| {
                                                            s.remove_terminal(&close_id, cx)
                                                        });
                                                    }),
                                                ),
                                        )
                                        .context_menu(move |menu, _, cx| {
                                            session_menu(menu, &menu_state, &menu_id, pinned, cx)
                                        })
                                }),
                        )
                        .with_animation(
                            ("terminal-section", s.terminals_toggle),
                            Animation::new(std::time::Duration::from_millis(240)),
                            {
                                let opacity = terminal_opacity.clone();
                                let motion = s.terminals_motion;
                                let target = if s.terminals_collapsed { 0.0 } else { 1.0 };
                                let reduced = cx.reduce_motion();
                                let height = s
                                    .terminal_rows
                                    .iter()
                                    .filter(|row| !s.terminal_archived(&row.id))
                                    .count() as f32
                                    * (ROW_HEIGHT + 2.0);
                                move |list, _| {
                                    let fraction = if reduced {
                                        target
                                    } else {
                                        motion.map_or(target, |(start, from)| {
                                            let t = (start.elapsed().as_secs_f32() / 0.24).min(1.0);
                                            from + (target - from) * (1.0 - (1.0 - t).powi(4))
                                        })
                                    };
                                    opacity.set(fraction);
                                    list.h(zpx(height * fraction)).opacity(fraction)
                                }
                            },
                        ),
                )
                .when(
                    !archived.is_empty() || !archived_terminals.is_empty(),
                    |list| {
                        list.child(
                            h_flex()
                                .id("archived-toggle")
                                .h(zpx(ROW_HEIGHT))
                                .min_h(zpx(ROW_HEIGHT))
                                .flex_none()
                                .px_2()
                                .mt_1()
                                .items_center()
                                .rounded_md()
                                .cursor_pointer()
                                .text_size(zpx(TEXT_SM))
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
                                let group =
                                    SharedString::from(format!("archived-terminal-{}", id.0));
                                let actions = archived_actions(&id, &state);
                                h_flex()
                                    .id(group.clone())
                                    .group(group.clone())
                                    .relative()
                                    .items_center()
                                    .h(zpx(ROW_HEIGHT))
                                    .min_h(zpx(ROW_HEIGHT))
                                    .flex_none()
                                    .px_2()
                                    .gap_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|row| row.bg(rgba(0xffffff0a)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(fading_title(s.current_title(&id))),
                                    )
                                    .child(div().flex_none().w(zpx(44.0)))
                                    .child(
                                        actions
                                            .absolute()
                                            .right(zpx(8.0))
                                            .top(zpx((ROW_HEIGHT - 20.0) / 2.0))
                                            .opacity(0.0)
                                            .group_hover(group, |a| a.opacity(1.0)),
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
                        .min_h(zpx(12.0))
                        .w_full()
                        .window_control_area(WindowControlArea::Drag)
                        .on_mouse_down(MouseButton::Left, |_, window, _| {
                            window.start_window_move();
                        }),
                ),
        )
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
                        .child(Icon::new(IconName::Settings).small())
                        .child("Settings"),
                ),
        )
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
        let menu = root.read_with(cx, |root, _| root.0.clone());
        cx.update(|window, cx| menu.read(cx).focus_handle(cx).focus(window, cx));
        cx.simulate_keystrokes("down down down enter");
        cx.run_until_parked();
        assert_eq!(&*clicked.borrow(), &["Pin", "Mark as unread"]);
    }
}
