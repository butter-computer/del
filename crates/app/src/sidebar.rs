//! Space switching and actions above grouped sessions, with keep-awake,
//! archive, memory and settings buttons in the footer.

use crate::animated_icons::Glyph;
use crate::appearance::{rgb, rgba};
use crate::micro_motion::MotionIconExt as _;
use crate::sessions::Row;
use crate::state::{AppState, Panel, Selection};
use crate::zoom::px as zpx;
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::assets::IconName as ActionIcon;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::{ManagedTooltipExt as _, Tooltip};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_WIDTH: f32 = 264.0;
const ROW_HEIGHT: f32 = 30.0;
/// Toolbar icon buttons, flush with the session rows.
const ICON_BUTTON: f32 = 28.0;
const FOOTER_HEIGHT: f32 = crate::icons::TOOL_TAB_HEIGHT;
const FOOTER_BACKGROUND: u8 = 0x0e;
const FOOTER_HOVER: u8 = 0x1a;
const FOOTER_PRESSED: u8 = 0x24;
/// Hover actions on a session row: archive and pin.
const ROW_ACTIONS_WIDTH: f32 = crate::icons::BUTTON_SIZE * 2.0 + 4.0;

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
        let ids: Vec<_> = state.space_rows().into_iter().map(|row| row.id).collect();
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

/// A 28 px borderless icon button for the sidebar toolbar.
fn icon_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .ghost()
        .tooltip_placement(Placement::Bottom)
        .size(zpx(ICON_BUTTON))
        .p_0()
        .rounded(zpx(6.))
        .text_color(rgba(MUTED))
}

fn footer_tint(alpha: u8) -> Rgba {
    let tint = if crate::appearance::light() {
        0x00000000
    } else {
        0xffffff00
    };
    gpui_kit::rgba(tint | u32::from(alpha))
}

fn footer_row() -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .gap(zpx(6.))
        .pr(zpx(6.))
        .py(zpx((crate::icons::TOOL_ROW_HEIGHT - FOOTER_HEIGHT) / 2.))
        .flex_none()
}

fn footer_button(id: &'static str, cx: &App) -> Button {
    let background = footer_tint(FOOTER_BACKGROUND);
    Button::new(id)
        .tooltip_placement(Placement::Top)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(background.into())
                .hover(footer_tint(FOOTER_HOVER).into())
                .active(footer_tint(FOOTER_PRESSED).into())
                .shadow(false),
        )
        // One translucent fill per state; no extra border tint or base layer.
        // Retain the component's keyboard focus ring.
        .border_0()
        .w_0()
        .min_w_0()
        .flex_1()
        .h(zpx(FOOTER_HEIGHT))
        .p_0()
        .text_color(rgba(MUTED))
        .rounded(zpx(9.))
}

fn footer_trigger_style() -> StyleRefinement {
    StyleRefinement::default().w_0().min_w_0().flex_1()
}

/// Short relative age: now, 4m, 2h, 5d.
fn relative_time(updated_secs: u64) -> Option<String> {
    if updated_secs == 0 {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let age = now.saturating_sub(updated_secs);
    Some(match age {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", age / 60),
        3_600..86_400 => format!("{}h", age / 3_600),
        _ => format!("{}d", age / 86_400),
    })
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
pub(crate) fn coffee_icon(active: bool, animate: bool, cx: &App) -> AnyElement {
    // Off inherits the toolbar foreground like its neighbours; only on is gold.
    let color = rgb(0xf5a524);
    let cup = crate::icons::path("icons/coffee-cup.svg");
    div()
        .relative()
        .size(crate::icons::size())
        .child(if active { cup.text_color(color) } else { cup })
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
}

fn archive_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .text()
        .size(crate::icons::button_size())
        .p_0()
        .tooltip("Archive")
        .accessibility_label("Archive")
        .text_color(rgba(MUTED))
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
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    restore_state.update(cx, |s, cx| s.restore(restore_id.clone(), cx));
                })
                .motion_child(true, move |feedback| feedback.glyph(Glyph::Restore)),
        )
        .child(
            Button::new(SharedString::from(format!("delete-{}", id.0)))
                .ghost()
                .size(crate::icons::button_size())
                .p_0()
                .tooltip("Permanently delete")
                .accessibility_label("Permanently delete")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    delete_state.update(cx, |s, cx| s.permanently_delete(delete_id.clone(), cx));
                })
                .motion_child(true, move |feedback| {
                    feedback.glyph(Glyph::Trash).text_color(rgb(0xff646b))
                }),
        )
}

fn session_menu_item(
    label: &'static str,
    icon: Icon,
    motion: &crate::hover_motion::HoverMotion,
) -> impl IntoElement {
    let destructive = matches!(label, "Permanently delete" | "Delete Space");
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

/// Working spinner, waiting ring, failed mark or unread tick.
/// Read, inactive sessions have no status marker.
fn status_glyph(row: &Row, working: bool, unread: bool) -> Option<AnyElement> {
    let size = zpx(14.);
    let glyph = |path: &'static str| crate::icons::path(path).with_size(size);
    let (label, element) = if working {
        (
            "Working",
            crate::icons::working_spinner()
                .color(rgba(MUTED).into())
                .into_any_element(),
        )
    } else {
        match row.status {
            del_agent::SessionStatus::WaitingApproval => (
                "Waiting for you",
                glyph("icons/waiting-ring.svg")
                    .text_color(crate::appearance::ActionTone::Warning.foreground())
                    .into_any_element(),
            ),
            del_agent::SessionStatus::Error => (
                "Failed",
                glyph("icons/error.svg")
                    .text_color(rgb(0xff646b))
                    .into_any_element(),
            ),
            _ if unread => (
                "Unread",
                glyph("icons/circle-check-filled.svg")
                    .text_color(rgb(0xf5f5f5))
                    .into_any_element(),
            ),
            _ => return None,
        }
    };
    Some(
        div()
            .w(size)
            .h(crate::icons::button_size())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .id("status-glyph")
            .aria_label(label)
            .child(element)
            .into_any_element(),
    )
}

/// Read-only session details: use the hovered session's directory, never the
/// selected chat's branch. Git inspection is cached by AppState off-thread.
fn session_hover_card(row: &Row, state: &AppState, cx: &App) -> AnyElement {
    let terminal = state.terminal_rows.iter().find(|r| r.id == row.id);
    let provider = terminal.map(|r| r.provider).unwrap_or(Some(row.provider));
    let root = state.project_root(&row.cwd);
    let info = state.project_info.get(&row.cwd);
    let detail = |icon: &'static str, label: &'static str, value: String| {
        h_flex()
            .gap_2()
            .items_center()
            .child(crate::icons::path(icon).text_color(cx.theme().muted_foreground))
            .child(
                div()
                    .id(label)
                    .aria_label(format!("{label}: {value}"))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(value),
            )
    };
    let provider_label = provider
        .map(|p| crate::providers::display_name(p).to_string())
        .unwrap_or_else(|| "Terminal".into());
    let model = row.model.as_deref().unwrap_or("Automatic");
    v_flex()
        .id("session-hover-card")
        .debug_selector(|| "session-hover-card".into())
        .w(zpx(360.))
        .max_w_full()
        .gap_2()
        .text_size(zpx(TEXT_MD))
        .text_color(cx.theme().popover_foreground)
        .child(
            h_flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .font_weight(FontWeight::MEDIUM)
                        .child(row.title.clone()),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .text_size(zpx(TEXT_SM))
                        .text_color(cx.theme().muted_foreground)
                        .when_some(relative_time(row.updated_secs), |d, time| d.child(time))
                        .when_some(
                            status_glyph(
                                row,
                                state.session_working(&row.id),
                                row.unread || state.terminal_unread(&row.id),
                            ),
                            |d, glyph| d.child(glyph),
                        ),
                ),
        )
        .child(detail(
            "icons/folder.svg",
            "Project",
            state.project_name(&root),
        ))
        .child(detail(
            if info.is_some_and(|info| !info.current.is_empty()) {
                "icons/folder-git.svg"
            } else {
                "icons/folder.svg"
            },
            "Directory",
            row.cwd.to_string_lossy().into_owned(),
        ))
        .when_some(
            info.filter(|info| !info.current.is_empty()),
            |card, info| {
                card.child(detail(
                    "icons/git-branch.svg",
                    "Branch",
                    info.current.clone(),
                ))
                .when_some(info.worktree.as_ref(), |card, name| {
                    card.child(detail("icons/git-fork.svg", "Worktree", name.clone()))
                })
            },
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(if terminal.is_some() {
                    crate::mode_ui::terminal_mark(provider)
                } else {
                    crate::mode_ui::tab_mark(provider)
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(if terminal.is_some() {
                            if provider.is_some() {
                                format!("{provider_label} · Terminal")
                            } else {
                                "Terminal".into()
                            }
                        } else {
                            format!("{provider_label} · {model}")
                        }),
                ),
        )
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
    let unread = row.unread || state.read(cx).terminal_unread(&row.id);
    let id = row.id.clone();
    let key = SharedString::from(id.0.clone());
    let group = SharedString::from(format!("row-{}", id.0));

    let action_right = 8.0;
    let motion = state.read(cx).sidebar_hover.clone();
    let hover = motion.clone();
    let hover_key = id.0.clone();
    let hover_state = state.downgrade();
    let hover_cwd = row.cwd.clone();
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
        .on_hover(move |entered, window, cx| {
            hover.hover(&hover_key, *entered, window, cx);
            if *entered && !cx.has_active_drag() {
                let _ = hover_state.update(cx, |state, _| state.refresh_git(hover_cwd.clone()));
            }
        })
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
        ));

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
    item = item.child(if is_terminal {
        crate::mode_ui::terminal_mark(logo)
    } else {
        crate::mode_ui::tab_mark(logo)
    });
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
                .text_color(if selected || unread {
                    rgb(0xf5f5f5)
                } else {
                    rgba(0xffffffcc)
                })
                .font_weight(if unread {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
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
        let unread_tick = unread
            && !working
            && !matches!(
                row.status,
                del_agent::SessionStatus::WaitingApproval | del_agent::SessionStatus::Error
            );
        let time = relative_time(row.updated_secs).filter(|_| !unread_tick);
        item = item.child(if motion.is_hovered(&row.id.0) {
            // The hover actions take the meta slot; reserve their width so the
            // title never runs underneath them.
            div()
                .flex_none()
                .w(zpx(ROW_ACTIONS_WIDTH))
                .into_any_element()
        } else {
            h_flex()
                .flex_none()
                .gap_1()
                .items_center()
                .text_size(zpx(TEXT_SM))
                .text_color(rgba(FAINT))
                .when_some(time, |meta, time| meta.child(time))
                .when_some(status_glyph(row, working, unread), |meta, glyph| {
                    meta.child(glyph)
                })
                .into_any_element()
        });
        item = item.child(
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
                .w(zpx(ROW_ACTIONS_WIDTH))
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
                            })
                            .motion_child(true, move |feedback| feedback.glyph(Glyph::Archive)),
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
                        })
                        .motion_child(true, move |feedback| feedback.glyph(Glyph::Pin)),
                    )
                })
                .when(archived, |a| a.child(archived_actions(&id, state))),
        );
    }
    let content = if !archived && !renaming {
        let menu_state = state.clone();
        let menu_id = id.clone();
        let pinned = row.pinned;
        item.context_menu(move |menu, _, cx| session_menu(menu, &menu_state, &menu_id, pinned, cx))
            .into_any_element()
    } else {
        item.into_any_element()
    };
    // GPUI permits one hover listener per element. Keep the shared row motion
    // on the child and the managed tooltip on this identically sized wrapper.
    let mut item = div()
        .id(SharedString::from(format!("session-preview-{}", id.0)))
        .w_full()
        .child(content);
    if !renaming && !cx.has_active_drag() {
        let tooltip_state = state.downgrade();
        let tooltip_row = row.clone();
        item = item.managed_tooltip_at(Placement::Right, move |window, cx| {
            let state = tooltip_state.clone();
            let fallback = tooltip_row.clone();
            Tooltip::element(move |_, cx| {
                let Some(state) = state.upgrade() else {
                    return div().into_any_element();
                };
                let state = state.read(cx);
                let rows = state.unified_rows();
                let row = rows
                    .iter()
                    .find(|r| r.id == fallback.id)
                    .unwrap_or(&fallback);
                session_hover_card(row, state, cx)
            })
            .p_3()
            .rounded(zpx(12.))
            .build(window, cx)
        });
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
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let destinations: Vec<_> = state
        .read(cx)
        .spaces
        .items
        .iter()
        .filter(|space| space.id != state.read(cx).spaces.active)
        .map(|space| (space.id.clone(), space.name.clone()))
        .collect();
    let move_state = state.clone();
    let move_path = path.to_path_buf();
    let menu = menu.when(!destinations.is_empty(), |menu| {
        menu.submenu("Move to Space", window, cx, move |mut menu, _, _| {
            for (id, name) in &destinations {
                let state = move_state.clone();
                let path = move_path.clone();
                let id = id.clone();
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.move_project_to_space(&path, &id, cx));
                }));
            }
            menu
        })
        .submenu_content(|_, cx| {
            h_flex()
                .flex_1()
                .h(zpx(28.))
                .items_center()
                .gap(zpx(10.))
                .text_size(zpx(TEXT_MD))
                .child(crate::icons::icon(IconName::Copy))
                .child(div().flex_1().child("Move to Space"))
                .child(
                    crate::icons::icon(IconName::ChevronRight)
                        .text_color(cx.theme().muted_foreground),
                )
        })
    });
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

// Navigation uses the same rows and grouping as the rendered sidebar.
fn session_rows(s: &AppState) -> Vec<Row> {
    if !s.show_archived {
        return s.space_rows();
    }
    let mut rows = s.archived_rows();
    rows.extend(
        s.terminal_rows
            .iter()
            .filter(|r| s.terminal_archived(&r.id))
            .map(|r| Row {
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
            }),
    );
    rows.retain(|r| s.space_has_project(&r.cwd));
    rows
}

const PROJECT_SESSION_LIMIT: usize = 5;

fn visible_project_rows<'a>(
    s: &AppState,
    rows: &[&'a Row],
    path: &std::path::Path,
) -> Vec<&'a Row> {
    let expanded = s
        .expanded_sidebar_projects
        .contains(&(s.spaces.active.clone(), path.to_path_buf()));
    rows.iter().copied().enumerate().filter_map(|(index, row)| {
        let selected = matches!(&s.selection, Selection::Session(id) | Selection::Terminal(id) if *id == row.id);
        (expanded || index < PROJECT_SESSION_LIMIT || selected).then_some(row)
    }).collect()
}

pub(crate) fn navigation_tabs(s: &AppState) -> Vec<crate::state::Tab> {
    use crate::state::Tab;
    let rows = session_rows(s);
    let tab = |row: &Row| {
        if s.terminal_rows.iter().any(|terminal| terminal.id == row.id) {
            Tab::Terminal(row.id.clone())
        } else {
            Tab::Session(row.id.clone())
        }
    };
    let mut tabs = Vec::new();
    if !s.project_preferences.collapsed_sections.contains("Pinned") {
        tabs.extend(rows.iter().filter(|r| r.pinned && !r.archived).map(tab));
    }
    // The Space's projects have no section heading to collapse them.
    {
        for path in s.project_paths_for(rows.iter().map(|r| r.cwd.as_path())) {
            if s.project_preferences.collapsed.contains(&path) {
                continue;
            }
            if !s.show_archived {
                tabs.extend(s.project_drafts(&path).into_iter().map(Tab::Draft));
            }
            let project_rows = rows
                .iter()
                .filter(|r| (!r.pinned || r.archived) && s.project_root(&r.cwd) == path)
                .collect::<Vec<_>>();
            tabs.extend(
                visible_project_rows(s, &project_rows, &path)
                    .into_iter()
                    .map(tab),
            );
        }
    }
    tabs
}

fn empty_space(archived: bool) -> impl IntoElement {
    v_flex()
        .id("empty-space")
        .w_full()
        .min_w_0()
        .flex_none()
        .px(zpx(16.))
        .pt(zpx(48.))
        .pb(zpx(24.))
        .gap(zpx(12.))
        .items_center()
        .text_center()
        .whitespace_normal()
        .child(
            crate::icons::path("icons/empty-space.svg")
                .with_size(zpx(64.))
                .text_color(rgba(MUTED)),
        )
        .child(
            div()
                .w_full()
                .text_size(zpx(TEXT_MD))
                .text_color(rgba(MUTED))
                .child(if archived {
                    "Nothing in this orbit yet"
                } else {
                    "A little space for your next big idea"
                }),
        )
        .child(
            div()
                .w_full()
                .text_size(zpx(TEXT_SM))
                .text_color(rgba(FAINT))
                .child(if archived {
                    "Archived sessions will land here."
                } else {
                    "Use + to create a project and get started."
                }),
        )
}

fn space_contents(
    state: &Entity<AppState>,
    rename: &Entity<InputState>,
    space: &crate::state::spaces::Space,
    cx: &App,
) -> AnyElement {
    let s = state.read(cx);
    let current = space.id == s.spaces.active;
    let rows = if current {
        session_rows(s)
    } else {
        s.space_rows_for(space)
    };
    let active = match &s.selection {
        Selection::Session(id) | Selection::Terminal(id) => Some(id.clone()),
        Selection::Draft(_) => None,
    };
    let providers_open = s.panel == Panel::Providers;
    let show_archived = current && s.show_archived;
    let pinned_state = state.clone();
    let pinned_open = !s.project_preferences.collapsed_sections.contains("Pinned");
    let tabs = if current || s.spaces.global_tabs {
        &s.tabs
    } else {
        &space.deck.tabs
    };
    let drafts: Vec<_> = tabs
        .iter()
        .filter_map(|t| {
            if let crate::state::Tab::Draft(k) = t {
                Some(*k)
            } else {
                None
            }
        })
        .collect();
    let mut paths = if current {
        s.project_paths_for(rows.iter().map(|r| r.cwd.as_path()))
    } else {
        rows.iter()
            .map(|r| s.project_root(&r.cwd))
            .chain(space.projects.iter().cloned())
            .chain(drafts.iter().filter_map(|k| s.draft_project(*k)))
            .filter(|p| {
                s.spaces.includes_in(space, p)
                    && !s.project_preferences.archived.contains(p)
                    && !s.project_preferences.deleted.contains(p)
            })
            .collect::<Vec<_>>()
    };
    paths.sort();
    paths.dedup();
    let empty = rows.is_empty() && paths.is_empty();
    let scroll = if current {
        s.sidebar_scroll.clone()
    } else {
        gpui::ScrollHandle::new()
    };
    div().relative().size_full().min_h_0().overflow_hidden()
            .child(v_flex()
                .id("session-list")
                .relative()
                .child(s.sidebar_hover.layer(None, rgba(0xffffff00)))
                .h_full()
                .track_scroll(&scroll)
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
                // The Space switcher in the header names the Space, so projects follow
                // directly; the archive view keeps a plain label.
                .when(show_archived, |list| list.child(div().px_2().pt_2().pb_1().text_size(zpx(TEXT_SM)).text_color(rgba(FAINT)).child("Archived")))
                .when(empty, |list| list.child(empty_space(show_archived)))
                .child(v_flex().debug_selector(|| "projects-list".into()).w_full().flex_none().gap_0p5()
                .children(paths.into_iter().map(|path| {
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
                    let project_rows: Vec<&Row> = rows.iter().filter(|r| (!r.pinned || r.archived) && s.project_root(&r.cwd) == path).collect();
                    let needs = project_rows.iter().filter(|r| r.status == del_agent::SessionStatus::WaitingApproval).count();
                    let visible_rows = visible_project_rows(s, &project_rows, &path);
                    let remaining = project_rows.len() - visible_rows.len();
                    let expanded = s.expanded_sidebar_projects.contains(&(s.spaces.active.clone(), path.clone()));
                    let expand_state = state.clone();
                    let expand_key = (s.spaces.active.clone(), path.clone());
                    // Projects open and close instantly; no reveal animation.
                    gpui_kit::component::collapsible::Collapsible::new()
                        .open(!collapsed).flex_none().gap_0p5()
                        .child(h_flex().id(group.clone()).group(group.clone()).relative().h(zpx(ROW_HEIGHT)).mt(zpx(6.)).px_2().gap_1p5().items_center().rounded_md().cursor_pointer()
                            .text_color(rgba(MUTED)).text_size(zpx(TEXT_MD)).font_weight(FontWeight::SEMIBOLD)
                            .hover(|row| row.bg(rgba(0xffffff0a)))
                            .aria_label(format!("{} {}", if collapsed { "Expand" } else { "Collapse" }, path.display()))
                            .on_click(move |_, _, cx| { if !project_renaming { toggle_state.update(cx, |s, cx| s.toggle_project(toggle_path.clone(), cx)); } })
                            .context_menu(move |menu, _, _| {
                                let state = cleanup_state.clone(); let path = cleanup_path.clone();
                                menu.item(PopupMenuItem::new("Remove unused clean worktrees").on_click(move |_, _, cx| state.update(cx, |s, cx| s.cleanup_project_worktrees(path.clone(), cx))))
                            })
                            .child(if project_renaming {
                                div().flex_1().min_w_0().child(Input::new(rename).appearance(false).bordered(false).text_size(zpx(TEXT_MD))).into_any_element()
                            } else { div().flex_initial().min_w_0().truncate().child(s.project_name(&path)).into_any_element() })
                            .child(div().flex_1())
                            .when(needs > 0, |row| row.child(div().flex_none().font_weight(FontWeight::NORMAL).text_size(zpx(TEXT_SM))
                                .text_color(crate::appearance::ActionTone::Warning.foreground())
                                .group_hover(group.clone(), |badge| badge.opacity(0.))
                                .child(format!("{needs} needs you"))))
                            .child(h_flex().absolute().right(zpx(4.)).top_0().h_full().items_center().gap_0p5()
                                .child(Button::new(SharedString::from(format!("project-menu-{}", path.display()))).ghost().compact().icon(crate::icons::path("icons/more-horizontal.svg"))
                                    .accessibility_label("Project actions").opacity(0.).group_hover(group.clone(), |button| button.opacity(1.))
                                    .dropdown_menu(move |menu, window, cx| project_menu(menu, &menu_state, &menu_path, window, cx)))
                                .when(!show_archived, |row| row.child(Button::new(SharedString::from(format!("project-new-{}", path.display()))).ghost().compact()
                                    .opacity(0.).group_hover(group.clone(), |button| button.opacity(1.))
                                    .tooltip(format!("New session in {}", path.display())).accessibility_label("New session in project")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(move |_, _, cx| { cx.stop_propagation(); new_state.update(cx, |s, cx| s.new_project_session(new_path.clone(), cx)); }).motion_child(true, move |feedback| feedback.glyph(Glyph::Add))))))
                        .content(v_flex().w_full().min_w_0().pl(zpx(12.)).debug_selector(|| "project-session-content".into()).gap_0p5()
                            .children(drafts.iter().copied().filter(|key| s.draft_project(*key).is_some_and(|p| p == path)).collect::<Vec<_>>().into_iter().filter(|_| !show_archived).map(|key| {
                                let state = state.clone();
                                let close_state = state.clone();
                                let group = SharedString::from(format!("project-draft-{key}"));
                                let working = s.tab_working(&crate::state::Tab::Draft(key));
                                div().group(group.clone()).id(SharedString::from(format!("project-draft-{key}"))).px_2().h(zpx(ROW_HEIGHT)).flex().items_center().rounded_md()
                                    .text_size(zpx(TEXT_MD)).text_color(rgba(MUTED)).cursor_pointer()
                                    .when(s.selection == Selection::Draft(key), |row| row.bg(rgba(0xffffff14)))
                                    .hover(|row| row.bg(rgba(0xffffff0a))).gap_1p5()
                                    .child(crate::mode_ui::tab_mark(s.tab_info(&crate::state::Tab::Draft(key)).1))
                                    .child(div().flex_1().min_w_0().truncate().child("New session"))
                                    .when(working, |row| row.child(crate::icons::working_spinner().color(rgba(MUTED).into())))
                                    .when(!working && s.terminal_unread(&crate::state::AppState::tab_unread_id(&crate::state::Tab::Draft(key))), |row| row.child(crate::icons::unread_mark()))
                                    .child(div().id(SharedString::from(format!("close-project-draft-{key}")))
                                        .role(Role::Button).aria_label("Close new session")
                                        .size(zpx(crate::icons::TAB_CLOSE_SIZE)).flex_none().flex().items_center().justify_center()
                                        .opacity(0.).group_hover(group, |button| button.opacity(1.))
                                        .rounded_sm().hover(|button| button.bg(rgba(0xffffff14)))

                                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            close_state.update(cx, |s, cx| s.close_tab(&crate::state::Tab::Draft(key), cx));
                                        }).motion_child(true, |feedback| feedback.glyph(Glyph::Close).with_size(zpx(crate::icons::TAB_CLOSE_GLYPH))))
                                    .on_click(move |_, _, cx| state.update(cx, |s, cx| s.select_tab(&crate::state::Tab::Draft(key), cx)))
                            }))
                            .children(visible_rows.iter().map(|row|
                                div().opacity(if row.archived { 0.6 } else { 1. }).child(row_item(row, active.as_ref() == Some(&row.id) && !providers_open, s.renaming.as_ref() == Some(&row.id), rename, state, row.archived, cx))))
                            .when(remaining > 0 || (expanded && project_rows.len() > PROJECT_SESSION_LIMIT), |list| list.child(
                                Button::new(SharedString::from(format!("project-show-more-{}", path.display())))
                                    .ghost().w_full().h(zpx(ROW_HEIGHT)).justify_start().px_2()
                                    .text_color(rgba(MUTED)).text_size(zpx(TEXT_SM))
                                    .label(if expanded { "Show less".into() } else { format!("Show more ({remaining})") })
                                    .on_click(move |_, _, cx| expand_state.update(cx, |s, cx| {
                                        if !s.expanded_sidebar_projects.remove(&expand_key) {
                                            s.expanded_sidebar_projects.insert(expand_key.clone());
                                        }
                                        cx.notify();
                                    })))))
                })))
                .child(
                    div()
                        .id("sidebar-empty-space")
                        .flex_1()
                        .min_h(zpx(12.0))
                        .w_full(),
                ))
            .children(crate::tab_transfer::vertical_edge_fades(&scroll, crate::appearance::sidebar_surface())).into_any_element()
}

pub fn render(
    state: &Entity<AppState>,
    rename: &Entity<InputState>,
    usage: &Entity<crate::usage::Usage>,
    updater: &Entity<crate::app_updates::AppUpdater>,
    cx: &App,
) -> AnyElement {
    let s = state.read(cx);
    let providers_open = s.panel == Panel::Providers;
    let show_archived = s.show_archived;
    let archive_state = state.clone();
    let prov_state = state.clone();
    let caffeine_state = state.clone();
    let switcher_state = state.clone();
    let switcher_open_state = state.clone();
    let space_actions_state = state.clone();
    let space_name = s
        .spaces
        .items
        .iter()
        .find(|space| space.id == s.spaces.active)
        .map(|space| space.name.clone())
        .unwrap_or_default();
    v_flex()
        .id("spaces-sidebar")
        .relative()
        .child(space_swipe_surface(state))
        .w_full()
        .min_w_0()
        .whitespace_nowrap()
        .h_full()
        .flex_none()
        .gap_0p5()
        .pl_2()
        .pb_2()
        .child(
            // Space switcher on the left; Space actions on the right.
            h_flex()
                .w_full()
                .min_w_0()
                .flex_none()
                .h(zpx(crate::icons::TOOL_ROW_HEIGHT))
                .pr(zpx(6.))
                .gap_0p5()
                .items_center()
                .child(
                    Button::new("space-switcher")
                        .ghost()
                        .h(zpx(ICON_BUTTON))
                        .px_2()
                        .min_w_0()
                        .rounded(zpx(6.))
                        .tooltip("Switch Space")
                        .accessibility_label(format!("Space: {space_name}"))
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_1()
                                .items_center()
                                .text_size(zpx(TEXT_MD))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xf5f5f5))
                                .child(div().min_w_0().truncate().child(space_name))
                                .child(
                                    crate::micro_motion::MorphIcon::new(
                                        "space-switcher-arrow",
                                        crate::animated_icons::Morph::Disclosure,
                                        s.space_switcher_open,
                                    )
                                    .with_size(zpx(12.))
                                    .text_color(rgba(MUTED)),
                                ),
                        )
                        .dropdown_menu(move |menu, _, cx| {
                            space_switch_menu(menu, &switcher_state, cx)
                        })
                        .on_open_change(move |open, _, cx| {
                            switcher_open_state.update(cx, |s, cx| {
                                s.space_switcher_open = *open;
                                cx.notify();
                            })
                        }),
                )
                .child(div().flex_1())
                .child(crate::micro_motion::control(
                    "space-actions-motion",
                    true,
                    move |feedback| {
                        div().child(
                            icon_button("space-actions")
                                .tooltip("Space actions")
                                .accessibility_label("Space actions")
                                .child(feedback.glyph(Glyph::Add))
                                .dropdown_menu(move |menu, _, cx| {
                                    space_menu(menu, &space_actions_state, true, cx)
                                }),
                        )
                    },
                )),
        )
        .child(space_pages(state, rename, cx))
        .when(s.spaces.items.len() > 1, |sidebar| {
            sidebar.child(
                h_flex()
                    .id("space-indicators")
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .overflow_x_scroll()
                    .justify_center()
                    .items_center()
                    .gap_0()
                    .py_1()
                    .children(s.spaces.items.iter().map(|space| {
                        let active = space.id == s.spaces.active;
                        let id = space.id.clone();
                        let state = state.clone();
                        Button::new(SharedString::from(format!("space-dot-{id}")))
                            .ghost()
                            .w(zpx(18.))
                            .h(zpx(24.))
                            .p_0()
                            .tooltip(space.name.clone())
                            .accessibility_label(format!("Switch to {}", space.name))
                            .child(
                                div()
                                    .size(zpx(16.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(active, |dot| dot.bg(rgba(0xffffff18)))
                                    .child(
                                        div().size(zpx(6.)).rounded_full().bg(rgba(if active {
                                            0xfffffff0
                                        } else {
                                            0xffffff50
                                        })),
                                    ),
                            )
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.switch_space(&id, cx))
                            })
                    })),
            )
        })
        .child(crate::app_updates::render(updater, cx))
        .child(
            footer_row()
                .child(
                    footer_button("caffeine-toggle", cx)
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
                        .on_click(move |_, _, cx| {
                            caffeine_state.update(cx, |s, cx| s.toggle_caffeine(cx))
                        }),
                )
                .child(
                    footer_button("show-archived", cx)
                        .selected(show_archived)
                        .when(show_archived, |button| {
                            button.bg(rgba(0xffffff14)).text_color(rgb(0xf5f5f5))
                        })
                        .tooltip(if show_archived {
                            "Show active sessions"
                        } else {
                            "Show archived sessions and projects"
                        })
                        .accessibility_label("Toggle archived sessions")
                        .on_click(move |_, _, cx| {
                            archive_state.update(cx, |s, cx| {
                                s.show_archived = !s.show_archived;
                                cx.notify();
                            })
                        })
                        .motion_child(true, |feedback| feedback.glyph(Glyph::Archive)),
                )
                .child(
                    gpui_kit::component::popover::Popover::new("session-usage")
                        .trigger_style(footer_trigger_style())
                        .open(usage.read(cx).open)
                        .arrow(true)
                        .anchor(Anchor::BottomLeft)
                        .on_open_change({
                            let usage = usage.clone();
                            move |open, _, cx| {
                                usage.update(cx, |usage, cx| usage.set_open(*open, cx))
                            }
                        })
                        .trigger(
                            footer_button("session-usage-trigger", cx)
                                .w_full()
                                .flex_none()
                                .tooltip("Memory usage")
                                .accessibility_label("Memory usage")
                                .child(crate::micro_motion::MorphIcon::new(
                                    "memory-waveform",
                                    crate::animated_icons::Morph::Activity,
                                    usage.read(cx).open,
                                )),
                        )
                        .content({
                            let usage = usage.clone();
                            move |_, _, _| usage.clone()
                        }),
                )
                .child(
                    footer_button("settings-entry", cx)
                        .selected(providers_open)
                        .when(providers_open, |button| {
                            button.bg(rgba(0xffffff14)).text_color(rgb(0xf5f5f5))
                        })
                        .tooltip("Settings (⌘,)")
                        .accessibility_label("Settings")
                        .on_click(move |_, _, cx| {
                            prov_state.update(cx, |s, cx| s.toggle_providers(cx))
                        })
                        .motion_child(true, |feedback| feedback.glyph(Glyph::Settings)),
                ),
        )
        .into_any_element()
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
    fn space_delete_divider_spans_checked_menu_width(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| s.create_space("Work".into(), cx));
        let (_, cx) = cx.add_window_view(move |window, cx| {
            window.set_rem_size(px(14.));
            let menu = PopupMenu::build(window, cx, move |menu, _, cx| {
                super::space_switch_menu(menu, &state, cx)
            });
            MenuFixture(menu)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let line = cx.debug_bounds("space-menu-divider").unwrap();
        let menu = cx.debug_bounds("menu-fixture").unwrap();
        assert_eq!(line.left(), menu.left());
        assert_eq!(line.right(), menu.right());
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

/// Spaces to switch to, followed by Space management actions.
pub(crate) fn space_switch_menu(
    mut menu: PopupMenu,
    state: &Entity<AppState>,
    cx: &mut App,
) -> PopupMenu {
    let spaces = state.read(cx).spaces.items.clone();
    let active = state.read(cx).spaces.active.clone();
    for space in spaces {
        let state = state.clone();
        let id = space.id.clone();
        menu = menu.item(
            PopupMenuItem::new(space.name)
                .checked(space.id == active)
                .on_click(move |_, _, cx| state.update(cx, |s, cx| s.switch_space(&id, cx))),
        );
    }
    space_menu(menu.separator(), state, false, cx)
}

fn space_menu(
    menu: PopupMenu,
    state: &Entity<AppState>,
    include_project: bool,
    cx: &mut App,
) -> PopupMenu {
    let deletable = state.read(cx).spaces.items.len() > 1;
    let motion = crate::hover_motion::HoverMotion::menu(cx);
    let clicks = motion.clone();
    let state = state.clone();
    menu.item(
        PopupMenuItem::element(move |_, cx| {
            let background: Rgba = cx.theme().popover.into();
            v_flex()
                .relative()
                .flex_1()
                .min_w(zpx(152.))
                .mx(gpui::px(-8.))
                .gap_0p5()
                .bg(background)
                .child(motion.layer(Some(background), cx.theme().border.into()))
                .child(session_menu_item(
                    "Create Space",
                    crate::icons::icon(IconName::Copy),
                    &motion,
                ))
                .when(include_project, |menu| {
                    menu.child(session_menu_item(
                        "Create Project",
                        crate::icons::icon(IconName::Folder),
                        &motion,
                    ))
                })
                .child(session_menu_item(
                    "Rename Space",
                    crate::icons::icon(ActionIcon::Pencil),
                    &motion,
                ))
                .when(deletable, |menu| {
                    menu.child(
                        motion
                            .divider()
                            .mx_neg_1()
                            .debug_selector(|| "space-menu-divider".into()),
                    )
                    .child(session_menu_item(
                        "Delete Space",
                        crate::icons::icon(ActionIcon::Trash),
                        &motion,
                    ))
                })
        })
        .custom_highlight()
        .on_click(move |event, window, cx| {
            let Some(action) = clicks.clicked(event) else {
                return;
            };
            match action.as_str() {
                "Create Space" => space_dialog(&state, false, window, cx),
                "Rename Space" => space_dialog(&state, true, window, cx),
                "Create Project" => state.update(cx, |s, cx| s.add_project(cx)),
                "Delete Space" => {
                    state.update(cx, |s, cx| s.delete_space(&s.spaces.active.clone(), cx))
                }
                _ => {}
            }
        }),
    )
}

fn space_dialog(state: &Entity<AppState>, rename: bool, window: &mut Window, cx: &mut App) {
    let id = state.read(cx).spaces.active.clone();
    let initial = if rename {
        state.read(cx).spaces.current().name.clone()
    } else {
        String::new()
    };
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder("Space name");
        input.set_value(initial, window, cx);
        input
    });
    let handle = window.window_handle();
    cx.subscribe(
        &input,
        move |_, event: &gpui_kit::component::input::InputEvent, cx| {
            if matches!(
                event,
                gpui_kit::component::input::InputEvent::PressEnter { .. }
            ) {
                let _ = handle.update(cx, |_, window, cx| {
                    window.dispatch_action(
                        Box::new(gpui_kit::component::dialog::Confirm { secondary: false }),
                        cx,
                    )
                });
            }
        },
    )
    .detach();
    let focus = input.clone();
    let state = state.clone();
    let submitted = std::rc::Rc::new(std::cell::Cell::new(false));
    window.open_dialog(cx, move |dialog, _, _| {
        let input = input.clone();
        let value = input.clone();
        let state = state.clone();
        let id = id.clone();
        let submitted = submitted.clone();
        dialog
            .title(if rename {
                "Rename Space"
            } else {
                "Create Space"
            })
            .w(zpx(320.))
            .close_button(false)
            .p_4()
            .gap_3()
            .border_0()
            .bg(crate::appearance::popup_surface())
            .entrance(gpui_kit::component::dialog::DialogEntrance::FadeSlide)
            .child(Input::new(&input).bordered(false).focus_bordered(false))
            .footer(
                gpui_kit::component::dialog::DialogFooter::new()
                    .pt_0()
                    .gap_2()
                    .child(
                        Button::new("space-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, w, cx| w.close_dialog(cx)),
                    )
                    .child(
                        Button::new("space-confirm")
                            .ghost()
                            .label(if rename { "Save" } else { "Create" })
                            .on_click(|_, w, cx| {
                                w.dispatch_action(
                                    Box::new(gpui_kit::component::dialog::Confirm {
                                        secondary: false,
                                    }),
                                    cx,
                                )
                            }),
                    ),
            )
            .on_ok(move |_, _, cx| {
                let name = value.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                if submitted.replace(true) {
                    return true;
                }
                state.update(cx, |s, cx| {
                    if rename {
                        s.rename_space(&id, name, cx)
                    } else {
                        s.create_space(name, cx)
                    }
                });
                true
            })
    });
    window.defer(cx, move |window, cx| {
        focus.update(cx, |input, cx| input.focus(window, cx))
    });
}

fn space_pages(state: &Entity<AppState>, rename: &Entity<InputState>, cx: &App) -> AnyElement {
    let s = state.read(cx);
    let offset = if cx.reduce_motion() {
        0.
    } else {
        s.space_swipe.offset
    };
    let incoming = s
        .space_swipe
        .target
        .as_ref()
        .and_then(|id| s.spaces.items.iter().find(|space| &space.id == id));
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id(SharedString::from(format!(
                    "space-page-{}",
                    s.spaces.active
                )))
                .debug_selector(|| "space-outgoing".into())
                .absolute()
                .top_0()
                .left(gpui::px(offset))
                .size_full()
                .child(space_contents(state, rename, s.spaces.current(), cx)),
        )
        .when(offset != 0., |pages| {
            pages.children(incoming.map(|space| {
                div()
                    .id(SharedString::from(format!("space-page-{}", space.id)))
                    .debug_selector(|| "space-incoming".into())
                    .absolute()
                    .top_0()
                    .left(gpui::px(offset - offset.signum() * s.space_swipe.width))
                    .size_full()
                    .child(space_contents(state, rename, space, cx))
            }))
        })
        .into_any_element()
}

fn settle_space_swipe(state: Entity<AppState>, window: &mut Window, _cx: &mut App) {
    window.on_next_frame(move |window, cx| {
        let reduced = cx.reduce_motion();
        let again = state.update(cx, |s, cx| {
            if let Some(target) = s.space_swipe.advance(std::time::Instant::now(), reduced) {
                s.switch_space(&target, cx);
            }
            cx.notify();
            s.space_swipe.settling()
        });
        if again {
            settle_space_swipe(state, window, cx);
        }
    });
    // on_next_frame schedules the frame itself; request_animation_frame requires a rendering view.
}

fn space_swipe_surface(state: &Entity<AppState>) -> impl IntoElement {
    let state = state.clone();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let state = state.clone();
            window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                if !phase.capture() || !hitbox.id.should_handle_scroll(window) {
                    return;
                }
                if !matches!(event.delta, gpui::ScrollDelta::Pixels(_)) {
                    return;
                }
                let delta = event.delta.pixel_delta(window.line_height());
                let reduced = cx.reduce_motion();
                let settle = state.update(cx, |s, cx| {
                    if s.spaces.items.len() < 2 {
                        return false;
                    }
                    let index = s
                        .spaces
                        .items
                        .iter()
                        .position(|v| v.id == s.spaces.active)
                        .unwrap_or(0);
                    let previous = index.checked_sub(1).map(|i| s.spaces.items[i].id.clone());
                    let next = s.spaces.items.get(index + 1).map(|v| v.id.clone());
                    let was_settling = s.space_swipe.settling();
                    let horizontal = s.space_swipe.update(
                        (delta.x.as_f32(), delta.y.as_f32()),
                        event.touch_phase,
                        std::time::Instant::now(),
                        bounds.size.width.as_f32(),
                        previous,
                        next,
                    );
                    if horizontal || delta.x.abs() > delta.y.abs() {
                        cx.stop_propagation();
                    }
                    if reduced
                        && let Some(target) = s.space_swipe.advance(std::time::Instant::now(), true)
                    {
                        s.switch_space(&target, cx);
                    }
                    if horizontal {
                        cx.notify();
                    }
                    !was_settling && s.space_swipe.settling()
                });
                if settle {
                    settle_space_swipe(state.clone(), window, cx);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod space_gesture_tests {
    use crate::state::AppState;
    use gpui::{
        AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
        ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled,
        TouchPhase, Window, div, point, px,
    };

    struct Fixture {
        state: Entity<AppState>,
        scroll: ScrollHandle,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .w(px(220.))
                .h(px(300.))
                .child(super::space_swipe_surface(&self.state))
                .child(
                    div()
                        .id("scrolling-projects")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(div().h(px(1000.)).w_full()),
                )
        }
    }
    #[gpui::test]
    fn horizontal_swipe_is_captured_before_vertical_list_scrolling(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.create_space("Work".into(), cx);
            s.switch_space("default", cx);
        });
        let scroll = ScrollHandle::new();
        let (_view, cx) = cx.add_window_view({
            let state = state.clone();
            let scroll = scroll.clone();
            move |_, _| Fixture { state, scroll }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(-90.), px(0.))),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        state.read_with(cx, |s, _| {
            assert_eq!(s.spaces.current().name, "Default");
            assert_eq!(s.space_swipe.offset, -90.);
        });
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(0.))),
            touch_phase: TouchPhase::Ended,
            ..Default::default()
        });
        state.read_with(cx, |s, _| assert_eq!(s.spaces.current().name, "Work"));
        assert_eq!(scroll.offset().y, px(0.));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-90.))),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        state.read_with(cx, |s, _| assert_eq!(s.spaces.current().name, "Work"));
        assert!(scroll.offset().y < px(0.));
    }
    struct PagesFixture {
        state: Entity<AppState>,
        input: Entity<gpui_kit::component::input::InputState>,
    }
    impl Render for PagesFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .flex()
                .flex_col()
                .w(px(220.))
                .h(px(300.))
                .child(super::space_swipe_surface(&self.state))
                .child(super::space_pages(&self.state, &self.input, cx))
        }
    }
    #[gpui::test]
    fn both_space_lists_move_with_finger_distance_before_selection_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        state.update(cx, |s, cx| {
            s.create_space("Work".into(), cx);
            s.switch_space("default", cx);
        });
        let (_view, cx) = cx.add_window_view({
            let state = state.clone();
            move |window, cx| {
                let input = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
                PagesFixture { state, input }
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let original = cx.debug_bounds("space-outgoing").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(-70.), px(0.))),
            touch_phase: TouchPhase::Started,
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let outgoing = cx.debug_bounds("space-outgoing").unwrap();
        let incoming = cx.debug_bounds("space-incoming").unwrap();
        assert_eq!(outgoing.origin.x, original.origin.x - px(70.));
        assert_eq!(incoming.origin.x, outgoing.origin.x + original.size.width);
        assert_eq!(outgoing.size, original.size);
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(0.))),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            cx.debug_bounds("space-outgoing").unwrap().origin.x,
            original.origin.x - px(110.)
        );
        state.read_with(cx, |s, _| assert_eq!(s.spaces.active, "default"));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(0.))),
            touch_phase: TouchPhase::Ended,
            ..Default::default()
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }

    #[gpui::test]
    fn enter_creates_exactly_one_space_and_rename_keeps_its_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let state = crate::state::tests::draft(cx);
        let (_view, cx) = cx.add_window_view({
            let state = state.clone();
            move |window, cx| {
                let view = cx.new(|_| Fixture {
                    state,
                    scroll: ScrollHandle::new(),
                });
                gpui_kit::component::Root::new(view, window, cx)
            }
        });
        cx.update(|window, cx| super::space_dialog(&state, false, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_input("Work");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let id = state.read_with(cx, |s, _| {
            assert_eq!(s.spaces.items.len(), 2);
            assert_eq!(s.spaces.current().name, "Work");
            s.spaces.active.clone()
        });
        cx.update(|window, cx| super::space_dialog(&state, true, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("Studio");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        state.read_with(cx, |s, _| {
            assert_eq!(s.spaces.items.len(), 2);
            assert_eq!(s.spaces.active, id);
            assert_eq!(s.spaces.current().name, "Studio");
        });
    }
}

#[cfg(test)]
mod session_hover_tests {
    use super::{AppState, InputState, Row};
    use gpui::{
        AppContext, Context, Entity, IntoElement, MouseButton, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use gpui_kit::component::Root;
    use std::time::Duration;

    struct Fixture {
        state: Entity<AppState>,
        row: Row,
        rename: Entity<InputState>,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().w(px(264.)).mt(px(100.)).child(super::row_item(
                    &self.row,
                    false,
                    false,
                    &self.rename,
                    &self.state,
                    false,
                    cx,
                )))
        }
    }

    fn settle(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn session_hover_is_read_only_and_dismisses_on_leave_and_press(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let state = crate::state::tests::draft(cx);
        let before = state.read_with(cx, |s, _| s.selection.clone());
        let row = Row {
            id: del_agent::SessionId::new("hover-test"),
            provider: del_agent::Provider::Codex,
            model: Some("review-model".into()),
            cwd: "/repo/review-worktree".into(),
            title: "A long session title that remains readable in the session preview".into(),
            status: del_agent::SessionStatus::Idle,
            updated_secs: 1,
            archived: false,
            pinned: false,
            unread: true,
        };
        state.update(cx, |s, cx| {
            s.mark_session_unread(&row.id, cx);
            s.project_info.insert(
                row.cwd.clone(),
                crate::projects::GitInfo {
                    root: "/repo".into(),
                    current: "review/independent".into(),
                    worktree: Some("review-worktree".into()),
                    ..Default::default()
                },
            );
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let fixture = cx.new(|cx| Fixture {
                    state: state.clone(),
                    row,
                    rename: cx.new(|cx| InputState::new(window, cx)),
                });
                cx.new(|cx| Root::new(fixture, window, cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        settle(&mut cx);
        assert!(cx.debug_bounds("session-hover-card").is_none());
        cx.simulate_mouse_move(point(px(80.), px(115.)), None, Default::default());
        settle(&mut cx);
        let card = cx
            .debug_bounds("session-hover-card")
            .expect("delayed hover card");
        assert!(card.origin.x >= px(264.), "card stays clear of the sidebar");
        state.read_with(&cx, |s, _| {
            assert_eq!(s.selection, before);
            assert!(s.terminal_unread(&del_agent::SessionId::new("hover-test")));
        });
        cx.simulate_mouse_move(point(px(700.), px(500.)), None, Default::default());
        settle(&mut cx);
        assert!(cx.debug_bounds("session-hover-card").is_none());
        cx.simulate_mouse_move(point(px(80.), px(115.)), None, Default::default());
        settle(&mut cx);
        assert!(cx.debug_bounds("session-hover-card").is_some());
        cx.simulate_mouse_down(
            point(px(80.), px(115.)),
            MouseButton::Left,
            Default::default(),
        );
        settle(&mut cx);
        assert!(cx.debug_bounds("session-hover-card").is_none());
    }
}
