//! The sessions sidebar: new-session button, session rows, archived list and
//! the providers entry. Borderless; rows are told apart by fill alone.

use crate::sessions::{self, Row};
use crate::state::{AppState, Panel, Selection};
use del_agent::{LoginState, SessionStatus};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 28.0;

const TEXT: u32 = 0xf0f0f0;
const MUTED: u32 = 0xffffff8c;
const FAINT: u32 = 0xffffff52;

pub fn status_color(status: SessionStatus) -> Rgba {
    rgb(match status {
        SessionStatus::Idle => 0x6e6e73,
        SessionStatus::Running => 0x4da3ff,
        SessionStatus::WaitingApproval => 0xf5a524,
        SessionStatus::Error => 0xf0524f,
    })
}

fn dot(color: Rgba) -> Div {
    div().size(px(6.0)).flex_none().rounded_full().bg(color)
}

/// A small borderless text button that must not start a window drag.
fn text_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_1()
        .rounded_sm()
        .text_size(px(TEXT_SM))
        .text_color(rgba(MUTED))
        .cursor_pointer()
        .hover(|b| b.bg(rgba(0xffffff14)).text_color(rgb(TEXT)))
        .child(label)
}

fn row_item(
    row: &Row,
    selected: bool,
    renaming: bool,
    rename: &Entity<InputState>,
    state: &Entity<AppState>,
    now: u64,
    archived: bool,
) -> impl IntoElement {
    let id = row.id.clone();
    let key = SharedString::from(id.0.clone());
    let group = SharedString::from(format!("row-{}", id.0));

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
        .when(selected, |r| r.bg(rgba(0xffffff14)))
        .when(!selected, |r| r.hover(|r| r.bg(rgba(0xffffff0a))))
        .child(dot(status_color(row.status)))
        .child(
            div()
                .w(px(22.0))
                .flex_none()
                .whitespace_nowrap()
                .text_size(px(TEXT_SM))
                .text_color(rgba(FAINT))
                .child(sessions::glyph(row.provider)),
        );

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
                .child(row.title.clone()),
        )
    };

    if !renaming {
        let (rename_state, archive_state) = (state.clone(), state.clone());
        let (rename_id, archive_id) = (id.clone(), id.clone());
        let hover_group = group.clone();
        let time = sessions::relative_time(now, row.updated_secs);
        item = item
            .child(
                // The time gives way to the actions while the row is hovered.
                div()
                    .flex_none()
                    .text_size(px(TEXT_SM))
                    .text_color(rgba(FAINT))
                    .group_hover(group.clone(), |t| t.opacity(0.0))
                    .child(time),
            )
            .child(
                // Always laid out and painted, only faded in on hover: toggling
                // `display` under the pointer made GPUI paint children it had
                // not prepainted, which aborted the app.
                h_flex()
                    .absolute()
                    .right_1()
                    .pl_2()
                    .gap_0p5()
                    .rounded_md()
                    .bg(rgb(0x2b2b2e))
                    .opacity(0.0)
                    .group_hover(hover_group, |a| a.opacity(1.0))
                    .when(!archived, |a| {
                        a.child(
                            text_button(SharedString::from(format!("rn-{}", id.0)), "Rename")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let id = rename_id.clone();
                                    rename_state.update(cx, |s, cx| s.start_rename(id, cx));
                                }),
                        )
                        .child(
                            text_button(SharedString::from(format!("ar-{}", id.0)), "Archive")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let id = archive_id.clone();
                                    archive_state.update(cx, |s, cx| s.archive(id, cx));
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
    }
    item
}

pub fn render(state: &Entity<AppState>, rename: &Entity<InputState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let now = sessions::now_secs();
    let rows = s.rows();
    let archived = s.archived_rows();
    let active = match &s.selection {
        Selection::Session(id) => Some(id.clone()),
        Selection::Draft(_) => None,
    };
    let draft_selected = matches!(s.selection, Selection::Draft(_)) && s.panel == Panel::Chat;
    let providers_open = s.panel == Panel::Providers;
    let show_archived = s.show_archived;

    let new_state = state.clone();
    let prov_state = state.clone();
    let arch_state = state.clone();

    v_flex()
        .w(px(SIDEBAR_WIDTH))
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
                    row_item(row, selected, renaming, rename, state, now, false)
                }))
                .when(!archived.is_empty(), |list| {
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
                            .child(format!("Archived ({})", archived.len())),
                    )
                    .when(show_archived, |list| {
                        list.children(
                            archived
                                .iter()
                                .map(|row| row_item(row, false, false, rename, state, now, true)),
                        )
                    })
                }),
        )
        .child(
            h_flex()
                .id("providers-entry")
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_1p5()
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
                .on_click(move |_, _, cx| prov_state.update(cx, |s, cx| s.toggle_providers(cx)))
                .child(div().flex_1().child("Providers"))
                .children(s.providers.iter().map(|p| {
                    dot(rgb(match (p.installed, p.login) {
                        (false, _) => 0x6e6e73,
                        (true, LoginState::LoggedIn) => 0x4cc38a,
                        (true, LoginState::LoggedOut) => 0xf5a524,
                        (true, LoginState::Unknown) => 0x6e6e73,
                    }))
                })),
        )
}
