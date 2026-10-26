//! `ChatView`: session plumbing and the message list. The composer is shared
//! with the new-chat page and lives in the app shell.

use super::model::{Block, Change, ChatModel, Phase, compact_count};
use crate::appearance::rgba;
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

struct MessageJump {
    target: usize,
    from: f32,
    started: std::time::Instant,
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

fn escaped_search_text(
    text: &str,
    range: std::ops::Range<usize>,
) -> (String, std::ops::Range<usize>) {
    let mut out = String::new();
    let mut start = 0;
    let mut end = 0;
    for (ix, ch) in text.char_indices() {
        if ix == range.start {
            start = out.len();
        }
        if ch.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(ch);
        if ix + ch.len_utf8() == range.end {
            end = out.len();
        }
    }
    (out, start..end)
}

/// Combine only adjacent, already-received chunks for the same turn. Never wait
/// for a newline, timer, or a later event, and never cross tool/status boundaries.
fn coalesce_deltas(events: impl IntoIterator<Item = AgentEvent>) -> Vec<AgentEvent> {
    let mut ready = Vec::new();
    for event in events {
        if let AgentEvent::AssistantTextDelta { turn_id, text } = &event
            && let Some(AgentEvent::AssistantTextDelta { turn_id: previous, text: accumulated }) = ready.last_mut()
            && previous == turn_id
        {
            accumulated.push_str(text);
        } else {
            ready.push(event);
        }
    }
    ready
}

pub struct PromptSubmitted;
pub struct OpenLink(pub String);

pub struct ChatView {
    pub(super) model: ChatModel,
    pub(super) scroller: ListState,
    jump: Option<MessageJump>,
    rail_hover: Option<usize>,
    rail_previous_hover: Option<usize>,
    backend: Backend,
    pub queue: crate::followups::Queue,
    session: Option<SessionId>,
    /// The session's event stream ended (session closed or app quitting).
    closed: bool,
    loading: bool,
    first_prompt: Option<String>,
    completed_turns: u64,
    working_since: Option<std::time::Instant>,
    working_message: Option<usize>,
    pub(super) response_hover: std::collections::HashMap<usize, bool>,
    pub(super) response_times: std::collections::HashMap<usize, chrono::DateTime<chrono::Local>>,
    pub(super) expanded_activity: std::collections::HashSet<usize>,
    pub(super) assistant_text: std::collections::HashMap<usize, Entity<text::TextViewState>>,
    pub pending_errors: Vec<String>,
    pub(super) search_pending: Option<crate::conversation_search::Hit>,
    pub(super) search_flash: Option<SearchFlash>,
}

impl EventEmitter<PromptSubmitted> for ChatView {}
impl EventEmitter<OpenLink> for ChatView {}

impl ChatView {
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
                let _ = view.update(cx, |this, cx| {
                    this.jump = None;
                    cx.notify();
                });
            });
        });
        let mut view = Self {
            queue: session
                .as_ref()
                .map(crate::followups::Queue::load)
                .unwrap_or_default(),
            model: ChatModel::default(),
            scroller,
            jump: None,
            rail_hover: None,
            rail_previous_hover: None,
            backend,
            session: session.clone(),
            closed: false,
            loading: session.is_some(),
            first_prompt: first_prompt.as_ref().map(|input| input.text.clone()),
            completed_turns: 0,
            working_since: None,
            working_message: None,
            response_times: Default::default(),
            response_hover: Default::default(),
            expanded_activity: Default::default(),
            assistant_text: Default::default(),
            pending_errors: Vec::new(),
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
        view
    }

    pub fn search_messages(&self) -> Vec<crate::conversation_search::Message> {
        crate::conversation_search::messages(&self.model)
    }

    #[cfg(test)]
    pub fn saved_transcript(backend: Backend, model: ChatModel, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(backend, None, None, cx);
        view.scroller.reset(model.blocks.len());
        view.model = model;
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
        let Some(message_ix) = hit.message else {
            return;
        };
        let message = &hit.document.messages[message_ix];
        let occurrence = hit.document.messages[..message_ix]
            .iter()
            .filter(|m| *m == message)
            .count();
        let found = self
            .model
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(ix, block)| {
                let (role, text) = match block {
                    Block::User { text, .. } => ("You", text),
                    Block::Assistant { text, .. } => ("Assistant", text),
                    _ => return Vec::new(),
                };
                if role != message.role {
                    return Vec::new();
                }
                text.match_indices(&message.text)
                    .map(move |(offset, _)| (ix, offset, text.clone()))
                    .collect::<Vec<_>>()
            })
            .nth(occurrence);
        let Some((ix, offset, text)) = found else {
            self.pending_errors.push(
                "That message changed since the search. Search again to find its current location."
                    .into(),
            );
            return;
        };
        let source_range = offset + hit.range.start..offset + hit.range.end;
        let (source, range) = if message.role == "You" {
            escaped_search_text(&text, source_range)
        } else {
            (text, source_range)
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
        self.scroller.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: zpx(0.),
        });
    }

    fn on_event(&mut self, event: &AgentEvent, cx: &mut Context<Self>) {
        if let AgentEvent::TurnCompleted { turn_id, .. } = event {
            self.completed_turns += 1;
            if !self.loading
                && let Some(ix) = self.model.blocks.iter().rposition(|block|
                    matches!(block, Block::Assistant { turn_id: id, .. } if id == turn_id))
            {
                self.response_times.insert(ix, chrono::Local::now());
                self.scroller.remeasure_items(ix..ix + 1);
            }
        }
        if !self.loading
            && let AgentEvent::Error { message, .. } = event
        {
            self.pending_errors.push(message.clone());
        }
        let change = self.model.apply(event);
        let delta = match event { AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()), _ => None };
        self.sync_text(change, delta, cx);
    }

    pub(super) fn working_label(&self, ix: usize) -> Option<String> {
        if self.working_message != Some(ix) { return None; }
        let seconds = self.working_since?.elapsed().as_secs();
        let elapsed = if seconds >= 60 { format!("{}m", seconds / 60) } else { format!("{seconds}s") };
        Some(format!("Working for {elapsed}"))
    }

    /// Keep the virtual list's rows in step with the model.
    fn sync(&mut self, change: Option<Change>, cx: &mut Context<Self>) {
        self.sync_text(change, None, cx);
    }

    fn sync_text(&mut self, change: Option<Change>, delta: Option<&str>, cx: &mut Context<Self>) {
        if self.model.is_running() && self.working_since.is_none() && !self.loading {
            let started = std::time::Instant::now();
            self.working_since = Some(started);
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                    if !this.update(cx, |this, cx| {
                        let active = this.working_since == Some(started) && this.model.is_running();
                        if active {
                            if let Some(ix) = this.working_message { this.scroller.remeasure_items(ix..ix + 1); }
                            cx.notify();
                        }
                        active
                    }).unwrap_or(false) { break; }
                }
            }).detach();
        } else if !self.model.is_running() {
            self.working_since = None;
        }
        let previous_working_message = self.working_message;
        self.working_message = self.working_since.and_then(|_| self.model.blocks.iter().rposition(|block| matches!(block, Block::User { .. })));
        if let Some(change) = change {
            let ix = match change { Change::Pushed(ix) | Change::Updated(ix) => ix };
            if let Some(Block::Assistant { text, .. }) = self.model.blocks.get(ix) {
                if let Some(view) = self.assistant_text.get(&ix) {
                    view.update(cx, |view, cx| {
                        if let Some(delta) = delta { view.push_str(delta, cx); }
                        else { view.set_text(text, cx); }
                    });
                } else {
                    let view = cx.new(|cx| text::TextViewState::markdown(text, cx));
                    cx.observe(&view, move |this, _, cx| {
                        this.scroller.remeasure_items(ix..ix + 1);
                        cx.notify();
                    }).detach();
                    self.assistant_text.insert(ix, view);
                }
            }
            match change {
                Change::Pushed(_) => {
                    let count = self.scroller.item_count();
                    self.scroller.splice(count..count, 1);
                }
                Change::Updated(ix) => self.scroller.remeasure_items(ix..ix + 1),
            }
            let mut start = ix;
            while start > 0 && matches!(self.model.blocks[start - 1], Block::Tool(_)) { start -= 1; }
            let mut end = ix + 1;
            while end < self.model.blocks.len() && matches!(self.model.blocks[end], Block::Tool(_)) { end += 1; }
            self.scroller.remeasure_items(start..end);
        } else if !self.model.is_running() {
            // A successful final tool sequence can collapse when the turn settles.
            let end = self.model.blocks.len();
            let mut start = end;
            while start > 0 && matches!(self.model.blocks[start - 1], Block::Tool(_)) { start -= 1; }
            if start < end { self.scroller.remeasure_items(start..end); }
        }
        for ix in [previous_working_message, self.working_message].into_iter().flatten() {
            if ix < self.scroller.item_count() { self.scroller.remeasure_items(ix..ix + 1); }
        }
        cx.notify();
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

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn save_queue(&self) {
        let _ = self.queue.save();
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
        let id = self.enqueue(input, cx);
        if !self.is_running() || steer {
            self.deliver_queued(id, steer && self.is_running(), cx);
        }
    }

    fn send_next(&mut self, cx: &mut Context<Self>) {
        if !self.is_running()
            && let Some(id) = self.queue.next()
        {
            self.deliver_queued(id, false, cx);
        }
    }

    pub fn deliver_queued(&mut self, item_id: String, steer: bool, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(input) = self.queue.begin(&item_id) else {
            return;
        };
        cx.emit(PromptSubmitted);
        self.save_queue();
        let backend = self.backend.clone();
        let displayed_input = input.clone();
        let row_index = self.model.blocks.len();
        let completed_turns = self.completed_turns;
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
                if busy {
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
                    // Backend events may already have arrived. Add the user row
                    // before dispatch below, rather than changing phase on ack.
                    if !this.model.is_running()
                        && this.model.last_status == Some(del_agent::TurnStatus::Completed)
                    {
                        this.send_next(cx);
                    }
                } else {
                    if matches!(this.model.blocks.get(row_index), Some(Block::User { .. })) {
                        this.model.blocks.remove(row_index);
                        this.scroller.splice(row_index..row_index + 1, 0);
                    }
                    if busy {
                        // A provider may already be running a steered follow-up
                        // before its next TurnStarted reaches this view.
                        if this.completed_turns > completed_turns && !this.is_running() {
                            if this.model.last_status == Some(del_agent::TurnStatus::Completed) {
                                this.send_next(cx);
                            }
                        } else {
                            this.model.phase = Phase::Running;
                        }
                    } else if !steer {
                        this.model.phase = Phase::Idle;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        let change = self.model.push_input(&displayed_input, steer);
        self.sync(Some(change), cx);
        self.scroller.scroll_to_end();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
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
        let change = self.model.resolve_approval(&id, response);
        self.sync(change, cx);
        let Some(session) = self.session.clone() else {
            return;
        };
        let manager = self.backend.manager.clone();
        self.run(cx, async move {
            manager.respond_approval(&session, &id, response).await
        });
    }

    pub(super) fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let change = self.model.toggle(ix);
        self.sync(change, cx);
    }

    /// Reattach the session if it is not live, replay its history into the
    /// model, then pump new events until the stream closes.
    fn follow(&mut self, id: SessionId, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let attached = self.backend.runtime.spawn(async move {
            backend.resume(&id).await?;
            backend.manager.subscribe_with_history(&id)
        });
        cx.spawn(async move |this, cx| {
            let (history, mut rx) = match attached.await {
                Ok(Ok(attached)) => attached,
                Ok(Err(err)) => return report(&this, cx, err.to_string()),
                Err(err) => return report(&this, cx, err.to_string()),
            };
            let replayed = this.update(cx, |this, cx| {
                // The initial row already includes attachment/goal metadata.
                // Skip its plain-text history counterpart exactly once.
                let first_prompt = this.first_prompt.take();
                let mut opening_user = true;
                for event in &history {
                    if let AgentEvent::UserMessage { text } = event
                        && opening_user
                    {
                        opening_user = false;
                        if first_prompt.as_deref() == Some(text.as_str()) {
                            continue;
                        }
                    }
                    this.on_event(event, cx);
                }
                this.scroller.set_follow_mode(FollowMode::Tail);
                this.scroller.scroll_to_end();
                this.loading = false;
                this.sync(None, cx);
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
                                    if let Ok(event) = rx.try_recv() { ready.push(event); }
                                }
                                for event in coalesce_deltas(ready) { this.on_event(&event, cx); }
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
                    Err(RecvError::Lagged(_)) => continue,
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

    fn render_status(&self) -> AnyElement {
        let muted = rgba(0xffffff8c);
        let waiting = self.model.has_pending_approval();
        let label = match (self.model.phase, self.model.last_status) {
            (Phase::Running, _) if waiting => "Waiting for your approval",
            (Phase::Running, _) => "",
            (Phase::Idle, Some(del_agent::TurnStatus::Completed)) => "",
            (Phase::Idle, Some(del_agent::TurnStatus::Interrupted)) => "Stopped",
            (Phase::Idle, Some(del_agent::TurnStatus::Failed)) => "Turn failed",
            (Phase::Idle, None) => "",
        };
        let usage = self.model.usage();
        let usage_text = (usage.input + usage.output > 0).then(|| {
            format!(
                "{} in · {} out · {} cached",
                compact_count(usage.input),
                compact_count(usage.output),
                compact_count(usage.cached)
            )
        });

        if label.is_empty() && usage_text.is_none() {
            return div().into_any_element();
        }

        h_flex()
            .id("status")
            .w_full()
            .max_w(zpx(COLUMN_MAX_WIDTH))
            .h(zpx(20.0))
            .px(if self.model.goal.is_some() {
                zpx(crate::goal_ui::GOAL_INSET + 10.)
            } else {
                zpx(8.)
            })
            .items_center()
            .justify_between()
            .text_size(zpx(TEXT_SM))
            .text_color(muted)
            .child(label)
            .children(usage_text)
            .into_any_element()
    }
}

/// Report a backend failure as a toast (and stop the running phase).
fn report(this: &WeakEntity<ChatView>, cx: &mut AsyncApp, message: String) {
    let _ = this.update(cx, |this, cx| {
        this.loading = false;
        this.pending_errors.push(message.clone());
        let change = this.model.push_error(message, false);
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
        if let Some(jump) = &self.jump {
            let t = (jump.started.elapsed().as_secs_f32() / 0.22).min(1.0);
            self.scroller.scroll_to(ListOffset {
                item_ix: jump.target,
                offset_in_item: zpx(0.0),
            });
            if t < 1.0 {
                let target = self
                    .scroller
                    .scroll_px_offset_for_scrollbar()
                    .y
                    .as_f32()
                    .max(-self.scroller.max_offset_for_scrollbar().y.as_f32());
                let eased = 1.0 - (1.0 - t).powi(3);
                self.scroller.set_offset_from_scrollbar(point(
                    zpx(0.0),
                    gpui::px(jump.from + (target - jump.from) * eased),
                ));
                window.request_animation_frame();
            } else {
                self.jump = None;
            }
        }
        let top = self.scroller.logical_scroll_top().item_ix;
        let prompts: Vec<_> = self
            .model
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(ix, block)| {
                if let Block::User { text, .. } = block {
                    Some((ix, text.clone()))
                } else {
                    None
                }
            })
            .collect();
        let at_end = self.scroller.is_following_tail()
            || self.scroller.is_scrolled_to_end() == Some(true)
            || self.scroller.max_offset_for_scrollbar().y <= zpx(0.0);
        let active = self.jump.as_ref().map(|jump| jump.target).or_else(|| {
            active_prompt(
                &prompts.iter().map(|(ix, _)| *ix).collect::<Vec<_>>(),
                top,
                at_end,
            )
        });
        let hover = self.rail_hover;
        let previous_hover = self.rail_previous_hover;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("message-navigation")
                            .w(zpx(24.0))
                            .h_full()
                            .flex_none()
                            .justify_center()
                            .py_3()
                            .children((prompts.len() >= 6).then(|| {
                                v_flex()
                                    .id("message-ticks")
                                    .max_h_full()
                                    .overflow_y_scroll()
                                    .children(prompts.into_iter().enumerate().map(
                                        |(position, (ix, text))| {
                                            let strength = |hover: Option<usize>| {
                                                hover.map_or(0.0, |hover| {
                                                    (1.0 - position.abs_diff(hover) as f32 / 4.0)
                                                        .max(0.0)
                                                })
                                            };
                                            let previous = strength(previous_hover);
                                            let next = strength(hover);
                                            let selected = hover.map_or(active == Some(ix), |hover| hover == position);
                                            div()
                                                .id(("message-jump", ix))
                                                .role(gpui::Role::Button)
                                                .aria_label(format!(
                                                    "Jump to message: {}",
                                                    text.chars().take(80).collect::<String>()
                                                ))
                                                .tooltip(move |window, cx| {
                                                    gpui_kit::component::tooltip::Tooltip::new(
                                                        text.chars().take(180).collect::<String>(),
                                                    )
                                                    .build(window, cx)
                                                })
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
                                                            this.rail_previous_hover =
                                                                this.rail_hover;
                                                            this.rail_hover = next;
                                                            if next.is_some() { crate::haptics::tick(); }
                                                            cx.notify();
                                                        }
                                                    },
                                                ))
                                                .group("message-tick")
                                                .w_full()
                                                .h(zpx(12.0))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_start()
                                                .pl(zpx(2.0))
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.jump = Some(MessageJump {
                                                        target: ix,
                                                        from: this
                                                            .scroller
                                                            .scroll_px_offset_for_scrollbar()
                                                            .y
                                                            .as_f32(),
                                                        started: std::time::Instant::now(),
                                                    });
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div().h(zpx(2.0)).with_animation(
                                                        SharedString::from(format!(
                                                            "rail-{ix}-{previous_hover:?}-{hover:?}"
                                                        )),
                                                        Animation::new(
                                                            std::time::Duration::from_millis(100),
                                                        )
                                                        .with_easing(ease_out_quint()),
                                                        move |tick, t| {
                                                            let weight =
                                                                previous + (next - previous) * t;
                                                            let width = 7.0 + 15.0 * weight;
                                                            // Only the hovered marker (or current message when idle)
                                                            // is bright. Neighbor magnification changes width only.
                                                            let alpha = if selected { 0.9 } else { 0.22 };
                                                            tick.w(zpx(width))
                                                                .bg(rgba(0xffffffff).opacity(alpha))
                                                        },
                                                    ),
                                                )
                                        },
                                    ))
                            })),
                    )
                    .child(
                        v_flex().flex_1().min_w_0().h_full()
                        .child(
                            list(self.scroller.clone(), move |ix, _, cx| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .px_3()
                                    .child(view.update(cx, |this, cx| this.render_row(ix, cx)))
                                    .into_any_element()
                            })
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .py_2(),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .items_center()
                    .px_3()
                    .child(self.render_status()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::gpui;
    use core::prelude::v1::test;

    #[::core::prelude::v1::test]
    fn message_rail_highlights_last_prompt_at_bottom_even_when_earlier_prompt_is_at_top() {
        assert_eq!(active_prompt(&[0, 3, 6, 9], 3, true), Some(9));
        assert_eq!(active_prompt(&[0, 3, 6, 9], 3, false), Some(3));
        assert_eq!(active_prompt(&[0, 3, 6, 9], 8, false), Some(6));
        assert_eq!(active_prompt(&[], 0, true), None);
    }
    #[test]
    fn ready_deltas_coalesce_without_crossing_turns_or_tools() {
        let delta = |turn: &str, text: &str| AgentEvent::AssistantTextDelta { turn_id: turn.into(), text: text.into() };
        let events = coalesce_deltas(vec![delta("a", "hello "), delta("a", "世界"),
            AgentEvent::ToolCallStarted { id: "tool".into(), name: "Read".into(), input_summary: "main.rs".into() },
            delta("a", "tail"), delta("b", "next")]);
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], AgentEvent::AssistantTextDelta { text, .. } if text == "hello 世界"));
        assert!(matches!(&events[1], AgentEvent::ToolCallStarted { .. }));
        assert!(matches!(&events[2], AgentEvent::AssistantTextDelta { turn_id, text } if turn_id == "a" && text == "tail"));
        assert!(matches!(&events[3], AgentEvent::AssistantTextDelta { turn_id, text } if turn_id == "b" && text == "next"));
        assert_eq!(coalesce_deltas(vec![delta("a", "partial")]).len(), 1);
    }

    #[gpui::test]
    fn working_label_follows_latest_user_message_and_clears_on_completion(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = SessionManager::new(dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>()).unwrap();
        let view = cx.new(|cx| ChatView::new(Backend { runtime: runtime.handle().clone(), manager }, None, None, cx));
        view.update(cx, |view, cx| {
            let change = view.model.push_user("first".into(), false);
            view.sync(Some(change), cx);
            assert_eq!(view.working_label(0).as_deref(), Some("Working for 0s"));
            let change = view.model.push_user("followup".into(), true);
            view.sync(Some(change), cx);
            assert!(view.working_label(0).is_none());
            assert!(view.working_label(1).is_some());
            view.model.phase = Phase::Idle;
            view.sync(None, cx);
            assert!(view.working_label(1).is_none());
        });
    }

    #[gpui::test]
    fn streamed_chunks_update_markdown_before_turn_completion(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = SessionManager::new(dir.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>()).unwrap();
        let view = cx.new(|cx| ChatView::new(Backend {
            runtime: runtime.handle().clone(), manager,
        }, None, None, cx));
        for (chunk, expected) in [("First", "First"), (" second", "First second"), (" third", "First second third")] {
            view.update(cx, |view, cx| view.on_event(&AgentEvent::AssistantTextDelta {
                turn_id: "streaming".into(), text: chunk.into(),
            }, cx));
            cx.run_until_parked();
            view.read_with(cx, |view, cx| {
                let text = view.assistant_text.values().next().unwrap();
                assert_eq!(text.read(cx).rendered_text().as_str().trim(), expected);
                assert!(view.model.last_status.is_none());
            });
        }
        // A provider's final snapshot may revise already streamed Markdown.
        view.update(cx, |view, cx| view.on_event(&AgentEvent::AssistantMessage {
            turn_id: "streaming".into(), text: "**Corrected** 世界".into(),
        }, cx));
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let text = view.assistant_text.values().next().unwrap();
            assert_eq!(text.read(cx).rendered_text().as_str().trim(), "Corrected 世界");
        });
    }

    #[gpui::test]
    fn followups_wait_for_completion_then_send_in_order(cx: &mut gpui::TestAppContext) {
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
        view.update(cx, |view, cx| {
            view.submit_input(UserInput::text("first"), false, cx);
            view.submit_input(UserInput::text("second"), false, cx);
            view.submit_input(UserInput::text("third"), false, cx);
            assert_eq!(view.queue.items.len(), 3);
            assert_eq!(
                view.queue.items.iter().filter(|item| item.sending).count(),
                1
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
            assert_eq!(users, ["first", "second", "third"]);
        });
        runtime.block_on(manager.shutdown());
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
                tab: crate::state::Tab::Session(SessionId::new("saved")),
                title: "Task".into(),
                provider: Some(del_agent::Provider::Codex),
                cwd: "/tmp".into(),
                conversation_id: None,
                archived: false,
                live: None,
            },
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
