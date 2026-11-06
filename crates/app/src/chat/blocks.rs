//! Rendering of one transcript row per `Block`.

use super::attachments::AttachmentSummary;
use super::model::{ApprovalBlock, ApprovalState, Block, DiffKind, FileChangeBlock, ToolCall};
use super::view::{COLUMN_MAX_WIDTH, ChatView};
use crate::appearance::ActionTone;
use crate::appearance::{rgb, rgba};
use crate::keyboard_control::KeyboardControl as _;
use crate::zoom::px as zpx;
use del_agent::{ApprovalKind, ApprovalResponse, FileChangeKind, TurnStatus};
use del_core::typography::{PROSE_LEADING, TEXT_LG, TEXT_MD, TEXT_PROSE, TEXT_SM, TEXT_XL};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const TEXT: u32 = 0xe8e8e8;
const STRONG: u32 = 0xf5f5f5;
/// Fill of cards (tool calls, diffs, the user's message).
const CARD: u32 = 0xffffff0d;
const CARD_HOVER: u32 = 0xffffff14;
/// The user's message: one step above the page.
const USER_CARD: u32 = 0xffffff12;
/// Secondary reading text: code context, tool output, error detail.
const BODY_MUTED: u32 = 0xffffffb3;
const ROW_HEIGHT: f32 = crate::ui::ROW_COMPACT;
const ROW_GAP: f32 = crate::ui::SPACE_SM;
pub(super) const TRANSCRIPT_TOP: f32 = 16.;
const MONO: &str = del_core::typography::MONO_FONT;
/// Every transcript row starts its text here, so prose, tool rows, the user's
/// card and the composer share one left edge.
const TEXT_INSET: f32 = 12.;
/// Diff lines drawn in an expanded file card; the rest are summarized.
const MAX_DIFF_LINES: usize = 400;
/// Approval details taller than this scroll inside the card.
const APPROVAL_DETAIL_MAX: f32 = 200.;

/// A peer receipt's two ends, resolved once when the receipt arrives rather
/// than reading the session index on every frame.
#[derive(Clone)]
pub(super) struct PeerRow {
    current: del_agent::SessionId,
    target: del_agent::SessionId,
    current_name: SharedString,
    target_name: SharedString,
    current_provider: Option<del_agent::Provider>,
    target_provider: Option<del_agent::Provider>,
}

impl PeerRow {
    pub(super) fn resolve(
        manager: &del_agent::SessionManager,
        session: Option<&del_agent::SessionId>,
        message: &del_agent::peers::PeerMessage,
    ) -> Self {
        let current = manager.resolve_peer(session.unwrap_or(&message.to));
        let from = manager.resolve_peer(&message.from);
        let target = manager.resolve_peer(if current == from {
            &message.to
        } else {
            &message.from
        });
        let name = |id: &del_agent::SessionId| -> SharedString {
            manager
                .session_record(id)
                .ok()
                .and_then(|r| r.title)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| {
                    if manager.is_temporary(id) {
                        "Side chat".into()
                    } else {
                        "New session".into()
                    }
                })
                .into()
        };
        Self {
            current_name: name(&current),
            target_name: name(&target),
            current_provider: manager.session_record(&current).ok().map(|r| r.provider),
            target_provider: manager.session_record(&target).ok().map(|r| r.provider),
            current,
            target,
        }
    }
}

/// Full tool input and output, inside a height-limited scrolling viewport.
#[derive(Clone)]
pub(super) struct ToolDetails {
    markdown: SharedString,
}

impl ToolDetails {
    fn new(tool: &ToolCall) -> Self {
        let text = if tool.output.is_empty() {
            tool.input.clone()
        } else {
            format!("{}\n\n{}", tool.input, tool.output)
        };
        let markdown = if text.trim().is_empty() {
            "No output".to_owned()
        } else {
            code_fence(&text)
        };
        Self {
            markdown: markdown.into(),
        }
    }
}

/// Wrap literal text in a code fence longer than any backtick run inside it,
/// so it renders verbatim (and selectable) whatever it contains.
fn code_fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        run = if ch == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    // No language: a "text" label above commands and output is noise.
    format!("{fence}\n{text}\n{fence}")
}

/// "N more lines" under a capped block.
fn more_lines(hidden: usize) -> Div {
    div()
        .px(zpx(TEXT_INSET))
        .pt(zpx(crate::ui::SPACE_XS))
        .text_size(zpx(TEXT_SM))
        .text_color(crate::ui::faint_text())
        .child(format!(
            "{} more",
            crate::ui::plural(hidden, "line", "lines")
        ))
}

/// The message a failed turn's row shows: its last error, if any.
pub(super) fn failed_turn_error(blocks: &[Block]) -> Option<String> {
    blocks
        .iter()
        .rev()
        .take_while(|block| !matches!(block, Block::User { .. }))
        .find_map(|block| match block {
            Block::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
}

/// User text as Markdown that renders literally: punctuation is escaped,
/// line breaks stay breaks and leading spaces never become code blocks.
/// `range` (a byte range of `text`) is mapped into the result.
pub(super) fn literal_markdown(
    text: &str,
    range: std::ops::Range<usize>,
) -> (String, std::ops::Range<usize>) {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut start = 0;
    let mut end = 0;
    let mut line_start = true;
    for (ix, ch) in text.char_indices() {
        if ix == range.start {
            start = out.len();
        }
        match ch {
            '\n' => {
                out.push_str("\\\n");
                line_start = true;
            }
            ' ' | '\t' if line_start => out.push('\u{a0}'),
            '\r' => {}
            _ => {
                line_start = false;
                if ch.is_ascii_punctuation() {
                    out.push('\\');
                }
                out.push(ch);
            }
        }
        if ix + ch.len_utf8() == range.end {
            end = out.len();
        }
    }
    (out, start..end)
}

/// Rows whose content belongs in a selectable text view, keyed by row.
fn selectable_text(id: impl Into<ElementId>, markdown: impl Into<SharedString>) -> TextView {
    TextView::markdown(id, markdown)
        .style(prose_style())
        .stream_fade(false)
        .selectable(true)
}

/// A collapsible transcript row: the reveal tool groups use, and nothing
/// mounted below a closed row once its spring has settled.
fn disclosure_body(
    id: impl Into<ElementId>,
    open: bool,
    header: impl IntoElement,
    body: Option<AnyElement>,
) -> gpui_kit::component::collapsible::Collapsible {
    gpui_kit::component::collapsible::Collapsible::new()
        .motion_id(id)
        .open(open)
        .child(header)
        .content(body.unwrap_or_else(|| div().into_any_element()))
}

impl ChatView {
    /// One list row: the block, centered in the transcript column.
    pub(super) fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        if let Some(section) = self
            .work_sections
            .iter()
            .find(|r| r.range().contains(&ix) && r.end - r.start > 1)
            .cloned()
        {
            let expanded = self.expanded_work.contains(&section.start);
            let motion = self.work_reveals.get(&section.start);
            let progress = motion.map_or(if expanded { 1. } else { 0. }, |reveal| {
                reveal.sample(std::time::Instant::now()).0
            });
            // The provider's checklist stays readable outside folded tool work.
            let content = if ix == section.answer
                || matches!(self.model.blocks.get(ix), Some(Block::Plan { .. }))
            {
                self.render_block_row(ix, cx)
            } else if expanded || motion.is_some() {
                gpui_kit::base::MotionReveal::new(
                    ("work-row-reveal", ix),
                    progress,
                    // The reveal measures its child on its own, so the row
                    // must claim the column's width or it shrinks to content.
                    div()
                        .w_full()
                        .opacity(progress)
                        .child(self.render_block_row(ix, cx))
                        .into_any_element(),
                )
                .into_any_element()
            } else {
                div().h_0().into_any_element()
            };
            if ix != section.start {
                return content;
            }
            let label = self
                .work_elapsed
                .get(&section.answer)
                .map(|elapsed| super::work_history::duration_label(*elapsed))
                .unwrap_or_else(|| "Worked".into());
            let range = section.range();
            let header = h_flex()
                .id(("work-history", section.start))
                .keyboard_control()
                .role(Role::Button)
                .aria_label(if expanded {
                    "Collapse work details"
                } else {
                    "Expand work details"
                })
                .w_full()
                .h(zpx(ROW_HEIGHT))
                .px(zpx(TEXT_INSET))
                .gap(zpx(ROW_GAP))
                .items_center()
                .cursor_pointer()
                .rounded(zpx(crate::ui::RADIUS_ROW))
                .text_size(zpx(TEXT_MD))
                .text_color(crate::ui::secondary_text())
                .hover(|row| row.bg(rgba(CARD)))
                .child(label)
                .child(disclosure(("work-chevron", section.start), expanded))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_work(range.clone(), cx);
                }));
            let header = h_flex()
                .w_full()
                .justify_center()
                .px(zpx(crate::ui::SPACE_LG))
                .pb(zpx(crate::ui::SPACE_MD))
                .child(
                    v_flex()
                        .w_full()
                        .max_w(zpx(COLUMN_MAX_WIDTH))
                        .child(header)
                        .child(
                            div()
                                .h(zpx(1.))
                                .mt(zpx(crate::ui::SPACE_2XS))
                                .mx(zpx(TEXT_INSET))
                                .bg(crate::appearance::separator()),
                        ),
                );
            return v_flex()
                .w_full()
                .child(header)
                .child(content)
                .into_any_element();
        }
        self.render_block_row(ix, cx)
    }

    fn render_block_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        // Keep the streamed entity; don't copy the entire growing answer each frame.
        if !self
            .search_flash
            .as_ref()
            .is_some_and(|flash| flash.ix == ix)
            && matches!(self.model.blocks.get(ix), Some(Block::Assistant { .. }))
            && let Some(state) = self.ensure_assistant_text(ix, cx)
        {
            let state = &state;
            return h_flex()
                .w_full()
                .justify_center()
                .px(zpx(crate::ui::SPACE_LG))
                .pb(zpx(crate::ui::SPACE_SM))
                .child(
                    div()
                        .w_full()
                        .max_w(zpx(COLUMN_MAX_WIDTH))
                        .child(assistant_message(
                            ix,
                            String::new(),
                            Some(state),
                            cx.entity().downgrade(),
                            ResponseFooter {
                                replied_at: self.response_times.get(&ix).copied(),
                                hovered: self.response_hover.get(&ix).copied(),
                                copied: self.copied_responses.contains_key(&ix),
                                completed: self.final_response(ix),
                                reduced: cx.reduce_motion(),
                            },
                        )),
                )
                .into_any_element();
        }
        // Borrow the row; cloning every visible block each frame copies whole
        // tool outputs and diffs that are usually collapsed.
        let Some(block) = self.model.blocks.get(ix) else {
            return div().into_any_element();
        };
        let jumping = self.jump_in_flight();
        if let Some(flash) = self.search_flash.as_mut().filter(|flash| flash.ix == ix) {
            // Reveal the match only once the eased jump has landed on its row.
            if !flash.applied && !jumping {
                flash.applied = flash.text_view.update(cx, |state, cx| {
                    let Some(range) = state.rendered_text().range_for_source(flash.range.clone())
                    else {
                        return false;
                    };
                    let _ = state.set_range_highlights(
                        [gpui_kit::base::text::RangeHighlight::new(
                            range.clone(),
                            rgba(0xe8b04a60),
                        )],
                        cx,
                    );
                    let _ = state.reveal_range(range, cx);
                    true
                });
                if flash.applied {
                    let view = flash.text_view.clone();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1400))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if this
                                .search_flash
                                .as_ref()
                                .is_some_and(|flash| flash.text_view == view)
                            {
                                this.search_flash = None;
                                this.scroller.remeasure_items(ix..ix + 1);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
            let style = prose_style();
            return h_flex()
                .w_full()
                .justify_center()
                .px(zpx(crate::ui::SPACE_LG))
                .child(
                    div()
                        .w_full()
                        .max_w(zpx(COLUMN_MAX_WIDTH))
                        .px(zpx(TEXT_INSET))
                        .rounded(zpx(crate::ui::RADIUS_CARD))
                        .when(matches!(block, Block::User { .. }), |row| {
                            row.py(zpx(crate::ui::SPACE_MD)).bg(rgba(USER_CARD))
                        })
                        .text_size(zpx(TEXT_PROSE))
                        .line_height(relative(PROSE_LEADING))
                        .text_color(rgb(TEXT))
                        .child(
                            TextView::new(&flash.text_view)
                                .style(style)
                                .code_block_actions(code_block_actions)
                                .selectable(true)
                                .on_link_click({
                                    let chat = cx.entity().downgrade();
                                    move |url, _, _, cx| {
                                        cx.stop_propagation();
                                        let _ = chat.update(cx, |_, cx| {
                                            cx.emit(super::view::OpenLink(url.to_string()))
                                        });
                                    }
                                }),
                        ),
                )
                .into_any_element();
        }
        if let Block::FileChange(change) = block
            && ix > 0
            && matches!(&self.model.blocks[ix - 1], Block::Tool(tool) if super::activity::matches_file(tool, change))
        {
            return div().h_0().into_any_element();
        }
        if let Some(group) = super::activity::group_at(&self.tool_groups, ix)
            .filter(|group| group.end <= self.model.blocks.len())
        {
            if ix != group.start {
                return div().h_0().into_any_element();
            }
            let expanded = self.expanded_activity.contains(&group.start);
            let label = super::activity::group_label(&self.model.blocks[group.clone()]);
            let live = self.model.blocks[group.clone()].iter().any(|block| matches!(block, Block::Tool(tool) if tool.state == super::model::ToolState::Running));
            let range = group.clone();
            let header = h_flex()
                .id(("activity-group", ix))
                .keyboard_control()
                .h(zpx(ROW_HEIGHT))
                .px(zpx(TEXT_INSET))
                .gap(zpx(ROW_GAP))
                .items_center()
                .cursor_pointer()
                .rounded(zpx(crate::ui::RADIUS_ROW))
                .text_size(zpx(TEXT_MD))
                .text_color(crate::ui::secondary_text())
                .hover(|row| row.bg(rgba(CARD)))
                .child(if live {
                    gpui_kit::component::shimmer::ShimmerText::new(label)
                        .id(("activity-live", ix))
                        .min_w_0()
                        .truncate()
                        .highlight_color(rgb(STRONG))
                        .duration(crate::ui::SHIMMER_PERIOD)
                        .into_any_element()
                } else {
                    // A failure count is news; the summary before it is not.
                    let summary = label.clone();
                    let failures = super::activity::failure_note(&self.model.blocks[group.clone()]);
                    h_flex()
                        .min_w_0()
                        .gap(zpx(crate::ui::SPACE_XS))
                        .child(div().min_w_0().truncate().child(summary))
                        .when_some(failures, |row, failures| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_color(ActionTone::Danger.foreground())
                                    .child(failures),
                            )
                        })
                        .into_any_element()
                })
                .child(disclosure(("activity-chevron", ix), expanded))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Keep the header under the pointer while the group opens.
                    this.scroller.pause_following_tail();
                    if !this.expanded_activity.remove(&ix) {
                        this.expanded_activity.insert(ix);
                    }
                    // The virtual list caches heights; remeasure each frame of the reveal.
                    this.settle(range.clone(), cx);
                    cx.notify();
                }));
            let mut children = v_flex()
                .gap(zpx(crate::ui::SPACE_XS))
                .pt(zpx(crate::ui::SPACE_XS));
            if expanded || self.is_settling(ix) {
                for at in group {
                    if let Block::Tool(tool) = &self.model.blocks[at] {
                        children = children.child(self.tool_card(at, tool, cx));
                    }
                }
            }
            let content = gpui_kit::component::collapsible::Collapsible::new()
                .motion_id(("tool-group-reveal", ix))
                .open(expanded)
                .child(header)
                .content(children);
            return h_flex()
                .w_full()
                .justify_center()
                .px(zpx(crate::ui::SPACE_LG))
                .pb(zpx(crate::ui::SPACE_2XS))
                .child(div().w_full().max_w(zpx(COLUMN_MAX_WIDTH)).child(content))
                .into_any_element();
        }
        let compact = matches!(
            block,
            Block::Tool(_) | Block::FileChange(_) | Block::Reasoning { .. }
        );
        let content = match block {
            Block::Plan { plan, .. } => {
                super::plan::card(ix, plan, self.model.plan_is_live(ix), cx.reduce_motion())
            }
            Block::ProviderSwitch { from, to } => h_flex()
                .w_full()
                .justify_center()
                .py(zpx(crate::ui::SPACE_MD))
                .gap(zpx(crate::ui::SPACE_MD))
                .text_size(zpx(TEXT_SM))
                .text_color(crate::ui::secondary_text())
                .child("Switched from")
                .child(crate::composer_ui::provider_logo(*from).size(zpx(14.)))
                .child(crate::providers::display_name(*from))
                .child("⇄")
                .child(crate::composer_ui::provider_logo(*to).size(zpx(14.)))
                .child(crate::providers::display_name(*to))
                .into_any_element(),
            Block::Peer(message) => {
                let PeerRow {
                    current,
                    target,
                    current_name,
                    target_name,
                    current_provider,
                    target_provider,
                } = self.peer_rows.get(&message.id).cloned().unwrap_or_else(|| {
                    PeerRow::resolve(&self.backend.manager, self.session.as_ref(), message)
                });
                let recovery = matches!(
                    message.state,
                    del_agent::peers::DeliveryState::Uncertain
                        | del_agent::peers::DeliveryState::Failed
                );
                let receipt_id = message.id.clone();
                let owner = cx.entity().downgrade();
                let row = h_flex()
                    .id(SharedString::from(format!("peer-{}", message.id)))
                    .w_full()
                    .justify_center()
                    .py(zpx(crate::ui::SPACE_MD))
                    .text_size(zpx(TEXT_SM))
                    .text_color(crate::ui::secondary_text())
                    .child(
                        h_flex()
                            .max_w_full()
                            .gap(zpx(crate::ui::SPACE_MD))
                            .items_center()
                            .child(
                                h_flex()
                                    .gap(zpx(crate::ui::SPACE_MD))
                                    .items_center()
                                    .id(SharedString::from(format!("peer-current-{}", message.id)))
                                    .keyboard_control()
                                    .min_w_0()
                                    .max_w(zpx(200.))
                                    .cursor_pointer()
                                    .role(Role::Button)
                                    .aria_label(format!("Open conversation: {current_name}"))
                                    .hover(|name| name.text_color(rgb(TEXT)))
                                    .when_some(current_provider, |row, provider| {
                                        row.child(
                                            crate::composer_ui::provider_logo(provider)
                                                .size(zpx(14.)),
                                        )
                                    })
                                    .child(div().min_w_0().truncate().child(current_name))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(super::view::OpenPeer(current.clone()))
                                    })),
                            )
                            .child(div().flex_none().child("⇄"))
                            .child(
                                h_flex()
                                    .gap(zpx(crate::ui::SPACE_MD))
                                    .items_center()
                                    .id(SharedString::from(format!("peer-other-{}", message.id)))
                                    .keyboard_control()
                                    .min_w_0()
                                    .max_w(zpx(200.))
                                    .cursor_pointer()
                                    .role(Role::Button)
                                    .aria_label(format!("Open peer conversation: {target_name}"))
                                    .hover(|name| name.text_color(rgb(TEXT)))
                                    .when_some(target_provider, |row, provider| {
                                        row.child(
                                            crate::composer_ui::provider_logo(provider)
                                                .size(zpx(14.)),
                                        )
                                    })
                                    .child(div().min_w_0().truncate().child(target_name))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(super::view::OpenPeer(target.clone()))
                                    })),
                            ),
                    );
                if !recovery {
                    row.into_any_element()
                } else {
                    row.context_menu(move |menu, _, _| {
                        let retry_owner = owner.clone();
                        let retry_id = receipt_id.clone();
                        let dismiss_owner = owner.clone();
                        let dismiss_id = receipt_id.clone();
                        menu.item(
                            PopupMenuItem::new("Retry after checking recipient").on_click(
                                move |_, _, cx| {
                                    let _ = retry_owner.update(cx, |this, cx| {
                                        if let Err(error) = this
                                            .backend
                                            .manager
                                            .recover_peer_message(&retry_id, true)
                                        {
                                            this.pending_errors.push(error.to_string());
                                        }
                                        cx.notify();
                                    });
                                },
                            ),
                        )
                        .item(
                            PopupMenuItem::new("Dismiss delivery").on_click(move |_, _, cx| {
                                let _ = dismiss_owner.update(cx, |this, cx| {
                                    if let Err(error) = this
                                        .backend
                                        .manager
                                        .recover_peer_message(&dismiss_id, false)
                                    {
                                        this.pending_errors.push(error.to_string());
                                    }
                                    cx.notify();
                                });
                            }),
                        )
                    })
                    .into_any_element()
                }
            }

            Block::User {
                text,
                steer,
                goal,
                attachments,
            } => v_flex()
                .w_full()
                .gap(zpx(crate::ui::SPACE_MD))
                .child(user_message(
                    Some(ix),
                    text.clone(),
                    *steer,
                    *goal,
                    attachments.clone(),
                    !self.compact_user_messages,
                ))
                .when_some(self.working_label(ix), |column, label| {
                    column.child(
                        h_flex()
                            .px(zpx(TEXT_INSET))
                            .h(zpx(ROW_HEIGHT))
                            .items_center()
                            .text_size(zpx(TEXT_MD))
                            .text_color(crate::ui::faint_text())
                            .child(
                                gpui_kit::component::shimmer::ShimmerText::new(label)
                                    .id(SharedString::from(format!("working-shimmer-{ix}")))
                                    .flex_none()
                                    .highlight_color(rgb(STRONG))
                                    .spread(0.22)
                                    .duration(crate::ui::SHIMMER_PERIOD),
                            ),
                    )
                })
                .into_any_element(),
            Block::Assistant { text, .. } => assistant_message(
                ix,
                text.clone(),
                self.assistant_text.get(&ix),
                cx.entity().downgrade(),
                ResponseFooter {
                    replied_at: self.response_times.get(&ix).copied(),
                    hovered: self.response_hover.get(&ix).copied(),
                    copied: self.copied_responses.contains_key(&ix),
                    completed: self.final_response(ix),
                    reduced: cx.reduce_motion(),
                },
            )
            .into_any_element(),
            Block::Reasoning {
                text,
                open,
                expanded,
            } => self
                .reasoning(ix, text, *open, *expanded, cx)
                .into_any_element(),
            Block::Tool(tool) => self.tool_card(ix, tool, cx).into_any_element(),
            Block::FileChange(change) => self
                .file_card(ix, change, change.expanded, cx)
                .into_any_element(),
            Block::Approval(approval) => self.approval_card(approval, cx).into_any_element(),
            Block::Error { message, .. } if self.is_load_failure(ix) => failure_row(
                "Could not open this conversation",
                Some(message.clone()),
                Some(Box::new(cx.listener(|this, _, _, cx| this.retry_load(cx)))),
            )
            .into_any_element(),
            // A fatal error outside a failed turn has no other row to say so.
            Block::Error {
                message,
                recoverable: false,
            } if !self.model.blocks[ix + 1..]
                .iter()
                .take_while(|block| !matches!(block, Block::User { .. }))
                .any(|block| matches!(block, Block::TurnEnd { .. })) =>
            {
                failure_row("Error", Some(message.clone()), None).into_any_element()
            }
            Block::Error { .. } => div().h_0().into_any_element(),
            Block::TurnEnd {
                status: TurnStatus::Failed,
            } => failure_row(
                "Turn failed",
                failed_turn_error(&self.model.blocks[..ix]),
                self.can_retry_turn(ix).then(|| {
                    Box::new(cx.listener(|this, _, _, cx| this.retry_last(cx))) as ClickHandler
                }),
            )
            .into_any_element(),
            Block::TurnEnd { status } => turn_end(*status).into_any_element(),
        };
        // A new turn gets more air than the steps inside one.
        let opens_turn = ix > 0 && matches!(block, Block::User { .. });
        let closes_turn = matches!(block, Block::TurnEnd { .. });
        h_flex()
            .w_full()
            .justify_center()
            .px(zpx(crate::ui::SPACE_LG))
            .when(opens_turn, |row| row.pt(zpx(14.)))
            .pb(if compact {
                zpx(crate::ui::SPACE_2XS)
            } else if closes_turn {
                zpx(10.)
            } else {
                zpx(crate::ui::SPACE_SM)
            })
            .child(div().w_full().max_w(zpx(COLUMN_MAX_WIDTH)).child(content))
            .into_any_element()
    }

    fn reasoning(
        &self,
        ix: usize,
        text: &str,
        open: bool,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let header = h_flex()
            .id(("reasoning", ix))
            .keyboard_control()
            .px(zpx(TEXT_INSET))
            .h(zpx(ROW_HEIGHT))
            .items_center()
            .gap(zpx(ROW_GAP))
            .rounded(zpx(crate::ui::RADIUS_ROW))
            .cursor_pointer()
            .text_size(zpx(TEXT_MD))
            .text_color(crate::ui::secondary_text())
            .hover(|row| row.bg(rgba(CARD)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
            .role(Role::Button)
            .aria_label(format!(
                "{} {}",
                if expanded { "Collapse" } else { "Expand" },
                if open { "thinking" } else { "thought" }
            ))
            .child(if open {
                gpui_kit::component::shimmer::ShimmerText::new("Thinking")
                    .id(("reasoning-live", ix))
                    .highlight_color(rgb(STRONG))
                    .duration(crate::ui::SHIMMER_PERIOD)
                    .into_any_element()
            } else {
                div().child("Thought").into_any_element()
            })
            .child(disclosure(("reasoning-chevron", ix), expanded));
        let body = (expanded || self.is_settling(ix)).then(|| {
            div()
                .px(zpx(TEXT_INSET))
                .pb(zpx(crate::ui::SPACE_XS))
                .text_size(zpx(TEXT_MD))
                .line_height(relative(1.5))
                .text_color(crate::ui::secondary_text())
                .child(selectable_text(("reasoning-text", ix), text.to_owned()))
                .into_any_element()
        });
        disclosure_body(("reasoning-reveal", ix), expanded, header, body)
            .gap(zpx(crate::ui::SPACE_XS))
    }

    fn tool_card(&self, ix: usize, tool: &ToolCall, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, label) = super::activity::describe(tool);
        let failed = matches!(
            tool.state,
            super::model::ToolState::Error | super::model::ToolState::Declined
        );
        let file = match self.model.blocks.get(ix + 1) {
            Some(Block::FileChange(file))
                if tool.expanded && super::activity::matches_file(tool, file) =>
            {
                Some(file)
            }
            _ => None,
        };
        let header = h_flex()
            .id(("tool", ix))
            .keyboard_control()
            .role(Role::Button)
            .aria_label(format!(
                "{} {label}",
                if tool.expanded { "Collapse" } else { "Expand" }
            ))
            .h(zpx(ROW_HEIGHT))
            .px(zpx(TEXT_INSET))
            .gap(zpx(ROW_GAP))
            .items_center()
            .rounded(zpx(crate::ui::RADIUS_ROW))
            .cursor_pointer()
            .text_size(zpx(TEXT_MD))
            .text_color(crate::ui::secondary_text())
            .hover(|row| row.bg(rgba(CARD)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
            .child(match label.split_once(' ').filter(|_| failed) {
                // The verb says which row in the group went wrong.
                Some((verb, rest)) => h_flex()
                    .min_w_0()
                    .gap(zpx(crate::ui::SPACE_XS))
                    .child(
                        div()
                            .flex_none()
                            .text_color(ActionTone::Danger.foreground())
                            .child(verb.to_owned()),
                    )
                    .child(div().min_w_0().truncate().child(rest.to_owned()))
                    .into_any_element(),
                None => div()
                    .min_w_0()
                    .truncate()
                    .child(label.clone())
                    .into_any_element(),
            })
            .child(disclosure(("tool-chevron", ix), tool.expanded));
        // Built once per change, and only while the card is open or closing.
        let body = (tool.expanded || self.is_settling(ix)).then(|| {
            let signature = (tool.input.len(), tool.output.len());
            let details = {
                let mut cache = self.tool_details.borrow_mut();
                match cache.get(&ix) {
                    Some((input, output, details)) if (*input, *output) == signature => {
                        details.clone()
                    }
                    _ => {
                        let details = ToolDetails::new(tool);
                        cache.insert(ix, (signature.0, signature.1, details.clone()));
                        details
                    }
                }
            };
            v_flex()
                .child(
                    div()
                        .id(("tool-output", ix))
                        .debug_selector(|| "tool-output".to_owned())
                        .max_h(zpx(260.))
                        .overflow_y_scroll()
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .rounded(zpx(crate::ui::RADIUS_CARD))
                        .bg(crate::appearance::code_surface())
                        .text_size(zpx(TEXT_SM))
                        .line_height(relative(1.5))
                        .text_color(rgba(BODY_MUTED))
                        .child(selectable_text(("tool-text", ix), details.markdown)),
                )
                .when_some(file, |card, file| {
                    card.child(self.file_card(ix + 1, file, true, cx))
                })
                .into_any_element()
        });
        disclosure_body(("tool-reveal", ix), tool.expanded, header, body)
            .gap(zpx(crate::ui::SPACE_XS))
    }

    fn file_card(
        &self,
        ix: usize,
        change: &FileChangeBlock,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind_label = match change.kind {
            FileChangeKind::Created => "Created",
            FileChangeKind::Modified => "Modified",
            FileChangeKind::Deleted => "Deleted",
        };
        let has_diff = !change.lines.is_empty();
        let name = change
            .path
            .file_name()
            .unwrap_or(change.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let full_path = change.path.to_string_lossy().into_owned();
        // Opens in Del's file viewer through the transcript's local-link handling.
        let link = if change.path.is_absolute() {
            url::Url::from_file_path(&change.path)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| full_path.clone())
        } else {
            full_path.clone()
        };
        let file_link = div()
            .id(("file-link", ix))
            .min_w_0()
            .truncate()
            .cursor_pointer()
            .hover(|name| name.text_color(rgb(TEXT)).underline())
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(crate::ui::display_path(&full_path))
                    .build(window, cx)
            })
            .child(name)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(super::view::OpenLink(link.clone()));
            }));
        let header = h_flex()
            .id(("file", ix))
            .h(zpx(ROW_HEIGHT))
            .px(zpx(TEXT_INSET))
            .gap(zpx(ROW_GAP))
            .items_center()
            .rounded(zpx(crate::ui::RADIUS_ROW))
            .text_size(zpx(TEXT_MD))
            .text_color(crate::ui::secondary_text())
            .when(has_diff, |row| {
                row.cursor_pointer().hover(|row| row.bg(rgba(CARD)))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
            .child(
                h_flex()
                    .min_w_0()
                    .gap(zpx(crate::ui::SPACE_XS))
                    .child(div().flex_none().child(kind_label))
                    .child(file_link),
            )
            .when(change.added > 0 || change.removed > 0, |row| {
                row.child(
                    h_flex()
                        .flex_none()
                        .gap(zpx(crate::ui::SPACE_XS))
                        .text_size(zpx(TEXT_SM))
                        .when(change.added > 0, |counts| {
                            counts.child(
                                div()
                                    .text_color(ActionTone::Success.foreground())
                                    .child(format!("+{}", change.added)),
                            )
                        })
                        .when(change.removed > 0, |counts| {
                            counts.child(
                                div()
                                    .text_color(ActionTone::Danger.foreground())
                                    .child(format!("−{}", change.removed)),
                            )
                        }),
                )
            })
            .when(has_diff, |row| {
                row.child(disclosure(("file-chevron", ix), expanded))
            });
        let body = (has_diff && (expanded || self.is_settling(ix))).then(|| {
            let hidden = change.lines.len().saturating_sub(MAX_DIFF_LINES);
            v_flex()
                .mt(zpx(crate::ui::SPACE_2XS))
                .mb(zpx(crate::ui::SPACE_XS))
                .rounded(zpx(crate::ui::RADIUS_CARD))
                .overflow_hidden()
                .bg(crate::appearance::code_surface())
                .child(
                    // Long lines scroll sideways instead of being clipped.
                    div()
                        .id(("diff", ix))
                        .max_h(zpx(260.0))
                        .overflow_scroll()
                        .py(zpx(crate::ui::SPACE_SM))
                        .child(v_flex().min_w_full().flex_none().items_start().children(
                            change.lines.iter().take(MAX_DIFF_LINES).map(|line| {
                                let added = ActionTone::Success.foreground();
                                let removed = ActionTone::Danger.foreground();
                                let (marker, fill, color) = match line.kind {
                                    DiffKind::Added => ("+", Some(added.opacity(0.13)), added),
                                    DiffKind::Removed => {
                                        ("−", Some(removed.opacity(0.13)), removed)
                                    }
                                    DiffKind::Context => (" ", None, rgba(BODY_MUTED)),
                                    DiffKind::Hunk => ("", None, crate::ui::faint_text()),
                                };
                                h_flex()
                                    .min_w_full()
                                    .flex_none()
                                    .px(zpx(TEXT_INSET))
                                    .gap(zpx(crate::ui::SPACE_MD))
                                    .when_some(fill, |row, fill| row.bg(fill))
                                    .font_family(MONO)
                                    .text_size(zpx(TEXT_SM))
                                    .line_height(relative(1.5))
                                    .text_color(color)
                                    .when(line.kind != DiffKind::Hunk, |row| {
                                        row.child(div().w(zpx(8.0)).flex_none().child(marker))
                                    })
                                    .child(
                                        div()
                                            .flex_none()
                                            .whitespace_nowrap()
                                            .child(line.text.clone()),
                                    )
                            }),
                        )),
                )
                .when(hidden > 0, |diff| {
                    diff.child(more_lines(hidden).pb(zpx(crate::ui::SPACE_SM)))
                })
                .into_any_element()
        });
        disclosure_body(("file-reveal", ix), expanded && has_diff, header, body)
    }

    fn approval_card(&self, approval: &ApprovalBlock, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match approval.kind {
            ApprovalKind::Command => "Run this command?",
            ApprovalKind::FileChange => "Apply these file changes?",
            ApprovalKind::Other => "Approval needed",
        };
        let pending = approval.state == ApprovalState::Pending;
        let (approve_id, deny_id) = (approval.id.clone(), approval.id.clone());
        let attention = ActionTone::Warning.foreground();
        let card = v_flex()
            .w_full()
            .my(zpx(crate::ui::SPACE_XS))
            .p(zpx(TEXT_INSET))
            .gap(zpx(10.))
            .rounded(zpx(crate::ui::RADIUS_CARD))
            .bg(crate::appearance::composer_surface())
            .border_1()
            .border_color(if pending {
                attention.opacity(0.28)
            } else {
                crate::appearance::separator()
            })
            .child(
                h_flex()
                    .gap(zpx(crate::ui::SPACE_MD))
                    .items_center()
                    .text_size(zpx(TEXT_MD))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(STRONG))
                    .when(pending, |row| {
                        row.child(
                            crate::icons::path("icons/waiting-ring.svg")
                                .with_size(zpx(14.))
                                .text_color(attention),
                        )
                    })
                    .child(title),
            )
            .child(
                // Long commands and patches scroll inside the card; the text selects.
                div()
                    .id(SharedString::from(format!(
                        "approval-detail-{}",
                        approval.id
                    )))
                    .max_h(zpx(APPROVAL_DETAIL_MAX))
                    .overflow_y_scroll()
                    .rounded(zpx(crate::ui::RADIUS_ROW))
                    .bg(crate::appearance::code_surface())
                    .text_size(zpx(TEXT_SM))
                    .line_height(relative(1.5))
                    .text_color(rgb(TEXT))
                    .child(selectable_text(
                        SharedString::from(format!("approval-text-{}", approval.id)),
                        code_fence(&approval.detail),
                    )),
            )
            .child(if pending {
                h_flex()
                    .gap(zpx(crate::ui::SPACE_SM))
                    .child(
                        Button::new("approve")
                            .primary()
                            .small()
                            .label("Approve")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond(approve_id.clone(), ApprovalResponse::Approve, cx)
                            })),
                    )
                    .child(
                        Button::new("deny")
                            .ghost()
                            .small()
                            .bg(rgba(crate::appearance::QUIET_FILL))
                            .label("Deny")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.respond(deny_id.clone(), ApprovalResponse::Deny, cx)
                            })),
                    )
                    .into_any_element()
            } else {
                let (label, color) = match approval.state {
                    ApprovalState::Approved => ("Approved", ActionTone::Success.foreground()),
                    ApprovalState::Closed => ("No longer needed", crate::ui::secondary_text()),
                    _ => ("Denied", ActionTone::Danger.foreground()),
                };
                div()
                    .text_size(zpx(TEXT_SM))
                    .text_color(color)
                    .child(label)
                    .into_any_element()
            });
        // A new request grows in rather than pushing the transcript at once.
        let progress = if cx.reduce_motion() {
            1.
        } else {
            self.approval_entered.get(&approval.id).map_or(1., |at| {
                crate::ui::ease_out(at.elapsed().as_secs_f32() / crate::ui::REVEAL.as_secs_f32())
            })
        };
        gpui_kit::base::MotionReveal::new(
            SharedString::from(format!("approval-reveal-{}", approval.id)),
            progress,
            card.opacity(progress).into_any_element(),
        )
    }
}

/// The one disclosure arrow transcript rows use, directly after their text.
/// Its points interpolate from right to down, reversibly; Reduce Motion snaps.
fn disclosure(id: impl Into<ElementId>, expanded: bool) -> Chevron {
    Chevron {
        id: id.into(),
        expanded,
    }
}

#[derive(IntoElement)]
struct Chevron {
    id: ElementId,
    expanded: bool,
}

impl RenderOnce for Chevron {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = gpui_kit::base::transition(
            self.id,
            if self.expanded { 1. } else { 0. },
            gpui_kit::base::Transition::new(std::time::Duration::from_millis(180))
                .ease(crate::ui::ease_out),
            window,
            cx,
        );
        // Right-pointing (collapsed) to down-pointing (expanded), on a 16-unit grid.
        let mix = |a: [f32; 2], b: [f32; 2]| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let points = [
            mix([6., 4.], [4., 6.]),
            mix([10., 8.], [8., 10.]),
            mix([6., 12.], [12., 6.]),
        ];
        div()
            .size(zpx(11.))
            .flex_none()
            .text_color(crate::ui::faint_text())
            .child(
                canvas(
                    |_, window, _| window.text_style().color,
                    move |bounds, color, window, _| {
                        let scale = bounds.size.width.as_f32() / 16.;
                        let mut path = PathBuilder::stroke(gpui::px(1.5 * scale));
                        for (i, p) in points.into_iter().enumerate() {
                            let p = bounds.origin
                                + point(gpui::px(p[0] * scale), gpui::px(p[1] * scale));
                            if i == 0 {
                                path.move_to(p);
                            } else {
                                path.line_to(p);
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    },
                )
                .size_full(),
            )
    }
}

/// A code block's corner: its language and a Copy with the same confirmation
/// as a reply's. Passing actions also drops the extra top padding the bare
/// language label needs, so the code sits evenly in its block.
fn code_block_actions(
    block: &gpui_kit::base::text::CodeBlock,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    use std::hash::{Hash as _, Hasher as _};
    let code = block.code();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    let key = hasher.finish();
    // Counts clicks, so a repeated click restarts the confirmation; 0 is idle.
    let copies = window.use_keyed_state(
        ElementId::NamedInteger("code-copy".into(), key),
        cx,
        |_, _| 0u32,
    );
    let copied = *copies.read(cx) > 0;
    h_flex()
        .h(zpx(24.))
        .pl(zpx(8.))
        .gap(zpx(2.))
        .items_center()
        .rounded(zpx(crate::ui::RADIUS_ROW))
        .bg(crate::appearance::code_surface())
        .font_family(cx.theme().font_family.clone())
        .text_size(zpx(del_core::typography::TEXT_XS))
        .text_color(crate::appearance::secondary_text())
        .when_some(block.lang().filter(|lang| !lang.is_empty()), |row, lang| {
            row.child(lang)
        })
        .child(
            div()
                .id(ElementId::NamedInteger("code-copy-button".into(), key))
                .keyboard_control()
                .size(zpx(24.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(zpx(crate::ui::RADIUS_ROW))
                .cursor_pointer()
                .role(Role::Button)
                .aria_label(if copied { "Copied" } else { "Copy code" })
                .hover(|button| button.bg(rgba(CARD_HOVER)))
                .child(
                    crate::micro_motion::MorphIcon::new(
                        ElementId::NamedInteger("code-copy-icon".into(), key),
                        crate::animated_icons::Morph::CopyCheck,
                        copied,
                    )
                    .with_size(zpx(13.))
                    .text_color(crate::appearance::secondary_text()),
                )
                // Pressing Copy must not start a text selection in the block.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                    let click = copies.update(cx, |count, cx| {
                        *count = count.wrapping_add(1).max(1);
                        cx.notify();
                        *count
                    });
                    let copies = copies.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1500))
                            .await;
                        copies.update(cx, |count, cx| {
                            if *count == click {
                                *count = 0;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }),
        )
}

fn prose_style() -> TextViewStyle {
    TextViewStyle::default()
        .heading_font_size(|level, _| {
            zpx(match level {
                1 => TEXT_XL,
                2 => TEXT_LG,
                _ => TEXT_PROSE,
            })
        })
        // The same surface as diffs and tool output.
        .code_block(StyleRefinement::default().bg(crate::appearance::code_surface()))
        // A column heading is a label to read, not disabled text.
        .table_head(
            StyleRefinement::default()
                .text_color(crate::appearance::secondary_text())
                .font_weight(FontWeight::MEDIUM),
        )
}

/// The person's message: selectable text, plus a hover Copy in the corner
/// that copies the prompt exactly as written (space is always reserved).
fn user_message(
    ix: Option<usize>,
    text: String,
    steer: bool,
    goal: bool,
    attachments: AttachmentSummary,
    full_width: bool,
) -> impl IntoElement {
    let label = attachments.label();
    let objective = del_agent::split_command(&text).and_then(|(name, args)| {
        (name == "goal"
            && !args.is_empty()
            && !matches!(args, "clear" | "pause" | "resume" | "status"))
        .then(|| args.to_owned())
    });
    let sent_as_goal = goal || objective.is_some();
    let text = objective.unwrap_or(text);
    let group = SharedString::from(format!("user-message-{}", ix.unwrap_or(usize::MAX)));
    let copy = ix.filter(|_| !text.trim().is_empty()).map(|ix| PromptCopy {
        ix,
        text: text.clone(),
        group: group.clone(),
    });
    // Main-chat cards span the transcript; Side chats keep text-sized bubbles.
    h_flex()
        .group(group)
        .relative()
        .max_w_full()
        .when(full_width, |row| row.w_full())
        .self_start()
        .items_start()
        .gap(zpx(crate::ui::SPACE_MD))
        .pl(zpx(TEXT_INSET))
        .pr(zpx(if copy.is_some() { 32. } else { TEXT_INSET }))
        .py(zpx(crate::ui::SPACE_MD))
        .rounded(zpx(crate::ui::RADIUS_CARD))
        .bg(rgba(USER_CARD))
        .when(steer, |row| {
            row.child(
                crate::icons::path("icons/queue.svg")
                    .mt(zpx(3.5))
                    .flex_none()
                    .text_color(rgba(0xe8b04ab3)),
            )
        })
        .child(
            v_flex()
                // A full-width card hands its width to the text; a compact
                // Side chat bubble sizes itself to its plain text instead.
                .when(full_width, |column| column.flex_1())
                .flex_shrink_1()
                .min_w_0()
                .gap(zpx(crate::ui::SPACE_XS))
                .when(!text.is_empty(), |column| {
                    let (markdown, _) = literal_markdown(&text, 0..0);
                    column.child(
                        div()
                            .min_w_0()
                            .text_size(zpx(TEXT_PROSE))
                            .line_height(relative(PROSE_LEADING))
                            .text_color(rgb(STRONG))
                            // Selectable text measures at its narrowest inside a
                            // shrink-to-fit bubble, so only full-width cards use it.
                            .child(match ix.filter(|_| full_width) {
                                Some(ix) => {
                                    selectable_text(("user-text", ix), markdown).into_any_element()
                                }
                                None => div().child(text.clone()).into_any_element(),
                            }),
                    )
                })
                .when(sent_as_goal, |column| {
                    column.child(
                        div()
                            .text_size(zpx(del_core::typography::TEXT_XS))
                            .text_color(crate::ui::secondary_text())
                            .child("◎  Sent as goal"),
                    )
                })
                .when_some(label, |column, label| {
                    column.child(
                        div()
                            .text_size(zpx(TEXT_SM))
                            .text_color(crate::ui::secondary_text())
                            .child(label),
                    )
                }),
        )
        .children(copy)
}

/// A prompt's Copy: the reply's glyph and 1.5 s check, shown on hover or focus.
#[derive(IntoElement)]
struct PromptCopy {
    ix: usize,
    text: String,
    group: SharedString,
}

impl RenderOnce for PromptCopy {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let PromptCopy { ix, text, group } = self;
        // Counts clicks, so a repeated click restarts the confirmation; 0 is idle.
        let copies = window.use_keyed_state(
            ElementId::NamedInteger("prompt-copy".into(), ix as u64),
            cx,
            |_, _| 0u32,
        );
        let copied = *copies.read(cx) > 0;
        div()
            .id(("copy-prompt", ix))
            .keyboard_control()
            .absolute()
            .top(zpx(crate::ui::SPACE_XS))
            .right(zpx(crate::ui::SPACE_XS))
            .size(zpx(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(zpx(crate::ui::RADIUS_ROW))
            .cursor_pointer()
            .role(Role::Button)
            .aria_label(if copied { "Copied" } else { "Copy message" })
            .opacity(if copied { 1. } else { 0. })
            .group_hover(group, |button| button.opacity(1.))
            .focus_visible(|button| button.opacity(1.))
            .hover(|button| button.bg(rgba(CARD_HOVER)))
            .child(
                crate::micro_motion::MorphIcon::new(
                    ("prompt-copy-confirmation", ix),
                    crate::animated_icons::Morph::CopyCheck,
                    copied,
                )
                .with_size(zpx(13.))
                .text_color(crate::ui::secondary_text()),
            )
            // Pressing Copy must not start a text selection in the message.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                let click = copies.update(cx, |count, cx| {
                    *count = count.wrapping_add(1).max(1);
                    cx.notify();
                    *count
                });
                let copies = copies.clone();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    copies.update(cx, |count, cx| {
                        if *count == click {
                            *count = 0;
                            cx.notify();
                        }
                    });
                })
                .detach();
            })
    }
}

struct ResponseFooter {
    replied_at: Option<chrono::DateTime<chrono::Local>>,
    hovered: Option<bool>,
    copied: bool,
    completed: bool,
    reduced: bool,
}

fn assistant_message(
    ix: usize,
    text: String,
    state: Option<&Entity<text::TextViewState>>,
    chat: WeakEntity<ChatView>,
    footer: ResponseFooter,
) -> impl IntoElement {
    let ResponseFooter {
        replied_at,
        hovered,
        copied,
        completed,
        reduced,
    } = footer;
    let style = prose_style();
    let copy_chat = chat.clone();
    let hover_chat = chat.clone();
    let group = SharedString::from(format!("assistant-message-{ix}"));
    div()
        .id(("assistant-response", ix))
        .on_hover(move |hovered, _, cx| {
            // The footer's space is reserved, so the row's height never changes.
            let _ = hover_chat.update(cx, |this, cx| {
                this.response_hover.insert(ix, *hovered);
                cx.notify();
            });
        })
        .group(group.clone())
        .px(zpx(TEXT_INSET))
        .text_size(zpx(TEXT_PROSE))
        .line_height(relative(PROSE_LEADING))
        .text_color(rgb(TEXT))
        .child(
            state
                .map(TextView::new)
                .unwrap_or_else(|| {
                    TextView::markdown(("markdown", ix), super::citations::markdown(&text))
                })
                .style(style)
                .code_block_actions(code_block_actions)
                .stream_fade(false)
                .selectable(true)
                .on_link_click(move |url, _, _, cx| {
                    cx.stop_propagation();
                    let _ =
                        chat.update(cx, |_, cx| cx.emit(super::view::OpenLink(url.to_string())));
                }),
        )
        .when(completed, |message| {
            message.child(
                h_flex()
                    .h(zpx(26.))
                    .ml(-zpx(crate::ui::SPACE_SM))
                    .gap(zpx(crate::ui::SPACE_XS))
                    .items_center()
                    .justify_start()
                    .text_size(zpx(del_core::typography::TEXT_XS))
                    .text_color(crate::appearance::secondary_text())
                    .child(
                        div()
                            .id(("copy-response", ix))
                            .keyboard_control()
                            // Tab can land here without a hover, so focus must show it.
                            .focus_visible(|button| button.opacity(1.))
                            .size(zpx(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(zpx(crate::ui::RADIUS_ROW))
                            .cursor_pointer()
                            .role(Role::Button)
                            .aria_label(if copied { "Copied" } else { "Copy response" })
                            .hover(|button| button.bg(rgba(CARD_HOVER)))
                            .child(
                                crate::micro_motion::MorphIcon::new(
                                    ("copy-confirmation", ix),
                                    crate::animated_icons::Morph::CopyCheck,
                                    copied,
                                )
                                .with_size(zpx(13.))
                                .text_color(crate::appearance::secondary_text()),
                            )
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = copy_chat.update(cx, |chat, cx| chat.copy_response(ix, cx));
                            })
                            .with_animation(
                                SharedString::from(format!("response-copy-{ix}-{hovered:?}")),
                                Animation::new(std::time::Duration::from_millis(160))
                                    .with_easing(ease_out_quint()),
                                move |button, t| {
                                    button.opacity(footer_opacity(hovered, reduced, t))
                                },
                            ),
                    )
                    .when_some(replied_at, |footer, time| {
                        footer.child(
                            div()
                                .child(time.format("%H:%M").to_string())
                                .with_animation(
                                    SharedString::from(format!("response-time-{ix}-{hovered:?}")),
                                    Animation::new(std::time::Duration::from_millis(160))
                                        .with_easing(ease_out_quint()),
                                    move |time, t| {
                                        time.opacity(footer_opacity(hovered, reduced, t))
                                    },
                                ),
                        )
                    }),
            )
        })
}

/// The reply footer's 160 ms fade, in both directions.
fn footer_opacity(hovered: Option<bool>, reduced: bool, t: f32) -> f32 {
    match hovered {
        None => 0.,
        Some(true) if reduced => 1.,
        Some(false) if reduced => 0.,
        Some(true) => t,
        Some(false) => 1. - t,
    }
}

fn turn_end(status: TurnStatus) -> impl IntoElement {
    let label = match status {
        TurnStatus::Completed => "Completed",
        TurnStatus::Interrupted => "Stopped by you",
        TurnStatus::Failed => "Turn failed",
    };
    h_flex()
        .px(zpx(TEXT_INSET))
        .py(zpx(crate::ui::SPACE_XS))
        .text_size(zpx(TEXT_MD))
        .line_height(zpx(18.))
        .text_color(crate::ui::secondary_text())
        .child(label)
}

pub(super) type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A failure in the transcript: the danger mark, what failed, why, and a
/// Retry when there is something to retry.
fn failure_row(
    label: &'static str,
    error: Option<String>,
    retry: Option<ClickHandler>,
) -> impl IntoElement {
    let danger = ActionTone::Danger.foreground();
    h_flex()
        .px(zpx(TEXT_INSET))
        .py(zpx(crate::ui::SPACE_XS))
        .gap(zpx(crate::ui::SPACE_SM))
        .items_start()
        .text_size(zpx(TEXT_MD))
        .line_height(zpx(18.))
        .text_color(danger)
        .child(
            div().h(zpx(18.)).flex_none().flex().items_center().child(
                crate::icons::path("icons/error.svg")
                    .with_size(zpx(14.))
                    .text_color(danger),
            ),
        )
        .child(div().flex_none().child(label))
        .when_some(error, |row, error| {
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .whitespace_normal()
                    .text_color(rgba(BODY_MUTED))
                    .child(error),
            )
        })
        .when_some(retry, |row, retry| {
            row.child(
                Button::new("retry-failed-turn")
                    .ghost()
                    .small()
                    .flex_none()
                    .mt(-zpx(crate::ui::SPACE_2XS))
                    .bg(rgba(crate::appearance::QUIET_FILL))
                    .label("Retry")
                    .on_click(move |event, window, cx| retry(event, window, cx)),
            )
        })
}

pub type RetryFn = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Retries the opening prompt; the app installs it while that prompt failed.
pub struct FirstPromptRetry(pub RetryFn);
impl Global for FirstPromptRetry {}

/// The first line of a start failure, short enough for the status row.
fn short_reason(error: &str) -> String {
    let line = error
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut reason: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        reason.push('…');
    }
    reason.trim_end_matches('.').to_owned()
}

/// The opening message is rendered before there is a backend session to follow.
pub fn pending_prompt(input: del_agent::UserInput, error: Option<String>) -> impl IntoElement {
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
                        .pt(zpx(TRANSCRIPT_TOP))
                        .justify_center()
                        .px(zpx(crate::ui::SPACE_LG))
                        .child(
                            div()
                                .w_full()
                                .max_w(zpx(COLUMN_MAX_WIDTH))
                                .child(user_message(
                                    None,
                                    input.text.clone(),
                                    false,
                                    input
                                        .command
                                        .as_ref()
                                        .is_some_and(|c| c.kind == del_agent::CommandKind::Goal),
                                    AttachmentSummary::from_input(&input),
                                    true,
                                )),
                        ),
                ),
        )
        .child(
            v_flex()
                .w_full()
                .items_center()
                .px(zpx(crate::ui::SPACE_LG))
                .child(
                    // One fixed-height status row: starting, or why it could not.
                    h_flex()
                        .id("first-prompt-status")
                        .w_full()
                        .max_w(zpx(COLUMN_MAX_WIDTH))
                        .h(zpx(24.))
                        .px(zpx(TEXT_INSET))
                        .gap(zpx(crate::ui::SPACE_SM))
                        .items_center()
                        .text_size(zpx(TEXT_SM))
                        .text_color(crate::ui::secondary_text())
                        .when(!failed, |row| {
                            row.child(crate::icons::spinner()).child("Starting agent…")
                        })
                        .when_some(error, |row, error| {
                            let full = error.clone();
                            row.child(
                                div()
                                    .flex_none()
                                    .text_color(ActionTone::Danger.foreground())
                                    .child("Could not start"),
                            )
                            .child(div().flex_none().child("·"))
                            .child(
                                div()
                                    .id("first-prompt-reason")
                                    .min_w_0()
                                    .truncate()
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(full.clone())
                                            .build(window, cx)
                                    })
                                    .child(short_reason(&error)),
                            )
                            .child(div().flex_none().child("·"))
                            .child(
                                div()
                                    .id("retry-first-prompt")
                                    .keyboard_control()
                                    .flex_none()
                                    .role(Role::Button)
                                    .aria_label("Retry message")
                                    .cursor_pointer()
                                    .text_color(crate::ui::primary_text())
                                    .hover(|retry| retry.underline())
                                    .child("Retry")
                                    .on_click(|_, window, cx| {
                                        if let Some(retry) = cx.try_global::<FirstPromptRetry>() {
                                            let retry = retry.0.clone();
                                            retry(window, cx);
                                        }
                                    }),
                            )
                        }),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    struct ToolFixture {
        chat: Entity<ChatView>,
        tool: ToolCall,
        outer: ScrollHandle,
    }
    impl Render for ToolFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let card = self.chat.update(cx, |chat, cx| {
                chat.tool_card(0, &self.tool, cx).into_any_element()
            });
            v_flex()
                .id("outer-tools")
                .w(px(500.))
                .h(px(500.))
                .overflow_y_scroll()
                .track_scroll(&self.outer)
                .child(div().flex_none().child(card))
                .child(div().h(px(1000.)).flex_none())
        }
    }

    #[::gpui::test]
    fn full_tool_output_stays_bounded_and_contains_wheel_events(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let backend = crate::chat::Backend {
            runtime: runtime.handle().clone(),
            manager: del_agent::SessionManager::new(dir.path().join("index"), []).unwrap(),
        };
        let outer = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, cx| ToolFixture {
            chat: cx.new(|cx| ChatView::new(backend, None, None, cx)),
            tool: ToolCall {
                id: "long".into(),
                name: "shell".into(),
                input: "command".into(),
                output: "output line\n".repeat(900),
                state: super::super::model::ToolState::Ok,
                expanded: true,
            },
            outer: outer.clone(),
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let bounds = cx.debug_bounds("tool-output").unwrap();
        cx.simulate_mouse_move(bounds.center(), None, Modifiers::default());
        assert!(bounds.size.height > px(0.) && bounds.size.height <= zpx(260.));
        for (dx, dy) in [(0., -100.), (-80., -30.), (0., -100000.), (0., 100000.)] {
            cx.simulate_event(ScrollWheelEvent {
                position: bounds.center(),
                delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                ..Default::default()
            });
            cx.run_until_parked();
            assert_eq!(outer.offset(), point(px(0.), px(0.)));
        }
    }

    #[test]
    fn expanded_tool_retains_all_input_and_output_lines() {
        let input = (0..650).map(|n| format!("input {n}\n")).collect::<String>();
        let output = (0..900)
            .map(|n| format!("output {n}\n"))
            .collect::<String>();
        let details = ToolDetails::new(&ToolCall {
            id: "long-tool".into(),
            name: "shell".into(),
            input: input.clone(),
            output: output.clone(),
            state: super::super::model::ToolState::Ok,
            expanded: true,
        });
        assert!(details.markdown.contains(&input));
        assert!(details.markdown.contains(&output));
        assert!(details.markdown.contains("output 899"));
    }
}
