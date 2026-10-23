//! Rendering of one transcript row per `Block`.

use super::model::{
    ApprovalBlock, ApprovalState, Block, DiffKind, FileChangeBlock, ToolCall, ToolState,
};
use super::view::{COLUMN_MAX_WIDTH, ChatView};
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use del_agent::{ApprovalKind, ApprovalResponse, FileChangeKind, TurnStatus};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use gpui_kit::base::Easing;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewMotion, TextViewStyle};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const TEXT: u32 = 0xe8e8e8;
const STRONG: u32 = 0xf5f5f5;
const MUTED: u32 = 0xffffff8c;
const FAINT: u32 = 0xffffff4d;
/// Fill of cards (tool calls, diffs, the user's message).
const CARD: u32 = 0xffffff0d;
const CARD_HOVER: u32 = 0xffffff14;
const GREEN: u32 = 0x5fc27eff;
const RED: u32 = 0xf0716bff;
const AMBER: u32 = 0xe8b04aff;
const MONO: &str = "Menlo";

impl ChatView {
    /// One list row: the block, centered in the transcript column.
    pub(super) fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(block) = self.model.blocks.get(ix).cloned() else {
            return div().into_any_element();
        };
        let content = match block {
            Block::User { text, steer } => user_message(text, steer).into_any_element(),
            Block::Assistant { text, turn_id, .. } => {
                let started = self
                    .first_chunks
                    .get(&turn_id)
                    .copied()
                    .filter(|start| !cx.reduce_motion() && start.elapsed() < STREAM_FADE);
                assistant_message(ix, text, started).into_any_element()
            }
            Block::Reasoning {
                text,
                open,
                expanded,
            } => self
                .reasoning(ix, text, open, expanded, cx)
                .into_any_element(),
            Block::Tool(tool) => self.tool_card(ix, tool, cx).into_any_element(),
            Block::FileChange(change) => self.file_card(ix, change, cx).into_any_element(),
            Block::Approval(approval) => self.approval_card(approval, cx).into_any_element(),
            Block::Error { .. } => div().h_0().into_any_element(),
            Block::TurnEnd { status } => turn_end(status).into_any_element(),
        };
        h_flex()
            .w_full()
            .justify_center()
            .px_3()
            .child(div().w_full().max_w(zpx(COLUMN_MAX_WIDTH)).child(content))
            .into_any_element()
    }

    fn reasoning(
        &self,
        ix: usize,
        text: String,
        open: bool,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id(("reasoning", ix))
                    .px_2()
                    .h(zpx(22.0))
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(zpx(TEXT_SM))
                    .text_color(rgba(MUTED))
                    .hover(|row| row.bg(rgba(CARD)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                    .when(open, |row| {
                        row.child(Spinner::new().xsmall().color(rgba(MUTED).into()))
                    })
                    .child(if open { "Thinking" } else { "Thought" })
                    .child(div().text_color(rgba(FAINT)).child(if expanded {
                        "hide"
                    } else {
                        "show"
                    })),
            )
            .when(expanded, |col| {
                col.child(
                    div()
                        .px_2()
                        .text_size(zpx(TEXT_SM))
                        .line_height(relative(1.5))
                        .text_color(rgba(MUTED))
                        .child(text),
                )
            })
    }

    fn tool_card(&self, ix: usize, tool: ToolCall, cx: &mut Context<Self>) -> impl IntoElement {
        let (state_label, state_color) = match tool.state {
            ToolState::Running => ("running", AMBER),
            ToolState::Ok => ("ok", GREEN),
            ToolState::Error => ("error", RED),
            ToolState::Declined => ("declined", MUTED),
        };
        let has_output = !tool.output.is_empty();
        v_flex()
            .rounded_lg()
            .bg(rgba(CARD))
            .child(
                h_flex()
                    .id(("tool", ix))
                    .h(zpx(30.0))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .when(has_output, |row| row.cursor_pointer())
                    .hover(|row| row.bg(rgba(CARD_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                    .child(status_dot(tool.state))
                    .child(
                        div()
                            .text_size(zpx(TEXT_MD))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(STRONG))
                            .child(tool.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgba(MUTED))
                            .child(tool.input),
                    )
                    .child(
                        div()
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgba(state_color))
                            .child(state_label),
                    ),
            )
            .when(tool.expanded && has_output, |card| {
                card.child(
                    div()
                        .px_2()
                        .pb_2()
                        .font_family(MONO)
                        .text_size(zpx(TEXT_SM))
                        .line_height(relative(1.5))
                        .text_color(rgba(MUTED))
                        .child(tool.output),
                )
            })
    }

    fn file_card(
        &self,
        ix: usize,
        change: FileChangeBlock,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (kind_label, kind_color) = match change.kind {
            FileChangeKind::Created => ("Created", GREEN),
            FileChangeKind::Modified => ("Modified", AMBER),
            FileChangeKind::Deleted => ("Deleted", RED),
        };
        let has_diff = !change.lines.is_empty();
        v_flex()
            .rounded_lg()
            .bg(rgba(CARD))
            .child(
                h_flex()
                    .id(("file", ix))
                    .h(zpx(30.0))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .when(has_diff, |row| row.cursor_pointer())
                    .hover(|row| row.bg(rgba(CARD_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                    .child(
                        div()
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgba(kind_color))
                            .child(kind_label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgb(STRONG))
                            .child(change.path.display().to_string()),
                    )
                    .when(has_diff, |row| {
                        row.child(
                            h_flex()
                                .gap_1p5()
                                .font_family(MONO)
                                .text_size(zpx(TEXT_SM))
                                .child(
                                    div()
                                        .text_color(rgba(GREEN))
                                        .child(format!("+{}", change.added)),
                                )
                                .child(
                                    div()
                                        .text_color(rgba(RED))
                                        .child(format!("-{}", change.removed)),
                                ),
                        )
                    }),
            )
            .when(change.expanded && has_diff, |card| {
                card.child(
                    v_flex()
                        .id(("diff", ix))
                        .pb_1()
                        .max_h(zpx(220.0))
                        .overflow_y_scroll()
                        .children(change.lines.iter().map(|line| {
                            let (marker, fill, color) = match line.kind {
                                DiffKind::Added => ("+", 0x5fc27e1f, GREEN),
                                DiffKind::Removed => ("-", 0xf0716b1f, RED),
                                DiffKind::Context => (" ", 0x00000000, MUTED),
                                DiffKind::Hunk => ("", 0x00000000, FAINT),
                            };
                            h_flex()
                                .px_2()
                                .gap_2()
                                .bg(rgba(fill))
                                .font_family(MONO)
                                .text_size(zpx(TEXT_SM))
                                .line_height(relative(1.5))
                                .text_color(rgba(color))
                                .child(div().w(zpx(8.0)).flex_none().child(marker))
                                .child(div().whitespace_nowrap().child(line.text.clone()))
                        })),
                )
            })
    }

    fn approval_card(&self, approval: ApprovalBlock, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match approval.kind {
            ApprovalKind::Command => "Run this command?",
            ApprovalKind::FileChange => "Apply these file changes?",
            ApprovalKind::Other => "Approval needed",
        };
        let pending = approval.state == ApprovalState::Pending;
        let (approve_id, deny_id) = (approval.id.clone(), approval.id.clone());
        v_flex()
            .p_2p5()
            .gap_2()
            .rounded_lg()
            .bg(rgba(0xe8b04a14))
            .child(
                div()
                    .text_size(zpx(TEXT_MD))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(STRONG))
                    .child(title),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(0x00000040))
                    .font_family(MONO)
                    .text_size(zpx(TEXT_SM))
                    .text_color(rgb(TEXT))
                    .child(approval.detail),
            )
            .child(if pending {
                h_flex()
                    .gap_1()
                    .child(pill("approve", "Approve", true).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.respond(approve_id.clone(), ApprovalResponse::Approve, cx)
                        },
                    )))
                    .child(pill("deny", "Deny", false).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.respond(deny_id.clone(), ApprovalResponse::Deny, cx)
                        },
                    )))
                    .into_any_element()
            } else {
                let (label, color) = if approval.state == ApprovalState::Approved {
                    ("Approved", GREEN)
                } else {
                    ("Denied", RED)
                };
                div()
                    .text_size(zpx(TEXT_SM))
                    .text_color(rgba(color))
                    .child(label)
                    .into_any_element()
            })
    }
}

fn pill(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(zpx(24.0))
        .px_3()
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(zpx(TEXT_MD))
        .font_weight(FontWeight::MEDIUM)
        .when(primary, |p| p.bg(rgb(0xffffff)).text_color(rgb(0x111113)))
        .when(!primary, |p| {
            p.bg(rgba(0xffffff14))
                .text_color(rgb(STRONG))
                .hover(|p| p.bg(rgba(0xffffff1f)))
        })
        .child(label)
}

fn status_dot(state: ToolState) -> AnyElement {
    match state {
        ToolState::Running => Spinner::new()
            .xsmall()
            .color(rgba(AMBER).into())
            .into_any_element(),
        ToolState::Ok => dot(GREEN),
        ToolState::Error => dot(RED),
        ToolState::Declined => dot(0xffffff59),
    }
}

fn dot(rgba_color: u32) -> AnyElement {
    div()
        .size(zpx(8.0))
        .flex_none()
        .rounded_full()
        .bg(rgba(rgba_color))
        .into_any_element()
}

fn user_message(text: String, steer: bool) -> impl IntoElement {
    h_flex()
        .items_start()
        .gap_2()
        .p_2p5()
        .rounded_lg()
        .bg(rgba(0xffffff12))
        .when(steer, |row| {
            row.child(
                svg()
                    .path("icons/queue.svg")
                    .size(zpx(14.0))
                    .mt(zpx(3.5))
                    .flex_none()
                    .text_color(rgba(0xe8b04ab3)),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(zpx(TEXT_MD))
                .line_height(relative(1.5))
                .text_color(rgb(STRONG))
                .child(text),
        )
}

const STREAM_FADE: std::time::Duration = std::time::Duration::from_millis(400);

fn first_chunk_opacity(elapsed: std::time::Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / STREAM_FADE.as_secs_f32()).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn assistant_message(
    ix: usize,
    text: String,
    started: Option<std::time::Instant>,
) -> impl IntoElement {
    let style = TextViewStyle::default().heading_font_size(|level, _| {
        if level <= 2 {
            zpx(TEXT_LG)
        } else {
            zpx(TEXT_MD)
        }
    });
    // FlowToken's fadeIn/diff effect, shortened to 400 ms for a quicker response.
    // https://github.com/Ephibbs/flowtoken/tree/a966002ca6fbbe1c78f4605a3849d14edd90260b
    // Native text motion fades appended glyphs, preserves Markdown selection,
    // and honors Reduce Motion. Keep it enabled after completion so the tail settles.
    let motion = TextViewMotion::default()
        .with_stream_fade(STREAM_FADE)
        .with_stream_fade_easing(Easing::EaseInOut);
    let view = div()
        .px_2()
        .text_size(zpx(TEXT_MD))
        .text_color(rgb(TEXT))
        .child(
            TextView::markdown(("markdown", ix), text)
                .style(style)
                .motion(motion)
                .selectable(true),
        );
    // TextView fades appended glyphs but treats its initial text as settled.
    // Arrival time prevents re-renders or scrolling from replaying this fade.
    if let Some(start) = started {
        view.with_animation(
            ("first-stream-chunk", ix),
            Animation::new(STREAM_FADE),
            move |view, _| view.opacity(first_chunk_opacity(start.elapsed())),
        )
        .into_any_element()
    } else {
        view.into_any_element()
    }
}

fn turn_end(status: TurnStatus) -> impl IntoElement {
    let label = match status {
        TurnStatus::Completed => "Completed",
        TurnStatus::Interrupted => "Stopped by you",
        TurnStatus::Failed => "Turn failed",
    };
    div()
        .px_2()
        .text_size(zpx(TEXT_SM))
        .text_color(rgba(FAINT))
        .child(label)
}

/// The opening message is rendered before there is a backend session to follow.
pub fn pending_prompt(text: String, error: Option<String>) -> impl IntoElement {
    let failed = error.is_some();
    v_flex()
        .size_full()
        .child(
            div()
                .id("pending-message")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    h_flex()
                        .w_full()
                        .pt_2()
                        .child(div().w(zpx(24.)).flex_none())
                        .child(
                            h_flex().flex_1().min_w_0().justify_center().px_6().child(
                                div()
                                    .w_full()
                                    .max_w(zpx(COLUMN_MAX_WIDTH))
                                    .child(user_message(text, false)),
                            ),
                        ),
                ),
        )
        .child(
            v_flex().w_full().items_center().px_3().child(
                h_flex()
                    .w_full()
                    .max_w(zpx(COLUMN_MAX_WIDTH))
                    .h(zpx(20.))
                    .px_2()
                    .gap_1p5()
                    .items_center()
                    .text_size(zpx(TEXT_SM))
                    .text_color(rgba(MUTED))
                    .when(!failed, |row| {
                        row.child(Spinner::new().xsmall()).child("Starting agent…")
                    }),
            ),
        )
}

#[cfg(test)]
mod stream_tests {
    #[test]
    fn first_chunk_starts_transparent_and_settles_smoothly() {
        use std::time::Duration;
        assert_eq!(super::first_chunk_opacity(Duration::ZERO), 0.);
        assert!((super::first_chunk_opacity(Duration::from_millis(200)) - 0.5).abs() < 0.001);
        assert_eq!(super::first_chunk_opacity(Duration::from_millis(400)), 1.);
        assert_eq!(super::first_chunk_opacity(Duration::from_secs(1)), 1.);
    }
}
