//! Rendering of one transcript row per `Block`.

use super::attachments::AttachmentSummary;
use super::model::{ApprovalBlock, ApprovalState, Block, DiffKind, FileChangeBlock, ToolCall};
use super::view::{COLUMN_MAX_WIDTH, ChatView};
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use del_agent::{ApprovalKind, ApprovalResponse, FileChangeKind, TurnStatus};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewStyle};
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
const MONO: &str = "Menlo";
/// Diff lines drawn in an expanded file card; the rest are summarized.
const MAX_DIFF_LINES: usize = 400;

impl ChatView {
    /// One list row: the block, centered in the transcript column.
    pub(super) fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        // Keep the streamed entity; don't copy the entire growing answer each frame.
        if !self
            .search_flash
            .as_ref()
            .is_some_and(|flash| flash.ix == ix)
            && matches!(self.model.blocks.get(ix), Some(Block::Assistant { .. }))
            && let Some(state) = self.assistant_text.get(&ix)
        {
            return h_flex()
                .w_full()
                .justify_center()
                .px_3()
                .pb(zpx(6.))
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
                                completed: self.model.is_completed_response(ix),
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
        if let Some(flash) = self.search_flash.as_mut().filter(|flash| flash.ix == ix) {
            if !flash.applied {
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
            let style = TextViewStyle::default().heading_font_size(|level, _| {
                if level <= 2 {
                    zpx(TEXT_LG)
                } else {
                    zpx(TEXT_MD)
                }
            });
            return h_flex()
                .w_full()
                .justify_center()
                .px_3()
                .child(
                    div()
                        .w_full()
                        .max_w(zpx(COLUMN_MAX_WIDTH))
                        .px_2()
                        .rounded_lg()
                        .when(matches!(block, Block::User { .. }), |row| {
                            row.py_2().bg(rgba(CARD))
                        })
                        .text_size(zpx(TEXT_MD))
                        .text_color(rgb(TEXT))
                        .child(
                            TextView::new(&flash.text_view)
                                .style(style)
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
                .h(zpx(28.))
                .px_2()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .rounded_md()
                .text_size(zpx(TEXT_MD))
                .text_color(rgba(MUTED))
                .hover(|row| row.bg(rgba(CARD)))
                .child(if live {
                    gpui_kit::component::shimmer::ShimmerText::new(label)
                        .id(("activity-live", ix))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .highlight_color(rgb(STRONG))
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(label)
                        .into_any_element()
                })
                .child(crate::icons::icon(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded_activity.remove(&ix) {
                        this.expanded_activity.insert(ix);
                    }
                    this.scroller.remeasure_items(range.clone());
                    cx.notify();
                    let range = range.clone();
                    cx.spawn(async move |this, cx| {
                        // The virtual list caches heights; remeasure throughout the reveal.
                        for _ in 0..30 {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(16))
                                .await;
                            if this
                                .update(cx, |this, cx| {
                                    this.scroller.remeasure_items(range.clone());
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                }));
            let mut children = v_flex().gap_1().pt_1();
            for at in group {
                if let Block::Tool(tool) = &self.model.blocks[at] {
                    children = children.child(self.tool_card(at, tool, cx));
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
                .px_3()
                .py_2()
                .child(div().w_full().max_w(zpx(COLUMN_MAX_WIDTH)).child(content))
                .into_any_element();
        }
        let compact = matches!(
            block,
            Block::Tool(_) | Block::FileChange(_) | Block::Reasoning { .. }
        );
        let content = match block {
            Block::Peer(message) => {
                let manager = &self.backend.manager;
                let current = manager.resolve_peer(self.session.as_ref().unwrap_or(&message.to));
                let from = manager.resolve_peer(&message.from);
                let target = manager.resolve_peer(if current == from {
                    &message.to
                } else {
                    &message.from
                });
                let name = |id: &del_agent::SessionId| {
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
                };
                let current_name = name(&current);
                let target_name = name(&target);
                let current_provider = manager.session_record(&current).ok().map(|r| r.provider);
                let target_provider = manager.session_record(&target).ok().map(|r| r.provider);
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
                    .py_2()
                    .text_size(zpx(TEXT_SM))
                    .text_color(rgba(MUTED))
                    .child(
                        h_flex()
                            .max_w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .id(SharedString::from(format!("peer-current-{}", message.id)))
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
                                    .gap_2()
                                    .items_center()
                                    .id(SharedString::from(format!("peer-other-{}", message.id)))
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
                .gap_2()
                .child(user_message(
                    text.clone(),
                    *steer,
                    *goal,
                    attachments.clone(),
                ))
                .when_some(self.working_label(ix), |column, label| {
                    column.child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgba(0xffffff66))
                            .child(
                                gpui_kit::component::shimmer::ShimmerText::new(label)
                                    .id(SharedString::from(format!("working-shimmer-{ix}")))
                                    .flex_none()
                                    .highlight_color(rgb(STRONG))
                                    .spread(0.22)
                                    .duration(std::time::Duration::from_millis(1200)),
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
                    completed: self.model.is_completed_response(ix),
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
            Block::Error { .. } => div().h_0().into_any_element(),
            Block::TurnEnd { status } => turn_end(*status).into_any_element(),
        };
        h_flex()
            .w_full()
            .justify_center()
            .px_3()
            .pb(if compact { zpx(2.) } else { zpx(6.) })
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
                            .into_any_element()
                    } else {
                        div().child("Thought").into_any_element()
                    }),
            )
            .when(expanded, |col| {
                col.child(
                    div()
                        .px_2()
                        .text_size(zpx(TEXT_SM))
                        .line_height(relative(1.5))
                        .text_color(rgba(MUTED))
                        .child(text.to_owned()),
                )
            })
    }

    fn tool_card(&self, ix: usize, tool: &ToolCall, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, label) = super::activity::describe(tool);
        let color = rgba(MUTED);
        let file = match self.model.blocks.get(ix + 1) {
            Some(Block::FileChange(file))
                if tool.expanded && super::activity::matches_file(tool, file) =>
            {
                Some(file)
            }
            _ => None,
        };
        // Only an expanded card shows the full input and output.
        let details = tool.expanded.then(|| {
            if tool.output.is_empty() {
                tool.input.clone()
            } else {
                format!("{}\n\n{}", tool.input, tool.output)
            }
        });
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .id(("tool", ix))
                    .h(zpx(26.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(zpx(TEXT_MD))
                    .text_color(color)
                    .hover(|row| row.bg(rgba(CARD)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        crate::icons::icon(if tool.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .with_size(zpx(12.))
                        .text_color(rgba(FAINT)),
                    ),
            )
            .when_some(details, |card, details| {
                card.child(
                    div()
                        .id(("tool-output", ix))
                        .px_2()
                        .py_3()
                        .max_h(zpx(260.))
                        .overflow_y_scroll()
                        .rounded_lg()
                        .bg(rgba(CARD))
                        .font_family(MONO)
                        .text_size(zpx(TEXT_SM))
                        .line_height(relative(1.5))
                        .text_color(rgba(MUTED))
                        .whitespace_normal()
                        .child(if details.is_empty() {
                            "No output".to_owned()
                        } else {
                            details
                        }),
                )
            })
            .when_some(file, |card, file| {
                card.child(self.file_card(ix + 1, file, true, cx))
            })
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
        v_flex()
            .child(
                h_flex()
                    .id(("file", ix))
                    .h(zpx(26.0))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .when(has_diff, |row| row.cursor_pointer())
                    .hover(|row| row.bg(rgba(CARD_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                    .child(crate::icons::path("icons/pencil.svg").text_color(rgba(MUTED)))
                    .child(
                        div()
                            .text_size(zpx(TEXT_MD))
                            .text_color(rgba(MUTED))
                            .child(kind_label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(zpx(TEXT_MD))
                            .text_color(rgba(MUTED))
                            .child(
                                change
                                    .path
                                    .file_name()
                                    .unwrap_or(change.path.as_os_str())
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                    )
                    .when(has_diff, |row| {
                        row.child(
                            crate::icons::icon(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .with_size(zpx(12.))
                            .text_color(rgba(FAINT)),
                        )
                    })
                    .when(change.added > 0 || change.removed > 0, |row| {
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
            .when(expanded && has_diff, |card| {
                let hidden = change.lines.len().saturating_sub(MAX_DIFF_LINES);
                card.child(
                    v_flex()
                        .id(("diff", ix))
                        .pb_1()
                        .max_h(zpx(220.0))
                        .overflow_y_scroll()
                        .children(change.lines.iter().take(MAX_DIFF_LINES).map(|line| {
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
                        }))
                        .when(hidden > 0, |diff| {
                            diff.child(
                                div()
                                    .px_2()
                                    .pt_1()
                                    .text_size(zpx(TEXT_SM))
                                    .text_color(rgba(FAINT))
                                    .child(format!(
                                        "{hidden} more {}",
                                        if hidden == 1 { "line" } else { "lines" }
                                    )),
                            )
                        }),
                )
            })
    }

    fn approval_card(&self, approval: &ApprovalBlock, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(approval.detail.clone()),
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
                let (label, color) = match approval.state {
                    ApprovalState::Approved => ("Approved", GREEN),
                    ApprovalState::Closed => ("No longer needed", MUTED),
                    _ => ("Denied", RED),
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

fn user_message(
    text: String,
    steer: bool,
    goal: bool,
    attachments: AttachmentSummary,
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
    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .p_2p5()
        .rounded_lg()
        .bg(rgba(0xffffff12))
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
                .flex_1()
                .min_w_0()
                .gap_1()
                .when(!text.is_empty(), |column| {
                    column.child(
                        div()
                            .text_size(zpx(TEXT_MD))
                            .line_height(relative(1.5))
                            .text_color(rgb(STRONG))
                            .child(text),
                    )
                })
                .when(sent_as_goal, |column| {
                    column.child(
                        div()
                            .text_size(zpx(11.))
                            .text_color(rgba(MUTED))
                            .child("◎  Sent as goal"),
                    )
                })
                .when_some(label, |column, label| {
                    column.child(
                        div()
                            .text_size(zpx(TEXT_SM))
                            .text_color(rgba(MUTED))
                            .child(label),
                    )
                }),
        )
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
    let style = TextViewStyle::default().heading_font_size(|level, _| {
        if level <= 2 {
            zpx(TEXT_LG)
        } else {
            zpx(TEXT_MD)
        }
    });
    let copy_chat = chat.clone();
    let hover_chat = chat.clone();
    let group = SharedString::from(format!("assistant-message-{ix}"));
    div()
        .id(("assistant-response", ix))
        .on_hover(move |hovered, _, cx| {
            let _ = hover_chat.update(cx, |this, cx| {
                this.response_hover.insert(ix, *hovered);
                this.scroller.remeasure_items(ix..ix + 1);
                cx.notify();
            });
        })
        .group(group.clone())
        .px_2()
        .text_size(zpx(TEXT_MD))
        .text_color(rgb(TEXT))
        .child(
            state
                .map(TextView::new)
                .unwrap_or_else(|| {
                    TextView::markdown(("markdown", ix), super::citations::markdown(&text))
                })
                .style(style)
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
                    .ml(-zpx(6.))
                    .gap(zpx(4.))
                    .items_center()
                    .justify_start()
                    .text_size(zpx(11.))
                    .text_color(rgba(MUTED))
                    .child(
                        div()
                            .id(("copy-response", ix))
                            .size(zpx(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .role(Role::Button)
                            .aria_label(if copied { "Copied" } else { "Copy response" })
                            .hover(|button| button.bg(rgba(CARD_HOVER)))
                            .child(if copied {
                                crate::icons::icon(IconName::Check)
                                    .with_size(zpx(13.))
                                    .text_color(rgba(MUTED))
                                    .into_any_element()
                            } else {
                                svg()
                                    .path("icons/copy-response.svg")
                                    .size(zpx(13.))
                                    .flex_none()
                                    .text_color(rgba(MUTED))
                                    .into_any_element()
                            })
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let _ = copy_chat.update(cx, |chat, cx| chat.copy_response(ix, cx));
                            }),
                    )
                    .when_some(replied_at, |footer, time| {
                        footer.child(time.format("%H:%M").to_string())
                    })
                    .with_animation(
                        SharedString::from(format!("response-footer-{ix}-{hovered:?}")),
                        Animation::new(std::time::Duration::from_millis(160))
                            .with_easing(ease_out_quint()),
                        move |footer, t| {
                            let opacity = match hovered {
                                None => 0.,
                                Some(true) if reduced => 1.,
                                Some(false) if reduced => 0.,
                                Some(true) => t,
                                Some(false) => 1. - t,
                            };
                            footer.opacity(opacity)
                        },
                    ),
            )
        })
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
                        .pt_2()
                        .child(div().w(zpx(24.)).flex_none())
                        .child(
                            h_flex().flex_1().min_w_0().justify_center().px_6().child(
                                div()
                                    .w_full()
                                    .max_w(zpx(COLUMN_MAX_WIDTH))
                                    .child(user_message(
                                        input.text.clone(),
                                        false,
                                        input.command.as_ref().is_some_and(|c| {
                                            c.kind == del_agent::CommandKind::Goal
                                        }),
                                        AttachmentSummary::from_input(&input),
                                    )),
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
                        row.child(crate::icons::spinner()).child("Starting agent…")
                    }),
            ),
        )
}
