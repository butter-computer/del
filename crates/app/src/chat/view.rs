//! `ChatView`: session plumbing and the message list. The composer is shared
//! with the new-chat page and lives in the app shell.

use super::model::{Block, Change, ChatModel};
use crate::appearance::rgba;
use crate::keyboard_control::KeyboardControl as _;
use crate::zoom::px as zpx;
use del_agent::{AgentError, AgentEvent, ApprovalResponse, SessionId, SessionManager, UserInput};
use del_core::typography::TEXT_SM;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::future::Future;
use tokio::sync::broadcast::error::RecvError;

/// Widest the transcript and composer grow, in logical pixels.
pub const COLUMN_MAX_WIDTH: f32 = 640.0;

/// What a chat needs to talk to agents. Cheap to clone.
#[derive(Clone)]
pub struct Backend {
    pub runtime: tokio::runtime::Handle,
    pub manager: SessionManager,
}

impl Backend {
    pub async fn resume(&self, id: &SessionId) -> del_agent::Result<del_agent::SessionHandle> {
        crate::providers::prepare_session_permissions(&self.manager, id)?;
        self.manager.resume(id).await
    }
}

/// Where an eased scroll lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JumpTarget {
    /// A row's top, `inset` pixels below the viewport's top edge.
    Row(usize),
    /// The end of the transcript; tail-following resumes on arrival.
    End,
}

struct MessageJump {
    target: JumpTarget,
    /// Space left above a row target so it does not sit flush against the edge.
    inset: f32,
    from: f32,
    started: std::time::Instant,
}

/// Duration of every eased transcript scroll: rail ticks, search hits, jump to latest.
const JUMP_SECONDS: f32 = 0.22;
/// Search hits land this far below the top so the match has context above it.
const SEARCH_INSET: f32 = 16.;
/// A disclosure's spring has settled well within this; remeasure until then.
const DISCLOSURE_SETTLE: std::time::Duration = std::time::Duration::from_millis(520);

/// Automatic delivery (after a turn, replay, or peer work) must not pull a
/// reader back down; only the person's own send scrolls to the new message.
fn scrolls_on_delivery(by_user: bool, following_tail: bool) -> bool {
    by_user || following_tail
}

/// The jump-to-latest pill shows only while the reader is above the end and
/// something new is there or still arriving.
fn shows_jump_to_latest(
    following_tail: bool,
    scrolled_to_end: Option<bool>,
    unseen: bool,
    running: bool,
) -> bool {
    !following_tail && scrolled_to_end == Some(false) && (unseen || running)
}

fn active_prompt(prompts: &[usize], top: usize, at_end: bool) -> Option<usize> {
    if at_end {
        prompts.last().copied()
    } else {
        prompts.iter().rev().find(|ix| **ix <= top).copied()
    }
}

pub(super) struct SearchFlash {
    pub ix: usize,
    pub text_view: Entity<gpui_kit::component::text::TextViewState>,
    pub range: std::ops::Range<usize>,
    pub applied: bool,
}

/// Combine only adjacent, already-received chunks for the same turn. Never wait
/// for a newline, timer, or a later event, and never cross tool/status boundaries.
fn coalesce_deltas(events: impl IntoIterator<Item = AgentEvent>) -> Vec<AgentEvent> {
    let mut ready: Vec<AgentEvent> = Vec::new();
    for event in events {
        let merged = match (&event, ready.last_mut()) {
            (
                AgentEvent::AssistantTextDelta { turn_id, text },
                Some(AgentEvent::AssistantTextDelta {
                    turn_id: previous,
                    text: accumulated,
                }),
            )
            | (
                AgentEvent::ReasoningDelta { turn_id, text },
                Some(AgentEvent::ReasoningDelta {
                    turn_id: previous,
                    text: accumulated,
                }),
            ) if previous == turn_id => {
                accumulated.push_str(text);
                true
            }
            _ => false,
        };
        if !merged {
            ready.push(event);
        }
    }
    ready
}

/// Whether the first history prompt is the row already shown as the opening
/// prompt. A kept opening row holds prose only; native history may append
/// attachment markers to the same prompt, so compare that prose as well.
fn replays_opening_row(opening: Option<&str>, history: &str) -> bool {
    opening.is_some_and(|opening| {
        opening == history
            || opening == super::attachments::AttachmentSummary::from_history(history).0
    })
}

/// Every non-overlapping match of `needle`, as (text index, byte offset).
/// Counting earlier occurrences and locating the hit must share this walk:
/// otherwise a repeated short message lands inside a longer one containing it.
fn substring_matches<'a>(
    texts: impl IntoIterator<Item = &'a str>,
    needle: &'a str,
) -> impl Iterator<Item = (usize, usize)> {
    texts.into_iter().enumerate().flat_map(move |(ix, text)| {
        text.match_indices(needle)
            .map(move |(offset, _)| (ix, offset))
    })
}

pub struct PromptSubmitted;
pub struct ReplyCompleted;
pub struct OpenLink(pub String);
pub struct OpenPeer(pub SessionId);
pub struct PeerActivity {
    pub outgoing: bool,
}

pub struct ChatView {
    pub(super) compact_user_messages: bool,
    pub usage: crate::session_usage::SessionUsage,
    pub(super) model: ChatModel,
    pub(super) scroller: ListState,
    jump: Option<MessageJump>,
    rail_hover: Option<usize>,
    /// Rows changed while the reader was scrolled up: the jump-to-latest pill shows.
    unseen: bool,
    /// Rows whose height animates (disclosure springs), remeasured every frame
    /// until the deadline so the virtual list follows the reveal.
    settling: Vec<(std::ops::Range<usize>, std::time::Instant)>,
    /// A final answer whose work is folding: keep its top where the reader saw it.
    answer_anchor: Option<(usize, Pixels)>,
    /// Peer receipt names and providers, resolved when the receipt arrives.
    pub(super) peer_rows: std::collections::HashMap<String, super::blocks::PeerRow>,
    /// When each live approval request arrived, for its entrance reveal.
    pub(super) approval_entered: std::collections::HashMap<String, std::time::Instant>,
    /// Expanded tool details, built once per change rather than every frame.
    pub(super) tool_details: std::cell::RefCell<
        std::collections::HashMap<usize, (usize, usize, super::blocks::ToolDetails)>,
    >,
    /// Rows holding user messages, in order: the message rail's ticks.
    prompt_rows: Vec<usize>,
    /// Cached `search_messages`; cleared whenever the transcript changes.
    search_messages:
        std::cell::RefCell<Option<std::sync::Arc<Vec<crate::conversation_search::Message>>>>,
    /// Consecutive tool rows shown as one group; refreshed when the model changes.
    pub(super) tool_groups: Vec<std::ops::Range<usize>>,
    /// History is folded without per-event list updates; one reset follows.
    replaying: bool,
    /// Optimistic user rows awaiting delivery, by queue item id. Row removal
    /// renumbers these, so a failed delivery never removes the wrong row.
    optimistic_rows: std::collections::HashMap<String, usize>,
    pub(super) backend: Backend,
    pub queue: crate::followups::Queue,
    pub(super) session: Option<SessionId>,
    /// The session's event stream ended (session closed or app quitting).
    closed: bool,
    loading: bool,
    first_prompt: Option<String>,
    completed_turns: u64,
    interrupt_requested: bool,
    working_since: Option<std::time::Instant>,
    working_message: Option<usize>,
    pub(super) copied_responses: std::collections::HashMap<usize, std::time::Instant>,
    pub(super) response_hover: std::collections::HashMap<usize, bool>,
    pub(super) response_times: std::collections::HashMap<usize, chrono::DateTime<chrono::Local>>,
    pub(super) expanded_activity: std::collections::HashSet<usize>,
    pub(super) work_sections: Vec<super::work_history::Section>,
    pub(super) expanded_work: std::collections::HashSet<usize>,
    pub(super) work_reveals: std::collections::HashMap<usize, super::work_history::Reveal>,
    work_started: std::collections::HashMap<usize, std::time::Instant>,
    pub(super) work_elapsed: std::collections::HashMap<usize, std::time::Duration>,
    pub(super) assistant_text: std::collections::HashMap<usize, Entity<text::TextViewState>>,
    pub pending_errors: Vec<String>,
    /// Messages sent while history loaded, delivered once it has.
    held_until_loaded: Vec<(String, bool)>,
    /// The last message delivered, so a failed turn can send it again.
    last_input: Option<UserInput>,
    /// The error row left by a failed opening load, which offers Retry.
    load_failed: Option<usize>,
    pub(super) search_pending: Option<crate::conversation_search::Hit>,
    pub(super) search_flash: Option<SearchFlash>,
}

impl EventEmitter<PromptSubmitted> for ChatView {}
impl EventEmitter<ReplyCompleted> for ChatView {}
impl EventEmitter<OpenLink> for ChatView {}
impl EventEmitter<OpenPeer> for ChatView {}
impl EventEmitter<PeerActivity> for ChatView {}

impl ChatView {
    pub fn with_compact_user_messages(mut self) -> Self {
        self.compact_user_messages = true;
        self
    }
    pub fn goal(&self) -> Option<del_agent::Goal> {
        self.model.goal()
    }

    /// A chat for `session`, which the app has already started. The view
    /// replays what the session emitted so far (so nothing sent before this
    /// view existed is missed), then follows it live. `first_prompt` is the
    /// input the app sent to start the session, shown as the opening user row.
    pub fn new(
        backend: Backend,
        session: Option<SessionId>,
        first_prompt: Option<UserInput>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroller = ListState::new(0, ListAlignment::Top, zpx(400.0));
        scroller.set_follow_mode(FollowMode::Tail);
        let view = cx.weak_entity();
        scroller.set_scroll_handler(move |_, _, cx| {
            let view = view.clone();
            cx.defer(move |cx| {
                // The reader's own scroll wins over any eased jump or anchor.
                let _ = view.update(cx, |this, cx| {
                    this.jump = None;
                    this.answer_anchor = None;
                    cx.notify();
                });
            });
        });
        let mut view = Self {
            compact_user_messages: false,
            queue: session
                .as_ref()
                .filter(|id| !backend.manager.is_temporary(id))
                .map(crate::followups::Queue::load)
                .unwrap_or_default(),
            model: ChatModel::default(),
            scroller,
            jump: None,
            rail_hover: None,
            unseen: false,
            settling: Vec::new(),
            answer_anchor: None,
            peer_rows: Default::default(),
            approval_entered: Default::default(),
            tool_details: Default::default(),
            prompt_rows: Vec::new(),
            search_messages: Default::default(),
            tool_groups: Vec::new(),
            replaying: false,
            usage: Default::default(),
            optimistic_rows: Default::default(),
            backend,
            session: session.clone(),
            closed: false,
            loading: session.is_some(),
            first_prompt: first_prompt.as_ref().map(|input| input.text.clone()),
            completed_turns: 0,
            interrupt_requested: false,
            working_since: None,
            working_message: None,
            response_times: Default::default(),
            response_hover: Default::default(),
            copied_responses: Default::default(),
            expanded_activity: Default::default(),
            work_sections: Default::default(),
            expanded_work: Default::default(),
            work_reveals: Default::default(),
            work_started: Default::default(),
            work_elapsed: Default::default(),
            assistant_text: Default::default(),
            pending_errors: Vec::new(),
            held_until_loaded: Vec::new(),
            last_input: None,
            load_failed: None,
            search_pending: None,
            search_flash: None,
        };
        if let Some(input) = first_prompt {
            let change = view.model.push_input(&input, false);
            view.sync(Some(change), cx);
        }
        if let Some(id) = session {
            view.follow(id, cx);
        }
        // Queue writes run in the background; let them land before the app exits.
        cx.on_app_quit(|_, _| {
            crate::sessions::flush_pending_writes();
            async {}
        })
        .detach();
        view
    }

    /// Shared with the search palette; rebuilt only after the transcript changed.
    pub fn search_messages(&self) -> std::sync::Arc<Vec<crate::conversation_search::Message>> {
        self.search_messages
            .borrow_mut()
            .get_or_insert_with(|| {
                std::sync::Arc::new(crate::conversation_search::messages(&self.model))
            })
            .clone()
    }

    #[cfg(test)]
    pub fn saved_transcript(backend: Backend, model: ChatModel, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(backend, None, None, cx);
        view.scroller.reset(model.blocks.len());
        view.model = model;
        view.refresh_prompt_rows();
        view
    }

    pub fn reveal_search_hit(
        &mut self,
        hit: crate::conversation_search::Hit,
        cx: &mut Context<Self>,
    ) {
        self.search_pending = Some(hit);
        self.search_flash = None;
        self.jump = None;
        self.apply_search_jump(cx);
        cx.notify();
    }

    fn apply_search_jump(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(hit) = self.search_pending.take() else {
            return;
        };
        self.refresh_work_sections();
        let Some(message_ix) = hit.message else {
            return;
        };
        let message = &hit.document.messages[message_ix];
        let occurrence = substring_matches(
            hit.document.messages[..message_ix]
                .iter()
                .filter(|m| m.role == message.role)
                .map(|m| m.text.as_str()),
            &message.text,
        )
        .count();
        let rows: Vec<(usize, &String)> = self
            .model
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(ix, block)| match block {
                Block::User { text, .. } if message.role == "You" => Some((ix, text)),
                Block::Assistant { text, .. } if message.role == "Assistant" => Some((ix, text)),
                _ => None,
            })
            .collect();
        let found = substring_matches(rows.iter().map(|(_, text)| text.as_str()), &message.text)
            .nth(occurrence)
            .map(|(row, offset)| (rows[row].0, offset, rows[row].1.clone()));
        let Some((ix, offset, text)) = found else {
            self.pending_errors.push(
                "That message changed since the search. Search again to find its current location."
                    .into(),
            );
            return;
        };
        let source_range = offset + hit.range.start..offset + hit.range.end;
        let (source, range) = if message.role == "You" {
            super::blocks::literal_markdown(&text, source_range)
        } else {
            super::citations::with_range(&text, source_range)
        };
        let text_view =
            cx.new(|cx| gpui_kit::component::text::TextViewState::markdown(&source, cx));
        cx.observe(&text_view, |_, _, cx| cx.notify()).detach();
        self.search_flash = Some(SearchFlash {
            ix,
            text_view,
            range,
            applied: false,
        });
        self.reveal_work_row(ix);
        // The same eased scroll as a rail jump, landing just below the top edge.
        self.jump = Some(MessageJump {
            target: JumpTarget::Row(ix),
            inset: SEARCH_INSET,
            from: self.scroller.scroll_px_offset_for_scrollbar().y.as_f32(),
            started: cx.background_executor().now(),
        });
        cx.notify();
    }

    pub(super) fn copy_response(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.refresh_work_sections();
        if !self.final_response(ix) {
            return;
        }
        let Some(Block::Assistant { text, .. }) = self.model.blocks.get(ix) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        let copied_at = std::time::Instant::now();
        self.copied_responses.insert(ix, copied_at);
        self.scroller.remeasure_items(ix..ix + 1);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                // A second click restarts the confirmation instead of being cleared by this timer.
                // Rows may have shifted since the click; find the entry by its stamp.
                if let Some(ix) = this
                    .copied_responses
                    .iter()
                    .find_map(|(ix, at)| (*at == copied_at).then_some(*ix))
                {
                    this.copied_responses.remove(&ix);
                    this.scroller.remeasure_items(ix..ix + 1);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_event(&mut self, event: &AgentEvent, cx: &mut Context<Self>) {
        let previous_cost = match event {
            AgentEvent::UsageSnapshot {
                cost_scope: Some(scope),
                ..
            } => self.usage.costs.get(scope).copied(),
            _ => None,
        };
        self.usage.apply(event);
        if let AgentEvent::UsageSnapshot {
            cost_scope: Some(scope),
            cost_usd: Some(cost),
            ..
        } = event
            && previous_cost != self.usage.costs.get(scope).copied()
            && let Some(id) = self.session.clone()
        {
            let (scope, cost) = (scope.clone(), *cost);
            self.backend.runtime.spawn_blocking(move || {
                if let Err(error) = crate::session_usage::save_cost(&id, &scope, cost) {
                    eprintln!("Could not save session usage: {error}");
                }
            });
        }

        if !self.loading && matches!(event, AgentEvent::TurnStarted { .. }) {
            let start = self
                .model
                .blocks
                .iter()
                .rposition(|b| {
                    matches!(
                        b,
                        Block::User {
                            steer: false,
                            goal: false,
                            ..
                        }
                    )
                })
                .map_or(0, |ix| ix + 1);
            self.work_started
                .entry(start)
                .or_insert_with(std::time::Instant::now);
        }

        // Stop can arrive while resume/send is still preparing the provider turn.
        if !self.loading
            && self.interrupt_requested
            && matches!(event, AgentEvent::TurnStarted { .. })
        {
            self.stop(cx);
        }
        if let AgentEvent::TurnCompleted { turn_id, .. } = event {
            self.completed_turns += 1;
            if !self.loading
                && let Some(ix) = self.model.blocks.iter().rposition(
                    |block| matches!(block, Block::Assistant { turn_id: id, .. } if id == turn_id),
                )
            {
                let time = chrono::Local::now();
                self.response_times.insert(ix, time);
                let start = super::work_history::request_start(&self.model, ix);
                // A chat opened with its session is still loading when the first turn
                // starts, so that turn is timed only by the live "Working for" clock.
                let elapsed = self
                    .work_started
                    .get(&start)
                    .or(self.working_since.as_ref())
                    .map(|at| at.elapsed());
                if let Some(elapsed) = elapsed {
                    self.work_elapsed.insert(ix, elapsed);
                }
                if let Some(id) = &self.session
                    && let Some(entry) = super::reply_times::entry(&self.model, ix, time)
                        .map(|e| e.with_elapsed(elapsed))
                {
                    let id = id.0.clone();
                    let write = cx
                        .background_executor()
                        .spawn(async move { super::reply_times::append(&id, &entry) });
                    // Housekeeping: a lost timestamp is not worth interrupting anyone.
                    cx.spawn(async move |_, _| {
                        if let Err(error) = write.await {
                            eprintln!("Could not save reply time: {error}");
                        }
                    })
                    .detach();
                }
                self.scroller.remeasure_items(ix..ix + 1);
            }
        }
        // A recoverable error has no row of its own, so it becomes a toast. A
        // fatal one fails the turn, whose inline row already says why.
        if !self.loading
            && let AgentEvent::Error {
                message,
                recoverable: true,
            } = event
        {
            self.pending_errors.push(message.clone());
        }
        if !self.loading
            && let AgentEvent::TurnCompleted {
                status: del_agent::TurnStatus::Failed,
                ..
            } = event
            && let Some(shown) = super::blocks::failed_turn_error(&self.model.blocks)
        {
            // Not shown yet: the failed row carries it instead of a toast.
            self.pending_errors.retain(|message| *message != shown);
        }
        let change = self.model.apply(event);
        if !self.loading
            && matches!(change, Some(Change::Pushed(_)))
            && let AgentEvent::PeerMessage { message } = event
        {
            cx.emit(PeerActivity {
                outgoing: self.session.as_ref() == Some(&message.from),
            });
        }
        let delta = match event {
            AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        };
        self.sync_text(change, delta, cx);
        if matches!(event, AgentEvent::PeerWorkChanged { .. }) && !self.loading {
            self.send_next(cx);
        }

        if !self.loading
            && matches!(
                event,
                AgentEvent::TurnCompleted {
                    status: del_agent::TurnStatus::Completed,
                    ..
                }
            )
        {
            cx.emit(ReplyCompleted);
        }
    }

    pub(super) fn final_response(&self, ix: usize) -> bool {
        self.work_sections
            .iter()
            .any(|section| section.answer == ix)
    }

    /// Recompute what depends on the whole transcript: tool groups and folded
    /// work. Runs when the model changes, never per frame.
    fn refresh_layout(&mut self, cx: &mut Context<Self>) {
        self.tool_groups = super::activity::tool_groups(&self.model.blocks);
        let before: std::collections::HashSet<usize> =
            self.work_sections.iter().map(|s| s.start).collect();
        self.refresh_work_sections();
        if self.loading || self.replaying || cx.reduce_motion() {
            return;
        }
        // A final answer just folded its work under Worked: close it with the
        // same reveal as a click, keeping the answer where the reader sees it.
        let now = std::time::Instant::now();
        let folded: Vec<_> = self
            .work_sections
            .iter()
            .filter(|section| {
                !before.contains(&section.start)
                    && section.end - section.start > 1
                    && !self.expanded_work.contains(&section.start)
            })
            .cloned()
            .collect();
        for section in folded {
            self.work_reveals.insert(
                section.start,
                super::work_history::Reveal::new(1., false, now),
            );
            if !self.scroller.is_following_tail()
                && let Some(bounds) = self.scroller.bounds_for_item(section.answer)
            {
                let top = bounds.top() - self.scroller.viewport_bounds().top();
                if top >= zpx(0.) {
                    self.answer_anchor = Some((section.answer, top));
                }
            }
        }
    }

    fn refresh_work_sections(&mut self) {
        let pending = self
            .session
            .as_ref()
            .and_then(|id| self.backend.manager.peer_work(id))
            .is_some_and(|w| w.status != del_agent::peers::WorkStatus::Complete)
            || self
                .model
                .goal()
                .is_some_and(|g| g.status == del_agent::GoalStatus::Active);
        let next = super::work_history::sections(&self.model, pending);
        if next != self.work_sections {
            self.work_sections = next;
            self.scroller.remeasure_items(0..self.model.blocks.len());
        }
    }

    fn reveal_work_row(&mut self, ix: usize) {
        if let Some(section) = self
            .work_sections
            .iter()
            .find(|r| r.range().contains(&ix) && ix != r.answer)
            && self.expanded_work.insert(section.start)
        {
            self.work_reveals.remove(&section.start);
            self.scroller.remeasure_items(section.range());
        }
    }

    pub(super) fn toggle_work(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.scroller.pause_following_tail();
        let now = std::time::Instant::now();
        let was_open = self.expanded_work.contains(&range.start);
        let from = self
            .work_reveals
            .get(&range.start)
            .map_or(if was_open { 1. } else { 0. }, |motion| {
                motion.sample(now).0
            });
        if was_open {
            self.expanded_work.remove(&range.start);
        } else {
            self.expanded_work.insert(range.start);
        }
        if cx.reduce_motion() {
            self.work_reveals.remove(&range.start);
        } else {
            // Render samples the reveal and remeasures each frame until it settles.
            self.work_reveals.insert(
                range.start,
                super::work_history::Reveal::new(from, !was_open, now),
            );
        }
        self.scroller.remeasure_items(range);
        cx.notify();
    }

    pub(super) fn working_label(&self, ix: usize) -> Option<String> {
        if self.working_message != Some(ix) {
            return None;
        }
        let seconds = self.working_since?.elapsed().as_secs();
        Some(format!(
            "Working for {}",
            super::work_history::elapsed_label(seconds)
        ))
    }

    /// Keep the virtual list's rows in step with the model.
    fn sync(&mut self, change: Option<Change>, cx: &mut Context<Self>) {
        self.sync_text(change, None, cx);
    }

    fn sync_text(&mut self, change: Option<Change>, delta: Option<&str>, cx: &mut Context<Self>) {
        self.search_messages.get_mut().take();
        if self.model.is_running() && self.working_since.is_none() && !self.loading {
            let started = std::time::Instant::now();
            self.working_since = Some(started);
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    if !this
                        .update(cx, |this, cx| {
                            let active =
                                this.working_since == Some(started) && this.model.is_running();
                            if active {
                                if let Some(ix) = this.working_message {
                                    this.scroller.remeasure_items(ix..ix + 1);
                                }
                                cx.notify();
                            }
                            active
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            })
            .detach();
        } else if !self.model.is_running() {
            self.working_since = None;
        }
        let previous_working_message = self.working_message;
        self.working_message = self.working_since.and_then(|_| {
            self.model
                .blocks
                .iter()
                .rposition(|block| matches!(block, Block::User { .. }))
        });
        if let Some(change) = change {
            let ix = match change {
                Change::Pushed(ix) | Change::Updated(ix) => ix,
            };
            if let Some(Block::Peer(message)) = self.model.blocks.get(ix) {
                let row = super::blocks::PeerRow::resolve(
                    &self.backend.manager,
                    self.session.as_ref(),
                    message,
                );
                self.peer_rows.insert(message.id.clone(), row);
            }
            self.tool_details.get_mut().remove(&ix);
            if let (Change::Pushed(_), Some(Block::Approval(approval))) =
                (change, self.model.blocks.get(ix))
                && !self.loading
                && !self.replaying
            {
                self.approval_entered
                    .insert(approval.id.clone(), std::time::Instant::now());
                self.settle(ix..ix + 1, cx);
            }
            if let Some(Block::Assistant { text, .. }) = self.model.blocks.get(ix) {
                if let Some(view) = self.assistant_text.get(&ix) {
                    view.update(cx, |view, cx| {
                        if let Some(delta) =
                            delta.filter(|_| !text.contains(super::citations::MARKER))
                        {
                            view.push_str(delta, cx);
                        } else {
                            view.set_text(&super::citations::markdown(text), cx);
                        }
                    });
                } else if !self.replaying {
                    // Replayed history parses lazily, when a row is first drawn.
                    self.ensure_assistant_text(ix, cx);
                }
            }
            if self.replaying {
                // One `scroller.reset` and notify follow the whole replay.
                return;
            }
            match change {
                Change::Pushed(_) => {
                    if self.navigation_text(ix).is_some() {
                        self.prompt_rows.push(ix);
                    }
                    let count = self.scroller.item_count();
                    self.scroller.splice(count..count, 1);
                }
                Change::Updated(ix) => {
                    if matches!(
                        self.model.blocks.get(ix),
                        Some(Block::User { .. } | Block::Peer(_))
                    ) {
                        self.refresh_prompt_rows();
                    }
                    self.scroller.remeasure_items(ix..ix + 1);
                }
            }
            let mut start = ix;
            while start > 0 && matches!(self.model.blocks[start - 1], Block::Tool(_)) {
                start -= 1;
            }
            let mut end = ix + 1;
            while end < self.model.blocks.len() && matches!(self.model.blocks[end], Block::Tool(_))
            {
                end += 1;
            }
            self.scroller.remeasure_items(start..end);
        } else if self.replaying {
            return;
        } else if !self.model.is_running() {
            // A successful final tool sequence can collapse when the turn settles.
            let end = self.model.blocks.len();
            let mut start = end;
            while start > 0 && matches!(self.model.blocks[start - 1], Block::Tool(_)) {
                start -= 1;
            }
            if start < end {
                self.scroller.remeasure_items(start..end);
            }
        }
        for ix in [previous_working_message, self.working_message]
            .into_iter()
            .flatten()
        {
            if ix < self.scroller.item_count() {
                self.scroller.remeasure_items(ix..ix + 1);
            }
        }
        if change.is_some() && !self.loading && !self.scroller.is_following_tail() {
            self.unseen = true;
        }
        self.refresh_layout(cx);
        cx.notify();
    }

    /// The reply's Markdown view, created on first use. Opening a long
    /// conversation parses only the replies that are actually drawn.
    pub(super) fn ensure_assistant_text(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<text::TextViewState>> {
        if let Some(view) = self.assistant_text.get(&ix) {
            return Some(view.clone());
        }
        let Some(Block::Assistant { text, .. }) = self.model.blocks.get(ix) else {
            return None;
        };
        let markdown = super::citations::markdown(text);
        let view = cx.new(|cx| text::TextViewState::markdown(&markdown, cx));
        // Look the row up on each notification: rows shift when one is removed.
        cx.observe(&view, |this, view, cx| {
            if let Some(ix) = this
                .assistant_text
                .iter()
                .find_map(|(ix, text)| (*text == view).then_some(*ix))
            {
                this.scroller.remeasure_items(ix..ix + 1);
            }
            cx.notify();
        })
        .detach();
        self.assistant_text.insert(ix, view.clone());
        Some(view)
    }

    pub fn remeasure_for_zoom(&mut self, cx: &mut Context<Self>) {
        self.jump = None;
        self.scroller.remeasure_items(0..self.scroller.item_count());
        cx.notify();
    }

    /// Whether a turn is in flight.
    pub fn is_running(&self) -> bool {
        self.model.is_running()
    }

    pub fn has_active_work(&self) -> bool {
        self.is_running()
            || self
                .session
                .as_ref()
                .and_then(|id| self.backend.manager.peer_work(id))
                .is_some_and(|w| {
                    matches!(
                        w.status,
                        del_agent::peers::WorkStatus::Active
                            | del_agent::peers::WorkStatus::Waiting
                    )
                })
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Persist the queue on the background writer: Enter, edit, delete and
    /// reorder paint first, and the coalesced write keeps the last snapshot.
    pub fn save_queue(&self) {
        self.queue.save_later();
    }

    pub fn enqueue(&mut self, input: UserInput, cx: &mut Context<Self>) -> String {
        cx.emit(PromptSubmitted);
        let id = self.queue.push(input);
        self.save_queue();
        cx.notify();
        id
    }

    pub fn take_queued(&mut self, id: &str, cx: &mut Context<Self>) -> Option<UserInput> {
        let item = self.queue.take(id)?;
        self.save_queue();
        cx.notify();
        Some(item.input)
    }

    pub fn reorder_queued(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        self.queue.reorder(from, to);
        self.save_queue();
        cx.notify();
    }

    pub fn submit_input(&mut self, input: UserInput, steer: bool, cx: &mut Context<Self>) {
        // A fresh human message can amend idle peer work. Automatic queue
        // draining still waits for that work to complete; older follow-ups stay.
        let steer = steer
            || (!self.is_running()
                && self
                    .session
                    .as_ref()
                    .is_some_and(|id| self.backend.manager.peer_blocks_user_queue(id)));
        let id = self.enqueue(input, cx);
        if self.loading {
            // History is still loading: hold the message (it shows in the
            // queue) and send it as soon as the transcript is ready.
            self.held_until_loaded.push((id, steer));
        } else if !self.is_running() || steer {
            self.deliver_queued(id, steer, cx);
        }
    }

    fn send_next(&mut self, cx: &mut Context<Self>) {
        if !self.interrupt_requested
            && !self.is_running()
            && matches!(
                self.model.last_status,
                None | Some(del_agent::TurnStatus::Completed)
            )
            && let Some(id) = self.queue.next()
        {
            self.deliver(id, false, false, cx);
        }
    }

    /// Deliver a queued item the person chose to send now (Enter, Steer).
    pub fn deliver_queued(&mut self, item_id: String, steer: bool, cx: &mut Context<Self>) {
        self.deliver(item_id, steer, true, cx);
    }

    fn deliver(&mut self, item_id: String, steer: bool, by_user: bool, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if !steer && self.backend.manager.peer_blocks_user_queue(&session) {
            return;
        }
        let Some(input) = self.queue.begin(&item_id) else {
            return;
        };
        self.interrupt_requested = false;
        cx.emit(PromptSubmitted);
        self.save_queue();
        let backend = self.backend.clone();
        let displayed_input = input.clone();
        let queued_id = item_id.clone();
        let delivery = self.backend.runtime.spawn(async move {
            backend.resume(&session).await?;
            if steer {
                backend.manager.steer_input(&session, input).await
            } else {
                backend.manager.send_prompt(&session, input).await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = delivery
                .await
                .map_err(|e| AgentError::Backend(e.to_string()))
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                let busy = matches!(&result, Err(AgentError::Busy(_)));
                if busy && !steer {
                    this.queue.wait(&item_id);
                } else {
                    if let Err(error) = &result {
                        this.pending_errors.push(format!("Could not send your message: {error}. It remains in the queue; try Steer again or edit it."));
                    }
                    this.queue
                        .finish(&item_id, result.as_ref().err().map(ToString::to_string));
                }
                this.save_queue();
                if result.is_ok() {
                    this.optimistic_rows.remove(&item_id);
                    // Backend events may already have arrived. Add the user row
                    // before dispatch below, rather than changing phase on ack.
                    if !this.model.is_running()
                        && this.model.last_status == Some(del_agent::TurnStatus::Completed)
                    {
                        this.send_next(cx);
                    }
                } else {
                    if let Some(row) = this.optimistic_rows.remove(&item_id)
                        && matches!(this.model.blocks.get(row), Some(Block::User { .. }))
                    {
                        this.remove_row(row);
                    }
                    this.model.reject_input();
                    if busy && !steer {
                        // Completion can precede provider readiness. A Busy rejection
                        // is undelivered; retry after yielding, without inventing a
                        // running turn that would wait forever for another completion.
                        let ready = this.backend.runtime.spawn(async {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        });
                        cx.spawn(async move |this, cx| {
                            let _ = ready.await;
                            let _ = this.update(cx, |this, cx| this.send_next(cx));
                        }).detach();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        self.last_input = Some(displayed_input.clone());
        let following = self.scroller.is_following_tail();
        let change = self.model.push_input(&displayed_input, steer);
        if let Change::Pushed(row) = change {
            self.optimistic_rows.insert(queued_id, row);
        }
        self.sync(Some(change), cx);
        if scrolls_on_delivery(by_user, following) {
            self.scroller.scroll_to_end();
            self.unseen = false;
        }
    }

    /// Send the failed turn's message again, exactly as it was delivered.
    pub(super) fn retry_last(&mut self, cx: &mut Context<Self>) {
        if self.is_running() {
            return;
        }
        if let Some(input) = self.last_input.clone() {
            self.submit_input(input, false, cx);
        }
    }

    /// The opening load failed; reattach and replay from the start.
    pub(super) fn retry_load(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.session.clone() else {
            return;
        };
        if let Some(row) = self.load_failed.take()
            && matches!(self.model.blocks.get(row), Some(Block::Error { .. }))
        {
            self.remove_row(row);
        }
        self.loading = true;
        self.follow(id, cx);
        cx.notify();
    }

    /// Whether `ix` is the opening load's failure row.
    pub(super) fn is_load_failure(&self, ix: usize) -> bool {
        self.load_failed == Some(ix)
    }

    /// Whether the failed turn ending at row `ix` can be sent again.
    pub(super) fn can_retry_turn(&self, ix: usize) -> bool {
        self.last_input.is_some() && !self.is_running() && ix + 1 == self.model.blocks.len()
    }

    /// Remove one transcript row, moving every row-keyed piece of view state
    /// after it up by one so it stays attached to its row.
    fn remove_row(&mut self, ix: usize) {
        use super::model::shifted_after_removal as shift;
        fn remap<V>(map: &mut std::collections::HashMap<usize, V>, removed: usize) {
            *map = std::mem::take(map)
                .into_iter()
                .filter_map(|(key, value)| Some((shift(key, removed)?, value)))
                .collect();
        }
        self.model.remove_block(ix);
        self.scroller.splice(ix..ix + 1, 0);
        remap(&mut self.assistant_text, ix);
        remap(&mut self.response_times, ix);
        remap(&mut self.copied_responses, ix);
        remap(&mut self.response_hover, ix);
        self.work_reveals.clear();
        remap(&mut self.work_started, ix);
        remap(&mut self.work_elapsed, ix);
        self.work_sections.clear();
        self.expanded_work = std::mem::take(&mut self.expanded_work)
            .into_iter()
            .filter_map(|key| shift(key, ix))
            .collect();
        self.expanded_activity = std::mem::take(&mut self.expanded_activity)
            .into_iter()
            .filter_map(|key| shift(key, ix))
            .collect();
        self.working_message = self.working_message.and_then(|key| shift(key, ix));
        self.optimistic_rows.retain(|_, row| *row != ix);
        for row in self.optimistic_rows.values_mut() {
            if *row > ix {
                *row -= 1;
            }
        }
        self.search_flash = self.search_flash.take().and_then(|mut flash| {
            flash.ix = shift(flash.ix, ix)?;
            Some(flash)
        });
        self.jump = self.jump.take().and_then(|mut jump| {
            if let JumpTarget::Row(target) = jump.target {
                jump.target = JumpTarget::Row(shift(target, ix)?);
            }
            Some(jump)
        });
        self.load_failed = self.load_failed.and_then(|row| shift(row, ix));
        self.settling.clear();
        self.answer_anchor = None;
        self.tool_details.get_mut().clear();
        self.tool_groups = super::activity::tool_groups(&self.model.blocks);
        self.refresh_prompt_rows();
    }

    fn navigation_text(&self, ix: usize) -> Option<&str> {
        match self.model.blocks.get(ix) {
            Some(Block::User { text, .. }) if !text.trim().is_empty() => Some(text),
            Some(Block::User { attachments, .. }) if attachments.label().is_some() => {
                Some("Attachments")
            }
            Some(Block::Peer(message)) if !message.text.trim().is_empty() => Some(&message.text),
            _ => None,
        }
    }

    fn refresh_prompt_rows(&mut self) {
        self.search_messages.get_mut().take();
        self.prompt_rows = self
            .model
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(ix, _)| self.navigation_text(ix).map(|_| ix))
            .collect();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.interrupt_requested = true;
        cx.notify();
        let Some(id) = self.session.clone() else {
            return;
        };
        let manager = self.backend.manager.clone();
        self.run(cx, async move { manager.interrupt(&id).await });
    }

    pub(super) fn respond(
        &mut self,
        id: String,
        response: ApprovalResponse,
        cx: &mut Context<Self>,
    ) {
        // Only a still-pending card is answered; the provider may have
        // settled it already.
        let Some(change) = self.model.resolve_approval(&id, response) else {
            return;
        };
        self.sync(Some(change), cx);
        let Some(session) = self.session.clone() else {
            return;
        };
        let manager = self.backend.manager.clone();
        self.run(cx, async move {
            manager.respond_approval(&session, &id, response).await
        });
    }

    /// Expand or collapse one row. The row stays under the pointer: growing
    /// content never drags the list to the end while tail-following.
    pub(super) fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.scroller.pause_following_tail();
        let change = self.model.toggle(ix);
        self.settle(ix..ix + 2, cx);
        self.sync(change, cx);
    }

    /// An eased scroll is still moving the list.
    pub(super) fn jump_in_flight(&self) -> bool {
        self.jump.is_some()
    }

    /// Whether row `ix` is still moving, so a closing body stays mounted.
    pub(super) fn is_settling(&self, ix: usize) -> bool {
        self.settling.iter().any(|(range, _)| range.contains(&ix))
    }

    /// Remeasure `range` on every frame while a disclosure spring moves it.
    pub(super) fn settle(&mut self, range: std::ops::Range<usize>, cx: &App) {
        let end = range.end.min(self.model.blocks.len());
        let range = range.start.min(end)..end;
        self.scroller.remeasure_items(range.clone());
        if !cx.reduce_motion() {
            self.settling
                .push((range, std::time::Instant::now() + DISCLOSURE_SETTLE));
        }
    }

    /// Reattach the session if it is not live, replay its history into the
    /// model, then pump new events until the stream closes.
    fn follow(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.follow_from(id, false, cx);
    }

    /// Clear the transcript before replaying history. Used after the live
    /// stream lagged and dropped events, which may include turn completion.
    fn reset_for_resync(&mut self) {
        // Keep an opening row's attachment metadata; replay skips its plain
        // history counterpart, exactly as on first load.
        let opening = self
            .model
            .blocks
            .first()
            .filter(|block| matches!(block, Block::User { .. }))
            .cloned();
        if let Some(Block::User { text, .. }) = &opening {
            self.first_prompt = Some(text.clone());
        }
        self.usage.context_tokens = None;
        self.usage.context_window = None;
        self.model = ChatModel::default();
        self.model.blocks.extend(opening);
        self.scroller.reset(self.model.blocks.len());
        self.assistant_text.clear();
        self.response_times.clear();
        self.copied_responses.clear();
        self.response_hover.clear();
        self.expanded_activity.clear();
        self.work_sections.clear();
        self.expanded_work.clear();
        self.work_reveals.clear();
        self.work_started.clear();
        self.work_elapsed.clear();
        self.optimistic_rows.clear();
        self.search_flash = None;
        self.jump = None;
        self.settling.clear();
        self.answer_anchor = None;
        self.peer_rows.clear();
        self.tool_details.get_mut().clear();
        self.load_failed = None;
        self.working_since = None;
        self.working_message = None;
        self.refresh_prompt_rows();
    }

    fn follow_from(&mut self, id: SessionId, resync: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let attached = self.backend.runtime.spawn(async move {
            backend.resume(&id).await?;
            let attached = backend.manager.subscribe_with_history(&id)?;
            let record = backend
                .manager
                .list_sessions()
                .into_iter()
                .find(|record| record.id == id);
            let (times, costs) = tokio::task::spawn_blocking(move || {
                let costs = crate::session_usage::load_costs(&id);
                let times = record
                    .as_ref()
                    .map(super::reply_times::load)
                    .unwrap_or_default();
                (times, costs)
            })
            .await
            .unwrap_or_default();
            Ok::<_, AgentError>((attached, times, costs))
        });
        cx.spawn(async move |this, cx| {
            let ((history, mut rx), times, costs) = match attached.await {
                Ok(Ok(attached)) => attached,
                Ok(Err(err)) => return report_load(&this, cx, err.to_string()),
                Err(err) => return report_load(&this, cx, err.to_string()),
            };
            let replayed = this.update(cx, |this, cx| {
                let scroll = (resync && !this.scroller.is_following_tail())
                    .then(|| this.scroller.logical_scroll_top());
                if resync {
                    this.loading = true;
                    this.reset_for_resync();
                }
                for (scope, cost) in costs {
                    this.usage.apply(&AgentEvent::UsageSnapshot {
                        context_tokens: None,
                        context_window: None,
                        cost_scope: Some(scope),
                        cost_usd: Some(cost),
                    });
                }
                // The initial row already includes attachment/goal metadata.
                // Skip its plain-text history counterpart exactly once.
                let first_prompt = this.first_prompt.take();
                let mut opening_user = true;
                // Fold history with list updates suppressed; reset the list once.
                this.replaying = true;
                for event in coalesce_deltas(history) {
                    if let AgentEvent::UserMessage { text } = &event
                        && opening_user
                    {
                        opening_user = false;
                        if replays_opening_row(first_prompt.as_deref(), text) {
                            continue;
                        }
                    }
                    this.on_event(&event, cx);
                }
                this.replaying = false;
                this.scroller.reset(this.model.blocks.len());
                this.refresh_prompt_rows();
                let timing = times.restore_details(&this.model);
                this.response_times.extend(timing.times);
                this.work_elapsed.extend(timing.elapsed);
                let now = std::time::Instant::now();
                for (start, time) in timing.started {
                    if let Ok(elapsed) = (chrono::Local::now() - time).to_std()
                        && let Some(at) = now.checked_sub(elapsed)
                    {
                        this.work_started.entry(start).or_insert(at);
                    }
                }
                if let Some(scroll) = scroll {
                    this.scroller.scroll_to(scroll);
                } else {
                    this.scroller.set_follow_mode(FollowMode::Tail);
                    this.scroller.scroll_to_end();
                }
                this.loading = false;
                if this.interrupt_requested && this.is_running() {
                    this.stop(cx);
                }
                this.sync(None, cx);
                // Messages sent while loading go out now, as the person sent them.
                for (id, steer) in std::mem::take(&mut this.held_until_loaded) {
                    if steer || !this.is_running() {
                        this.deliver_queued(id, steer, cx);
                    }
                }
                if this.model.last_status.is_none()
                    || this.model.last_status == Some(del_agent::TurnStatus::Completed)
                {
                    this.send_next(cx);
                }
                cx.notify();
            });
            if replayed.is_err() {
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if this
                            .update(cx, |this, cx| {
                                let before = this.completed_turns;
                                let mut ready = vec![event];
                                // Drain only what is ready now, bounded so rendering gets
                                // control back. Preserve completion/start boundaries.
                                for _ in 0..rx.len().min(128) {
                                    if let Ok(event) = rx.try_recv() {
                                        ready.push(event);
                                    }
                                }
                                for event in coalesce_deltas(ready) {
                                    this.on_event(&event, cx);
                                }
                                if this.completed_turns > before
                                    && this.model.last_status
                                        == Some(del_agent::TurnStatus::Completed)
                                {
                                    this.send_next(cx);
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Dropped events can include a completion; rebuild from
                        // the session's full history on a fresh subscription.
                        let _ = this.update(cx, |this, cx| {
                            if let Some(id) = this.session.clone() {
                                this.follow_from(id, true, cx);
                            }
                        });
                        return;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            let _ = this.update(cx, |this, _| this.closed = true);
        })
        .detach();
    }

    /// True once the session's stream ended; the app drops the view and
    /// builds a fresh one if the session is reopened.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Run a backend call on the tokio runtime; failures become toast notifications.
    fn run(
        &mut self,
        cx: &mut Context<Self>,
        op: impl Future<Output = Result<(), AgentError>> + Send + 'static,
    ) {
        let task = self.backend.runtime.spawn(op);
        cx.spawn(async move |this, cx| match task.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => report(&this, cx, err.to_string()),
            Err(err) => report(&this, cx, err.to_string()),
        })
        .detach();
    }

    /// Per-frame motion bookkeeping, driven by the frame clock instead of
    /// timers: folding work, disclosure springs, and the anchored answer.
    fn advance_motion(&mut self, window: &mut Window, cx: &App) {
        let now = std::time::Instant::now();
        let mut moving = false;
        let finished: Vec<usize> = self
            .work_reveals
            .iter()
            .filter_map(|(start, reveal)| reveal.sample(now).1.then_some(*start))
            .collect();
        let ranges: Vec<_> = self
            .work_sections
            .iter()
            .filter(|section| self.work_reveals.contains_key(&section.start))
            .map(|section| section.range())
            .collect();
        for range in ranges {
            self.scroller.remeasure_items(range);
            moving = true;
        }
        for start in finished {
            self.work_reveals.remove(&start);
        }
        self.settling.retain(|(range, until)| {
            let end = range.end.min(self.scroller.item_count());
            self.scroller.remeasure_items(range.start.min(end)..end);
            now < *until
        });
        moving |= !self.settling.is_empty();
        if let Some((answer, top)) = self.answer_anchor {
            // Pin the answer's top to where it was while its work folds above it.
            self.scroller.scroll_to(ListOffset {
                item_ix: answer,
                offset_in_item: zpx(0.),
            });
            self.scroller.scroll_by(-top);
            if self.work_reveals.is_empty() {
                self.answer_anchor = None;
            }
        }
        if moving && !cx.reduce_motion() {
            window.request_animation_frame();
        }
    }

    /// One pill over the bottom of the transcript, so nothing shifts the
    /// composer: Stopping…, the approval waiting below, or a jump to the latest.
    fn render_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let approval = self
            .model
            .blocks
            .iter()
            .rposition(|b| {
                matches!(b, Block::Approval(a) if a.state == super::model::ApprovalState::Pending)
            })
            .filter(|_| self.model.is_running());
        let offscreen = |ix: usize| {
            self.scroller.item_is_below_viewport(ix) != Some(false)
                || self.scroller.item_is_above_viewport(ix) == Some(true)
        };
        enum Pill {
            Stopping,
            Approval(usize),
            Latest,
        }
        let pill = if self.model.is_running() && self.interrupt_requested {
            Some(Pill::Stopping)
        } else if let Some(ix) = approval.filter(|ix| offscreen(*ix)) {
            Some(Pill::Approval(ix))
        } else if shows_jump_to_latest(
            self.scroller.is_following_tail(),
            self.scroller.is_scrolled_to_end(),
            self.unseen,
            self.model.is_running(),
        ) {
            Some(Pill::Latest)
        } else {
            None
        };
        let Some(pill) = pill else {
            return div().into_any_element();
        };
        let (key, label, color): (&str, Option<&str>, Rgba) = match pill {
            Pill::Stopping => ("stopping", Some("Stopping…"), crate::ui::secondary_text()),
            Pill::Approval(_) => (
                "approval",
                Some("Waiting for your approval"),
                crate::appearance::ActionTone::Warning.foreground(),
            ),
            Pill::Latest => ("latest", None, crate::ui::secondary_text()),
        };
        let button = h_flex()
            .id(SharedString::from(format!("transcript-pill-{key}")))
            .debug_selector(|| "transcript-pill".into())
            .h(zpx(28.))
            .when(label.is_none(), |pill| pill.w(zpx(28.)).justify_center())
            .when(label.is_some(), |pill| pill.px(zpx(crate::ui::SPACE_LG)))
            .gap(zpx(crate::ui::SPACE_SM))
            .items_center()
            .rounded_full()
            .bg(crate::appearance::popup_surface())
            .border_1()
            .border_color(crate::appearance::separator())
            .shadow_md()
            .text_size(zpx(TEXT_SM))
            .text_color(color)
            .when(label.is_none(), |pill| {
                pill.child(
                    crate::icons::icon(IconName::ArrowDown)
                        .with_size(zpx(14.))
                        .text_color(crate::ui::secondary_text()),
                )
            })
            .when_some(label, |pill, label| pill.child(label));
        let button = match pill {
            Pill::Stopping => button.into_any_element(),
            Pill::Approval(ix) => button
                .keyboard_control()
                .role(gpui::Role::Button)
                .aria_label("Show the approval request")
                .cursor_pointer()
                .hover(|pill| pill.bg(crate::appearance::composer_surface()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.jump = Some(MessageJump {
                        target: JumpTarget::Row(ix),
                        inset: SEARCH_INSET,
                        from: this.scroller.scroll_px_offset_for_scrollbar().y.as_f32(),
                        started: cx.background_executor().now(),
                    });
                    cx.notify();
                }))
                .into_any_element(),
            Pill::Latest => button
                .keyboard_control()
                .role(gpui::Role::Button)
                .aria_label("Jump to latest")
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Jump to latest").build(window, cx)
                })
                .cursor_pointer()
                .hover(|pill| pill.bg(crate::appearance::composer_surface()))
                .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx)))
                .into_any_element(),
        };
        h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom(zpx(crate::ui::SPACE_MD))
            .justify_center()
            .child(div().child(button).with_animation(
                SharedString::from(format!("transcript-pill-enter-{key}")),
                Animation::new(crate::ui::POPOVER_ENTER).with_easing(crate::ui::ease_out),
                |pill, t| pill.opacity(t).mt(zpx(crate::ui::POPOVER_LIFT * (1. - t))),
            ))
            .into_any_element()
    }

    /// Ease to the end of the transcript, then follow new content again.
    pub(super) fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.jump = Some(MessageJump {
            target: JumpTarget::End,
            inset: 0.,
            from: self.scroller.scroll_px_offset_for_scrollbar().y.as_f32(),
            started: cx.background_executor().now(),
        });
        cx.notify();
    }
}

/// Report a failed backend call (Stop, an approval answer) as a toast.
fn report(this: &WeakEntity<ChatView>, cx: &mut AsyncApp, message: String) {
    let _ = this.update(cx, |this, cx| {
        this.pending_errors.push(message);
        this.model.halt();
        this.sync(None, cx);
    });
}

/// The conversation could not be opened: say so in the transcript, with Retry,
/// instead of leaving it empty. The row is the only place this is reported.
fn report_load(this: &WeakEntity<ChatView>, cx: &mut AsyncApp, message: String) {
    let _ = this.update(cx, |this, cx| {
        this.loading = false;
        let change = this.model.push_error(message, false);
        if let Change::Pushed(row) = change {
            this.load_failed = Some(row);
        }
        this.sync(Some(change), cx);
    });
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.loading && self.model.blocks.is_empty() {
            return super::conversation_loading("Loading conversation…".into(), cx.reduce_motion());
        }
        self.apply_search_jump(cx);
        let view = cx.entity();
        self.advance_motion(window, cx);
        if let Some(jump) = &self.jump {
            let t = if cx.reduce_motion() {
                1.0
            } else {
                // The executor's clock: real time in the app, controlled in tests.
                (cx.background_executor()
                    .now()
                    .saturating_duration_since(jump.started)
                    .as_secs_f32()
                    / JUMP_SECONDS)
                    .min(1.0)
            };
            let max = self.scroller.max_offset_for_scrollbar().y.as_f32();
            let target = match jump.target {
                JumpTarget::Row(ix) => {
                    self.scroller.scroll_to(ListOffset {
                        item_ix: ix,
                        offset_in_item: zpx(0.0),
                    });
                    self.scroller.scroll_px_offset_for_scrollbar().y.as_f32() + jump.inset
                }
                JumpTarget::End => -max,
            }
            .clamp(-max, 0.);
            if t < 1.0 {
                let eased = crate::ui::ease_out(t);
                self.scroller.set_offset_from_scrollbar(point(
                    zpx(0.0),
                    gpui::px(jump.from + (target - jump.from) * eased),
                ));
                window.request_animation_frame();
            } else {
                if jump.target == JumpTarget::End {
                    self.scroller.set_follow_mode(FollowMode::Tail);
                    self.unseen = false;
                } else if jump.inset > 0. {
                    self.scroller
                        .set_offset_from_scrollbar(point(zpx(0.0), gpui::px(target)));
                }
                self.jump = None;
            }
        }
        if self.scroller.is_following_tail() {
            self.unseen = false;
        }
        let top = self.scroller.logical_scroll_top().item_ix;
        let prompts = self.prompt_rows.clone();
        let at_end = self.scroller.is_following_tail()
            || self.scroller.is_scrolled_to_end() == Some(true)
            || self.scroller.max_offset_for_scrollbar().y <= zpx(0.0);
        let active = self
            .jump
            .as_ref()
            .and_then(|jump| match jump.target {
                JumpTarget::Row(ix) => Some(ix),
                JumpTarget::End => None,
            })
            .or_else(|| active_prompt(&prompts, top, at_end));
        let hover = self.rail_hover;
        let overlay = self.render_overlay(cx);
        v_flex()
            .size_full()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex().size_full().min_w_0().px(zpx(6.)).child(
                            list(self.scroller.clone(), {
                                let view = view.clone();
                                move |ix, _, cx| {
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .when(ix == 0, |row| {
                                            row.pt(zpx(super::blocks::TRANSCRIPT_TOP))
                                        })
                                        .child(view.update(cx, |this, cx| this.render_row(ix, cx)))
                                        .into_any_element()
                                }
                            })
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .pb(zpx(crate::ui::SPACE_MD)),
                        ),
                    )
                    .child(overlay)
                    // Overlaid so the transcript and the composer share one column.
                    .child(
                        v_flex()
                            .id("message-navigation")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(zpx(if prompts.len() >= 6 { 24.0 } else { 0.0 }))
                            .flex_none()
                            .justify_center()
                            .py(zpx(crate::ui::SPACE_LG))
                            .children((prompts.len() >= 6).then(|| {
                                v_flex()
                                    .id("message-ticks")
                                    .max_h_full()
                                    .overflow_y_scroll()
                                    .children(prompts.into_iter().enumerate().map(
                                        |(position, ix)| {
                                            let text = self.navigation_text(ix).unwrap_or_default();
                                            let tooltip_view = view.downgrade();
                                            let strength = |hover: Option<usize>| {
                                                hover.map_or(0.0, |hover| {
                                                    (1.0 - position.abs_diff(hover) as f32 / 4.0)
                                                        .max(0.0)
                                                })
                                            };
                                            let selected = hover
                                                .map_or(active == Some(ix), |hover| {
                                                    hover == position
                                                });
                                            // Interruptible: rapid hovers retarget from where
                                            // each tick is drawn instead of restarting.
                                            let rail = || {
                                                gpui_kit::base::Transition::new(
                                                    std::time::Duration::from_millis(100),
                                                )
                                                .ease(crate::ui::ease_out)
                                            };
                                            let weight = gpui_kit::base::transition(
                                                ElementId::NamedInteger(
                                                    "rail-weight".into(),
                                                    ix as u64,
                                                ),
                                                strength(hover),
                                                rail(),
                                                window,
                                                cx,
                                            );
                                            let alpha = gpui_kit::base::transition(
                                                ElementId::NamedInteger(
                                                    "rail-alpha".into(),
                                                    ix as u64,
                                                ),
                                                if selected { 0.9 } else { 0.22 },
                                                rail(),
                                                window,
                                                cx,
                                            );
                                            div()
                                                .id(("message-jump", ix))
                                                .keyboard_control()
                                                .role(gpui::Role::Button)
                                                .aria_label(format!(
                                                    "Jump to message: {}",
                                                    text.chars().take(80).collect::<String>()
                                                ))
                                                .tooltip(move |window, cx| {
                                                    // Read the text only when the tooltip opens.
                                                    let text = tooltip_view
                                                        .upgrade()
                                                        .and_then(|view| {
                                                            view.read(cx)
                                                                .navigation_text(ix)
                                                                .map(str::to_owned)
                                                        })
                                                        .unwrap_or_default();
                                                    let max_width = zpx(320.).min(
                                                        (window.viewport_size().width - zpx(48.))
                                                            .max(zpx(1.)),
                                                    );
                                                    gpui_kit::component::tooltip::Tooltip::element(
                                                        move |_, _| {
                                                            div()
                                                                .max_w(max_width)
                                                                .max_h(zpx(72.))
                                                                .line_height(zpx(18.))
                                                                .whitespace_normal()
                                                                .text_ellipsis()
                                                                .line_clamp(4)
                                                                .child(text.clone())
                                                        },
                                                    )
                                                    .build(window, cx)
                                                })
                                                .tooltip_show_delay(std::time::Duration::ZERO)
                                                .on_hover(cx.listener(
                                                    move |this, hovered, _, cx| {
                                                        let next = if *hovered {
                                                            Some(position)
                                                        } else if this.rail_hover == Some(position)
                                                        {
                                                            None
                                                        } else {
                                                            this.rail_hover
                                                        };
                                                        if next != this.rail_hover {
                                                            this.rail_hover = next;
                                                            if next.is_some() {
                                                                crate::haptics::tick();
                                                            }
                                                            cx.notify();
                                                        }
                                                    },
                                                ))
                                                .group("message-tick")
                                                .w_full()
                                                .h(zpx(10.0))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_start()
                                                .pl(zpx(2.0))
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.reveal_work_row(ix);
                                                    this.jump = Some(MessageJump {
                                                        target: JumpTarget::Row(ix),
                                                        inset: 0.,
                                                        from: this
                                                            .scroller
                                                            .scroll_px_offset_for_scrollbar()
                                                            .y
                                                            .as_f32(),
                                                        started: cx.background_executor().now(),
                                                    });
                                                    cx.notify();
                                                }))
                                                // Only the hovered marker (or current message when
                                                // idle) is bright; neighbors only lengthen. Width and
                                                // brightness ease together.
                                                .child(
                                                    div()
                                                        .h(zpx(2.0))
                                                        .w(zpx(6.0 + 12.0 * weight))
                                                        .bg(rgba(0xffffffff).opacity(alpha)),
                                                )
                                        },
                                    ))
                            })),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_short_message_is_located_past_longer_messages_containing_it() {
        // Saved transcript: "yes please", "yes", "yes". The second "yes" message
        // has two earlier matches, so it is the third match overall.
        let earlier = ["yes please", "yes"];
        let occurrence = substring_matches(earlier, "yes").count();
        let rows = ["yes please", "yes", "yes"];
        assert_eq!(substring_matches(rows, "yes").nth(occurrence), Some((2, 0)));
        assert_eq!(
            substring_matches(["yes please"], "yes").next(),
            Some((0, 0))
        );
    }
    use ::gpui;
    use core::prelude::v1::test;

    #[::core::prelude::v1::test]
    fn message_rail_highlights_last_prompt_at_bottom_even_when_earlier_prompt_is_at_top() {
        assert_eq!(active_prompt(&[0, 3, 6, 9], 3, true), Some(9));
        assert_eq!(active_prompt(&[0, 3, 6, 9], 3, false), Some(3));
        assert_eq!(active_prompt(&[0, 3, 6, 9], 8, false), Some(6));
        assert_eq!(active_prompt(&[], 0, true), None);
    }

    #[::core::prelude::v1::test]
    fn automatic_delivery_never_pulls_a_reader_down() {
        // The person's own send always shows the new message.
        assert!(scrolls_on_delivery(true, false));
        assert!(scrolls_on_delivery(true, true));
        // Queue draining, replay and peer work only follow an already-followed tail.
        assert!(!scrolls_on_delivery(false, false));
        assert!(scrolls_on_delivery(false, true));
    }

    #[::core::prelude::v1::test]
    fn jump_to_latest_shows_only_above_new_or_live_content() {
        assert!(shows_jump_to_latest(false, Some(false), true, false));
        assert!(shows_jump_to_latest(false, Some(false), false, true));
        assert!(!shows_jump_to_latest(false, Some(false), false, false));
        assert!(!shows_jump_to_latest(true, Some(false), true, true));
        assert!(!shows_jump_to_latest(false, Some(true), true, true));
        assert!(!shows_jump_to_latest(false, None, true, true));
    }

    #[gpui::test]
    fn failed_turn_errors_show_inline_once_and_recoverable_ones_toast(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager,
        };
        let view = cx.new(|cx| ChatView::new(backend, None, None, cx));
        view.update(cx, |view, cx| {
            for event in [
                AgentEvent::UserMessage { text: "go".into() },
                AgentEvent::TurnStarted {
                    turn_id: "t".into(),
                },
                AgentEvent::Error {
                    message: "rate limited, retrying".into(),
                    recoverable: true,
                },
            ] {
                view.on_event(&event, cx);
            }
            assert_eq!(view.pending_errors, ["rate limited, retrying"]);
            view.pending_errors.clear();
            // A fatal error fails the turn; its row says why, so no toast too.
            for event in [
                AgentEvent::Error {
                    message: "model overloaded".into(),
                    recoverable: false,
                },
                AgentEvent::TurnCompleted {
                    turn_id: "t".into(),
                    status: del_agent::TurnStatus::Failed,
                },
            ] {
                view.on_event(&event, cx);
            }
            assert!(view.pending_errors.is_empty());
            assert_eq!(
                super::super::blocks::failed_turn_error(&view.model.blocks).as_deref(),
                Some("model overloaded")
            );
            // A recoverable error the failed row already shows is not toasted twice.
            for event in [
                AgentEvent::UserMessage {
                    text: "again".into(),
                },
                AgentEvent::TurnStarted {
                    turn_id: "u".into(),
                },
                AgentEvent::Error {
                    message: "connection reset".into(),
                    recoverable: true,
                },
                AgentEvent::TurnCompleted {
                    turn_id: "u".into(),
                    status: del_agent::TurnStatus::Failed,
                },
            ] {
                view.on_event(&event, cx);
            }
            assert!(view.pending_errors.is_empty(), "{:?}", view.pending_errors);
        });
    }

    #[test]
    fn user_text_renders_literally_with_breaks_and_mapped_ranges() {
        let (markdown, range) =
            super::super::blocks::literal_markdown("# not a heading\n    keep *this*", 25..31);
        assert!(markdown.starts_with("\\# not a heading\\\n"));
        assert!(!markdown.contains("    "), "leading spaces never make code");
        assert_eq!(&markdown[range], "\\*this\\*");
    }
    #[test]
    fn history_replay_skips_only_the_matching_opening_prompt() {
        assert!(replays_opening_row(Some("Build it"), "Build it"));
        // A resync keeps the opening row's prose; native history appends markers.
        assert!(replays_opening_row(
            Some("inspect these"),
            "inspect these\n[attached image: /tmp/photo.png]"
        ));
        assert!(replays_opening_row(Some(""), "[Image]"));
        assert!(!replays_opening_row(None, "Build it"));
        assert!(!replays_opening_row(Some("Build it"), "Build it again"));
        assert!(!replays_opening_row(Some("Build"), "Build it"));
    }

    #[test]
    fn ready_deltas_coalesce_without_crossing_turns_or_tools() {
        let delta = |turn: &str, text: &str| AgentEvent::AssistantTextDelta {
            turn_id: turn.into(),
            text: text.into(),
        };
        let events = coalesce_deltas(vec![
            delta("a", "hello "),
            delta("a", "世界"),
            AgentEvent::ToolCallStarted {
                id: "tool".into(),
                name: "Read".into(),
                input_summary: "main.rs".into(),
            },
            delta("a", "tail"),
            delta("b", "next"),
        ]);
        assert_eq!(events.len(), 4);
        assert!(
            matches!(&events[0], AgentEvent::AssistantTextDelta { text, .. } if text == "hello 世界")
        );
        assert!(matches!(&events[1], AgentEvent::ToolCallStarted { .. }));
        assert!(
            matches!(&events[2], AgentEvent::AssistantTextDelta { turn_id, text } if turn_id == "a" && text == "tail")
        );
        assert!(
            matches!(&events[3], AgentEvent::AssistantTextDelta { turn_id, text } if turn_id == "b" && text == "next")
        );
        assert_eq!(coalesce_deltas(vec![delta("a", "partial")]).len(), 1);
    }

    #[gpui::test]
    fn collapsed_work_keeps_final_copy_and_reveals_navigation_targets(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.on_event(
                &AgentEvent::UserMessage {
                    text: "Delegate work".into(),
                },
                cx,
            );
            for (turn, text) in [
                ("one", "Waiting"),
                ("two", "More work"),
                ("three", "Final answer"),
            ] {
                view.on_event(
                    &AgentEvent::TurnStarted {
                        turn_id: turn.into(),
                    },
                    cx,
                );
                view.on_event(
                    &AgentEvent::AssistantMessage {
                        turn_id: turn.into(),
                        text: text.into(),
                    },
                    cx,
                );
                view.on_event(
                    &AgentEvent::TurnCompleted {
                        turn_id: turn.into(),
                        status: del_agent::TurnStatus::Completed,
                    },
                    cx,
                );
            }
            view.refresh_work_sections();
            assert_eq!(
                view.work_sections
                    .iter()
                    .map(|s| s.range())
                    .collect::<Vec<_>>(),
                vec![1..4]
            );
            assert!(view.expanded_work.is_empty());
            view.copy_response(1, cx);
            assert!(view.copied_responses.is_empty());
            view.copy_response(3, cx);
            assert!(view.copied_responses.contains_key(&3));
            view.reveal_work_row(2);
            assert!(view.expanded_work.contains(&1));
            assert!(!view.final_response(1));
            assert!(view.final_response(3));
            view.remove_row(0);
            view.refresh_work_sections();
            assert_eq!(
                view.work_sections
                    .iter()
                    .map(|s| s.range())
                    .collect::<Vec<_>>(),
                vec![0..3]
            );
            assert!(view.expanded_work.contains(&0));
        });
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("Final answer")
        );
    }

    #[gpui::test]
    fn copying_full_reply_confirms_then_resets(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        let reply = "**Full reply**\n\n```rust\nlet x = 1;\n```";
        view.update(cx, |view, cx| {
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "copy".into(),
                    text: reply.into(),
                },
                cx,
            );
            view.copy_response(0, cx);
            assert!(view.copied_responses.is_empty());
            view.on_event(
                &AgentEvent::TurnCompleted {
                    turn_id: "copy".into(),
                    status: del_agent::TurnStatus::Completed,
                },
                cx,
            );
            view.copy_response(0, cx);
            assert!(view.copied_responses.contains_key(&0));
        });
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(reply)
        );
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1000));
        view.update(cx, |view, cx| view.copy_response(0, cx));
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.copied_responses.contains_key(&0))
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1000));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(!view.copied_responses.contains_key(&0))
        });
    }

    #[gpui::test]
    fn peer_receipts_add_one_navigation_tick_and_update_in_place(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        let mut message = del_agent::peers::PeerMessage {
            id: "receipt".into(),
            idempotency_key: "request".into(),
            from: SessionId::new("a"),
            to: SessionId::new("b"),
            reply_to: None,
            text: "Inspect the API".into(),
            created_at_ms: 1,
            state: del_agent::peers::DeliveryState::Queued,
            error: None,
        };
        view.update(cx, |view,cx| {
            view.on_event(&AgentEvent::PeerMessage { message:message.clone() },cx);
            assert_eq!(view.prompt_rows,vec![0]);
            message.state=del_agent::peers::DeliveryState::Delivered;
            view.on_event(&AgentEvent::PeerMessage { message:message.clone() },cx);
            assert_eq!(view.prompt_rows,vec![0]);
            assert_eq!(view.navigation_text(0), Some("Inspect the API"));
            view.on_event(&AgentEvent::UserMessage { text: "   ".into() }, cx);
            assert_eq!(view.prompt_rows, vec![0], "blank messages must not add rail ticks");
            view.refresh_prompt_rows();
            assert_eq!(view.prompt_rows, vec![0], "replay must use the same filter");
            message.id = "empty-receipt".into();
            message.text = "\n  ".into();
            view.on_event(&AgentEvent::PeerMessage { message: message.clone() }, cx);
            assert_eq!(view.prompt_rows, vec![0]);
            message.text = "Now readable".into();
            view.on_event(&AgentEvent::PeerMessage { message: message.clone() }, cx);
            assert_eq!(view.prompt_rows, vec![0, 2]);
            assert_eq!(view.navigation_text(2), Some("Now readable"));
            view.on_event(&AgentEvent::UserMessage { text: "[Image]".into() }, cx);
            assert_eq!(view.prompt_rows, vec![0, 2, 3]);
            assert_eq!(view.navigation_text(3), Some("Attachments"));

            assert!(matches!(&view.model.blocks[0], Block::Peer(m) if m.state == del_agent::peers::DeliveryState::Delivered));
        });
    }

    #[gpui::test]
    fn failed_delivery_removes_its_own_row_and_renumbers_side_tables(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.on_event(
                &AgentEvent::UserMessage {
                    text: "first".into(),
                },
                cx,
            );
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "a".into(),
                    text: "one".into(),
                },
                cx,
            );
            view.on_event(
                &AgentEvent::TurnCompleted {
                    turn_id: "a".into(),
                    status: del_agent::TurnStatus::Completed,
                },
                cx,
            );
            // Two optimistic prompts; the earlier one fails after the later one was queued.
            let change = view.model.push_user("undelivered".into(), false);
            view.optimistic_rows.insert("item-1".into(), 2);
            view.sync(Some(change), cx);
            let change = view.model.push_user("later".into(), true);
            view.optimistic_rows.insert("item-2".into(), 3);
            view.sync(Some(change), cx);
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "b".into(),
                    text: "two".into(),
                },
                cx,
            );
            view.on_event(
                &AgentEvent::TurnCompleted {
                    turn_id: "b".into(),
                    status: del_agent::TurnStatus::Completed,
                },
                cx,
            );
            let before = view.response_times[&4];
            assert_eq!(view.prompt_rows, vec![0, 2, 3]);
            let row = view.optimistic_rows.remove("item-1").unwrap();
            view.remove_row(row);
            assert!(matches!(&view.model.blocks[2], Block::User { text, .. } if text == "later"));
            assert_eq!(view.optimistic_rows["item-2"], 2);
            assert_eq!(view.response_times.get(&3), Some(&before));
            assert!(view.assistant_text.contains_key(&3) && !view.assistant_text.contains_key(&4));
            assert!(view.model.is_completed_response(3));
            assert_eq!(view.prompt_rows, vec![0, 2]);
            assert_eq!(view.scroller.item_count(), view.model.blocks.len());
        });
    }

    #[gpui::test]
    fn working_label_follows_latest_user_message_and_clears_on_completion(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            let change = view.model.push_user("first".into(), false);
            view.sync(Some(change), cx);
            assert_eq!(view.working_label(0).as_deref(), Some("Working for 0s"));
            let change = view.model.push_user("followup".into(), true);
            view.sync(Some(change), cx);
            assert!(view.working_label(0).is_none());
            assert!(view.working_label(1).is_some());
            view.model.phase = super::super::model::Phase::Idle;
            view.sync(None, cx);
            assert!(view.working_label(1).is_none());
        });
    }

    #[gpui::test]
    fn first_turn_started_while_loading_still_records_its_elapsed_time(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            let change = view.model.push_user("first".into(), false);
            view.sync(Some(change), cx);
            // TurnStarted landed during loading, so nothing recorded a start.
            assert!(view.work_started.is_empty());
            let started = AgentEvent::TurnStarted {
                turn_id: "t1".into(),
            };
            let change = view.model.apply(&started);
            view.sync(change, cx);
            let reply = AgentEvent::AssistantTextDelta {
                turn_id: "t1".into(),
                text: "done".into(),
            };
            let change = view.model.apply(&reply);
            view.sync(change, cx);
            view.on_event(
                &AgentEvent::TurnCompleted {
                    turn_id: "t1".into(),
                    status: del_agent::TurnStatus::Completed,
                },
                cx,
            );
            assert!(view.work_elapsed.contains_key(&1));
        });
    }

    /// The real path: the turn starts on the backend, then the view attaches and
    /// replays it while loading, so only the live clock can time it.
    #[gpui::test]
    fn first_turn_attached_mid_flight_records_its_elapsed_time(cx: &mut gpui::TestAppContext) {
        use std::{sync::Arc, time::Duration};
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            [Arc::new(
                del_agent::MockBackend::new(del_agent::Provider::Claude)
                    .with_delay(Duration::from_millis(40)),
            ) as Arc<dyn del_agent::AgentBackend>],
        )
        .unwrap();
        let session = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider: del_agent::Provider::Claude,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: Default::default(),
            }))
            .unwrap();
        runtime
            .block_on(manager.send_prompt(&session.id, UserInput::text("tool please")))
            .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager: manager.clone(),
                },
                Some(session.id.clone()),
                None,
                cx,
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while view.read_with(cx, |view, _| view.completed_turns == 0) {
            assert!(std::time::Instant::now() < deadline, "turn never completed");
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(10));
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.work_elapsed
                    .keys()
                    .any(|ix| matches!(view.model.blocks[*ix], Block::Assistant { .. })),
                "the first turn lost its elapsed time"
            );
        });
        runtime.block_on(manager.shutdown());
        runtime.shutdown_background();
    }

    #[gpui::test]
    fn streamed_chunks_update_markdown_before_turn_completion(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager,
                },
                None,
                None,
                cx,
            )
        });
        for (chunk, expected) in [
            ("First", "First"),
            (" second", "First second"),
            (" third", "First second third"),
            (
                "\n\n:codex-file-citation{path=\"/tmp/Report",
                "First second third\n:codex-file-citation{path=\"/tmp/Report",
            ),
            (
                ".pdf\" purpose=\"output\"}",
                "First second third\nReport.pdf",
            ),
            ("\n\nReady.", "First second third\nReport.pdf\nReady."),
        ] {
            view.update(cx, |view, cx| {
                view.on_event(
                    &AgentEvent::AssistantTextDelta {
                        turn_id: "streaming".into(),
                        text: chunk.into(),
                    },
                    cx,
                )
            });
            cx.run_until_parked();
            view.read_with(cx, |view, cx| {
                let text = view.assistant_text.values().next().unwrap();
                assert_eq!(text.read(cx).rendered_text().as_str().trim(), expected);
                assert!(view.model.last_status.is_none());
            });
        }
        // A provider's final snapshot may revise already streamed Markdown.
        view.update(cx, |view, cx| {
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "streaming".into(),
                    text: "**Corrected** 世界\n\n::codex-file-citation{path=\"/tmp/Final.pdf\"}"
                        .into(),
                },
                cx,
            )
        });
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let text = view.assistant_text.values().next().unwrap();
            assert_eq!(
                text.read(cx).rendered_text().as_str().trim(),
                "Corrected 世界\nFinal.pdf"
            );
            assert!(
                matches!(&view.model.blocks[0], Block::Assistant { text, .. }
                if text.contains("::codex-file-citation"))
            );
        });
        // Loading saved messages creates a fresh text view, without streaming deltas.
        view.update(cx, |view, cx| {
            view.reset_for_resync();
            view.replaying = true;
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "restored".into(),
                    text: ":codex-file-citation{path=\"/tmp/Saved.pdf\" purpose=\"output\"}".into(),
                },
                cx,
            );
            view.replaying = false;
            // Replayed replies parse when first drawn.
            view.ensure_assistant_text(0, cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.assistant_text[&0]
                    .read(cx)
                    .rendered_text()
                    .as_str()
                    .trim(),
                "Saved.pdf"
            );
        });
    }

    #[gpui::test]
    fn followups_wait_for_completion_then_send_in_order(cx: &mut gpui::TestAppContext) {
        followup_delivery_scenario(cx, None, FollowupScenario::Ordered);
    }

    #[gpui::test]
    #[ignore = "requires an installed, authenticated Claude CLI"]
    fn real_claude_followups_send_automatically(cx: &mut gpui::TestAppContext) {
        followup_delivery_scenario(
            cx,
            Some(del_agent::Provider::Claude),
            FollowupScenario::Ordered,
        );
        followup_delivery_scenario(
            cx,
            Some(del_agent::Provider::Claude),
            FollowupScenario::Steering,
        );
    }

    #[gpui::test]
    fn followups_retry_when_completion_precedes_provider_readiness(cx: &mut gpui::TestAppContext) {
        followup_delivery_scenario(cx, None, FollowupScenario::DelayedReady);
    }

    #[gpui::test]
    fn stop_pauses_followups_during_busy_retry(cx: &mut gpui::TestAppContext) {
        followup_delivery_scenario(cx, None, FollowupScenario::StopDuringRetry);
    }

    #[gpui::test]
    fn steering_preserves_automatic_followups(cx: &mut gpui::TestAppContext) {
        followup_delivery_scenario(cx, None, FollowupScenario::Steering);
    }

    #[gpui::test]
    #[ignore = "requires an installed, authenticated Codex CLI"]
    fn real_codex_followups_steering_and_stop(cx: &mut gpui::TestAppContext) {
        for scenario in [
            FollowupScenario::Ordered,
            FollowupScenario::Steering,
            FollowupScenario::StopRunning,
        ] {
            followup_delivery_scenario(cx, Some(del_agent::Provider::Codex), scenario);
        }
    }

    #[gpui::test]
    #[ignore = "requires installed OpenCode and access to its free model"]
    fn real_opencode_followups_steering_and_stop(cx: &mut gpui::TestAppContext) {
        for scenario in [
            FollowupScenario::Ordered,
            FollowupScenario::Steering,
            FollowupScenario::StopRunning,
        ] {
            followup_delivery_scenario(cx, Some(del_agent::Provider::OpenCode), scenario);
        }
    }

    #[derive(Clone, Copy)]
    enum FollowupScenario {
        Ordered,
        DelayedReady,
        StopDuringRetry,
        Steering,
        StopRunning,
    }

    fn followup_delivery_scenario(
        cx: &mut gpui::TestAppContext,
        native: Option<del_agent::Provider>,
        scenario: FollowupScenario,
    ) {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let live = native.is_some();
        let provider = native.unwrap_or(del_agent::Provider::Codex);
        let agent: Arc<dyn del_agent::AgentBackend> = match native {
            Some(del_agent::Provider::Claude) => {
                Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
                    setting_sources: Some("project".into()),
                    ..Default::default()
                }))
            }
            Some(del_agent::Provider::Codex) => {
                Arc::new(del_codex::CodexBackend::new(Default::default()))
            }
            Some(del_agent::Provider::OpenCode) => {
                Arc::new(del_opencode::OpenCodeBackend::with_environment(
                    [
                        "XDG_DATA_HOME",
                        "XDG_CONFIG_HOME",
                        "XDG_STATE_HOME",
                        "XDG_CACHE_HOME",
                    ]
                    .into_iter()
                    .map(|key| {
                        (
                            key.into(),
                            dir.path().join(key).to_string_lossy().into_owned(),
                        )
                    })
                    .collect(),
                ))
            }
            Some(del_agent::Provider::Acp) => unreachable!(),
            None => Arc::new(
                del_agent::MockBackend::new(provider)
                    .with_delay(std::time::Duration::from_millis(3))
                    .with_completion_delay(std::time::Duration::from_millis(
                        if matches!(
                            scenario,
                            FollowupScenario::DelayedReady | FollowupScenario::StopDuringRetry
                        ) {
                            20
                        } else {
                            0
                        },
                    )),
            ),
        };
        let manager = SessionManager::new(dir.path().join("sessions.jsonl"), [agent]).unwrap();
        let session = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider,
                model: match native {
                    Some(del_agent::Provider::Claude) => Some("haiku".into()),
                    Some(del_agent::Provider::Codex) => Some("gpt-5.6-luna".into()),
                    Some(del_agent::Provider::OpenCode) => Some("opencode/big-pickle".into()),
                    _ => None,
                },
                reasoning_effort: None,
                service_tier: None,
                permission_mode: del_agent::PermissionMode::FullAuto,
            }))
            .unwrap();
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager: manager.clone(),
        };
        let view = cx.new(|cx| ChatView::new(backend, None, None, cx));
        view.update(cx, |view, cx| {
            view.session = Some(session.id.clone());
            view.loading = true;
            view.follow(session.id.clone(), cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if !view.read_with(cx, |view, _| view.loading) {
                break;
            }
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        let prompts = if live {
            [
                "Reply with exactly FIRST. No tools.",
                "Reply with exactly SECOND. No tools.",
                "Reply with exactly THIRD. No tools.",
            ]
        } else {
            ["first", "second", "third"]
        };
        view.update(cx, |view, cx| {
            view.submit_input(UserInput::text(prompts[0]), false, cx);
            view.submit_input(UserInput::text(prompts[1]), false, cx);
            view.submit_input(UserInput::text(prompts[2]), false, cx);
            assert_eq!(view.queue.items.len(), 3);
            assert_eq!(
                view.queue.items.iter().filter(|item| item.sending).count(),
                1
            );
        });
        if matches!(
            scenario,
            FollowupScenario::Steering
                | FollowupScenario::StopDuringRetry
                | FollowupScenario::StopRunning
        ) {
            for _ in 0..500 {
                cx.run_until_parked();
                let ready = view.read_with(cx, |view, _| {
                    !view.queue.items.iter().any(|item| item.sending)
                        && if matches!(
                            scenario,
                            FollowupScenario::Steering | FollowupScenario::StopRunning
                        ) {
                            view.is_running() && view.completed_turns == 0
                        } else {
                            !view.is_running() && view.completed_turns == 1
                        }
                });
                if ready {
                    break;
                }
                runtime.block_on(async {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await
                });
            }
            view.update(cx, |view, cx| {
                assert_eq!(view.queue.items.len(), 2);
                if matches!(scenario, FollowupScenario::Steering) {
                    let id = view.queue.items[0].id.clone();
                    view.deliver_queued(id, true, cx);
                } else {
                    view.stop(cx);
                }
            });
            if matches!(
                scenario,
                FollowupScenario::StopDuringRetry | FollowupScenario::StopRunning
            ) {
                for _ in 0..if live { 6000 } else { 500 } {
                    cx.run_until_parked();
                    if view.read_with(cx, |view, _| !view.is_running()) {
                        break;
                    }
                    runtime.block_on(async {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await
                    });
                }
                for _ in 0..100 {
                    cx.run_until_parked();
                    runtime.block_on(async {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await
                    });
                }
                view.update(cx, |view, cx| {
                    assert_eq!(
                        view.queue.items.len(),
                        2,
                        "Stop must cancel the pending retry"
                    );
                    assert_eq!(
                        view.completed_turns,
                        1,
                        "running={}, errors={:?}",
                        view.is_running(),
                        view.pending_errors
                    );
                    if matches!(scenario, FollowupScenario::StopRunning) {
                        assert_eq!(
                            view.model.last_status,
                            Some(del_agent::TurnStatus::Interrupted)
                        );
                    }
                    let id = view.queue.items[0].id.clone();
                    view.deliver_queued(id, false, cx);
                });
            }
        }
        for _ in 0..if live { 12000 } else { 500 } {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                (!view.pending_errors.is_empty())
                    || (view.queue.items.is_empty() && !view.is_running())
            }) {
                break;
            }
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.queue.items.is_empty(),
                "remaining={:?}, running={}, completed={}, errors={:?}",
                view.queue
                    .items
                    .iter()
                    .map(|item| (&item.input.text, item.sending, &item.error))
                    .collect::<Vec<_>>(),
                view.is_running(),
                view.completed_turns,
                view.pending_errors
            );
            assert!(view.pending_errors.is_empty(), "{:?}", view.pending_errors);
            assert_eq!(
                view.model.last_status,
                Some(del_agent::TurnStatus::Completed)
            );
            if matches!(scenario, FollowupScenario::Steering) {
                assert!((2..=3).contains(&view.completed_turns));
            } else {
                assert_eq!(view.completed_turns, 3);
            }
            let users = view
                .model
                .blocks
                .iter()
                .filter_map(|block| {
                    if let Block::User { text, .. } = block {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(users, prompts);
        });
        runtime.block_on(manager.shutdown());
    }
    #[gpui::test]
    fn stop_before_provider_start_is_not_lost(cx: &mut gpui::TestAppContext) {
        use std::{sync::Arc, time::Duration};
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            [Arc::new(
                del_agent::MockBackend::new(del_agent::Provider::Claude)
                    .with_delay(Duration::from_secs(1)),
            ) as Arc<dyn del_agent::AgentBackend>],
        )
        .unwrap();
        let session = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider: del_agent::Provider::Claude,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: Default::default(),
            }))
            .unwrap();
        let mut events = session.subscribe();
        let view = cx.new(|cx| {
            ChatView::new(
                Backend {
                    runtime: runtime.handle().clone(),
                    manager: manager.clone(),
                },
                None,
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.session = Some(session.id.clone());
            view.stop(cx);
        });
        runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
        });
        runtime
            .block_on(manager.send_prompt(&session.id, UserInput::text("slow response")))
            .unwrap();
        loop {
            let event = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(1), events.recv())
                    .await
                    .unwrap()
                    .unwrap()
            });
            let started = matches!(event, AgentEvent::TurnStarted { .. });
            view.update(cx, |view, cx| view.on_event(&event, cx));
            if started {
                break;
            }
        }
        let interrupted = runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), async {
                loop {
                    if let AgentEvent::TurnCompleted { status, .. } = events.recv().await.unwrap() {
                        return status == del_agent::TurnStatus::Interrupted;
                    }
                }
            })
            .await
            .unwrap_or(false)
        });
        runtime.block_on(manager.shutdown());
        assert!(
            interrupted,
            "stop request was lost before the provider began its turn"
        );
    }

    #[gpui::test]
    fn first_prompt_replay_shows_exactly_one_user_row(cx: &mut gpui::TestAppContext) {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            [
                Arc::new(del_agent::MockBackend::new(del_agent::Provider::Codex))
                    as Arc<dyn del_agent::AgentBackend>,
            ],
        )
        .unwrap();
        let input = UserInput::text("Build a chess board");
        let session = runtime.block_on(async {
            let h = manager
                .start_session(del_agent::SessionConfig {
                    cwd: dir.path().into(),
                    provider: del_agent::Provider::Codex,
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    permission_mode: Default::default(),
                })
                .await
                .unwrap();
            manager.send_prompt(&h.id, input.clone()).await.unwrap();
            h
        });
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager: manager.clone(),
        };
        let view = cx.new(|cx| ChatView::new(backend, Some(session.id), Some(input), cx));
        for _ in 0..100 {
            cx.run_until_parked();
            if !view.read_with(cx, |view, _| view.loading) {
                break;
            }
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        view.read_with(cx, |view, _| {
            assert!(!view.loading);
            assert_eq!(
                view.model
                    .blocks
                    .iter()
                    .filter(|b| matches!(b, Block::User { .. }))
                    .count(),
                1
            );
        });
        runtime.block_on(manager.shutdown());
    }

    #[gpui::test]
    fn busy_followup_waits_for_the_running_turn_without_showing_an_error(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            [Arc::new(
                del_agent::MockBackend::new(del_agent::Provider::Codex)
                    .with_delay(std::time::Duration::from_millis(3)),
            ) as Arc<dyn del_agent::AgentBackend>],
        )
        .unwrap();
        let session = runtime
            .block_on(manager.start_session(del_agent::SessionConfig {
                cwd: dir.path().into(),
                provider: del_agent::Provider::Codex,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: Default::default(),
            }))
            .unwrap();
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager: manager.clone(),
        };
        let view = cx.new(|cx| ChatView::new(backend, None, None, cx));
        view.update(cx, |view, cx| {
            view.session = Some(session.id.clone());
            view.loading = true;
            view.follow(session.id.clone(), cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if !view.read_with(cx, |view, _| view.loading) {
                break;
            }
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        runtime
            .block_on(manager.send_prompt(&session.id, UserInput::text("already running")))
            .unwrap();
        // The backend has accepted a turn before its TurnStarted reaches the view.
        view.update(cx, |view, cx| {
            view.submit_input(UserInput::text("follow-up"), false, cx)
        });
        for _ in 0..10 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        view.read_with(cx, |view, _| {
            assert!(
                view.queue.items.iter().all(|item| item.error.is_none()),
                "busy is a waiting state, not a failed message"
            );
            assert!(
                view.is_running(),
                "a busy response must not reset a running turn to Done"
            );
        });
        for _ in 0..500 {
            cx.run_until_parked();
            if view.read_with(cx, |view, _| {
                view.queue.items.is_empty() && !view.is_running()
            }) {
                break;
            }
            runtime.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            });
        }
        view.read_with(cx, |view, _| {
            assert!(view.queue.items.is_empty());
            let users = view
                .model
                .blocks
                .iter()
                .filter_map(|block| {
                    if let Block::User { text, .. } = block {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(users, ["follow-up"]);
        });
        runtime.block_on(manager.shutdown());
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::conversation_search as search;
    use ::gpui;
    use core::prelude::v1::test;
    use std::sync::Arc;

    fn backend() -> (tempfile::TempDir, tokio::runtime::Runtime, Backend) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let manager = SessionManager::new(
            dir.path().join("sessions.jsonl"),
            std::iter::empty::<Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager,
        };
        (dir, runtime, backend)
    }
    fn hit(messages: Vec<search::Message>, query: &str, n: usize) -> search::Hit {
        let doc = Arc::new(search::Document {
            source: search::Source {
                journal: None,
                account: None,
                tab: crate::state::Tab::Session(SessionId::new("saved")),
                title: "Task".into(),
                provider: Some(del_agent::Provider::Codex),
                project_name: "tmp".into(),
                cwd: "/tmp".into(),
                conversation_id: None,
                archived: false,
                live: None,
            },
            folded: Default::default(),
            messages: Arc::new(messages),
        });
        search::search(&[doc], query).remove(n)
    }
    struct Fixture(Entity<ChatView>);
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(14.));
            div().w(px(650.)).h(px(350.)).child(self.0.clone())
        }
    }
    #[gpui::test]
    fn message_jump_settles_in_one_frame_with_reduce_motion(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_dir, _runtime, backend) = backend();
        let messages = (0..12)
            .map(|i| search::Message {
                role: "You",
                text: format!("Message {i}"),
            })
            .collect::<Vec<_>>();
        let (root, cx) = cx.add_window_view(|_, cx| {
            Fixture(cx.new(|cx| ChatView::saved_transcript(backend, search::model(&messages), cx)))
        });
        cx.run_until_parked();
        let chat = root.read_with(cx, |root, _| root.0.clone());
        chat.update(cx, |chat, cx| {
            chat.jump = Some(MessageJump {
                target: JumpTarget::Row(0),
                inset: 0.,
                from: -400.,
                started: std::time::Instant::now(),
            });
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        chat.read_with(cx, |chat, _| {
            assert!(chat.jump.is_none());
            assert_eq!(chat.scroller.logical_scroll_top().item_ix, 0);
            assert_eq!(chat.scroller.logical_scroll_top().offset_in_item, px(0.));
        });
    }
    #[gpui::test]
    fn search_reveals_deep_markdown_match_and_clears_highlight(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let (_dir, _runtime, backend) = backend();
        let text = (0..100)
            .map(|i| format!("Paragraph {i}: some text.\n\n"))
            .collect::<String>()
            + "**uniqueneedle** at the end.";
        let messages = vec![search::Message {
            role: "Assistant",
            text,
        }];
        let target = hit(messages.clone(), "uniqueneedle", 0);
        let (root, cx) = cx.add_window_view(|_, cx| {
            let chat =
                cx.new(|cx| ChatView::saved_transcript(backend, search::model(&messages), cx));
            chat.update(cx, |chat, cx| chat.reveal_search_hit(target, cx));
            Fixture(chat)
        });
        for _ in 0..8 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            // The eased jump settles first, then the match is revealed inside the row.
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(60));
        }
        let chat = root.read_with(cx, |root, _| root.0.clone());
        chat.read_with(cx, |chat, _| {
            assert!(chat.search_flash.as_ref().unwrap().applied);
            assert_eq!(chat.scroller.logical_scroll_top().item_ix, 0);
            assert!(
                chat.scroller.logical_scroll_top().offset_in_item > px(1000.),
                "{:?}",
                chat.scroller.logical_scroll_top()
            );
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1500));
        cx.run_until_parked();
        chat.read_with(cx, |chat, _| assert!(chat.search_flash.is_none()));
    }
    #[gpui::test]
    fn search_waits_for_hydration_and_resolves_repeated_messages(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let (_dir, _runtime, backend) = backend();
        let messages = vec![
            search::Message {
                role: "You",
                text: "same message".into(),
            },
            search::Message {
                role: "Assistant",
                text: "answer".into(),
            },
            search::Message {
                role: "You",
                text: "same message".into(),
            },
        ];
        let target = hit(messages.clone(), "same", 1);
        let chat = cx.new(|cx| ChatView::new(backend, None, None, cx));
        chat.update(cx, |chat, cx| {
            chat.loading = true;
            chat.reveal_search_hit(target, cx);
            assert!(chat.search_pending.is_some());
            assert!(chat.search_flash.is_none());
            chat.model = search::model(&messages);
            chat.scroller.reset(chat.model.blocks.len());
            chat.loading = false;
            chat.apply_search_jump(cx);
            assert_eq!(chat.search_flash.as_ref().unwrap().ix, 2);
            assert!(chat.search_pending.is_none());
        });
    }
}
