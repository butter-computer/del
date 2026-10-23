//! `ChatView`: session plumbing and the message list. The composer is shared
//! with the new-chat page and lives in the app shell.

use super::model::{Block, Change, ChatModel, Phase, compact_count};
use crate::appearance::rgba;
use crate::zoom::px as zpx;
use del_agent::{AgentError, AgentEvent, ApprovalResponse, SessionId, SessionManager, UserInput};
use del_core::typography::TEXT_SM;
use gpui_kit::component::spinner::Spinner;
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

pub struct PromptSubmitted;

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
    completed_turns: u64,
    pub(super) first_chunks: std::collections::HashMap<String, std::time::Instant>,
    pub pending_errors: Vec<String>,
}

impl EventEmitter<PromptSubmitted> for ChatView {}

impl ChatView {
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
            completed_turns: 0,
            first_chunks: Default::default(),
            pending_errors: Vec::new(),
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

    fn on_event(&mut self, event: &AgentEvent, cx: &mut Context<Self>) {
        if matches!(event, AgentEvent::TurnCompleted { .. }) {
            self.completed_turns += 1;
        }
        if !self.loading
            && let AgentEvent::Error { message, .. } = event
        {
            self.pending_errors.push(message.clone());
        }
        if !self.loading
            && let AgentEvent::AssistantTextDelta { turn_id, text }
                | AgentEvent::AssistantMessage { turn_id, text } = event
            && !text.is_empty()
            && !self.first_chunks.contains_key(turn_id)
            && !self.model.blocks.iter().any(|block| matches!(block,
                Block::Assistant { turn_id: existing, text, .. } if existing == turn_id && !text.is_empty()))
        {
            self.first_chunks.insert(turn_id.clone(), std::time::Instant::now());
        }
        let change = self.model.apply(event);
        self.sync(change, cx);
    }

    /// Keep the virtual list's rows in step with the model.
    fn sync(&mut self, change: Option<Change>, cx: &mut Context<Self>) {
        if let Some(change) = change {
            match change {
                Change::Pushed(_) => {
                    let count = self.scroller.item_count();
                    self.scroller.splice(count..count, 1);
                }
                Change::Updated(ix) => self.scroller.remeasure_items(ix..ix + 1),
            }
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
                for event in &history {
                    this.on_event(event, cx);
                }
                this.scroller.set_follow_mode(FollowMode::Tail);
                this.scroller.scroll_to_end();
                this.loading = false;
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
                                this.on_event(&event, cx);
                                // Claude emits completion and the next queued
                                // turn's start together. Fold both before deciding
                                // whether the provider is ready for another prompt.
                                while let Ok(event) = rx.try_recv() {
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

    fn render_status(&self) -> impl IntoElement {
        let muted = rgba(0xffffff8c);
        let waiting = self.model.has_pending_approval();
        let (label, hint) = match (self.model.phase, self.model.last_status) {
            (Phase::Running, _) if waiting => ("Waiting for your approval", ""),
            (Phase::Running, _) => ("Working", "Enter queues · ⌘ + Enter steers"),
            (Phase::Idle, Some(del_agent::TurnStatus::Completed)) => ("Done", ""),
            (Phase::Idle, Some(del_agent::TurnStatus::Interrupted)) => ("Stopped", ""),
            (Phase::Idle, Some(del_agent::TurnStatus::Failed)) => ("Turn failed", ""),
            (Phase::Idle, None) => ("", ""),
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

        h_flex()
            .id("status")
            .w_full()
            .max_w(zpx(COLUMN_MAX_WIDTH))
            .h(zpx(20.0))
            .px_2()
            .items_center()
            .justify_between()
            .text_size(zpx(TEXT_SM))
            .text_color(muted)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .when(self.model.is_running() && !waiting, |row| {
                        row.child(Spinner::new().xsmall().color(muted.into()))
                    })
                    .child(label)
                    .when(!hint.is_empty(), |row| {
                        row.child(div().text_color(rgba(0xffffff4d)).child(hint))
                    }),
            )
            .children(usage_text)
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
                                            let selected = active == Some(ix);
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
                                                            let alpha = (0.22 + 0.68 * weight).max(
                                                                if selected { 0.8 } else { 0.22 },
                                                            );
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
                        div().flex_1().min_w_0().h_full().child(
                            list(self.scroller.clone(), move |ix, _, cx| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .px_3()
                                    .pb(zpx(6.0))
                                    .child(view.update(cx, |this, cx| this.render_row(ix, cx)))
                                    .into_any_element()
                            })
                            .size_full()
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
    #[gpui::test]
    fn initial_stream_fade_starts_once_and_skips_history(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let manager = SessionManager::new(
            directory.path().join("sessions.jsonl"),
            std::iter::empty::<std::sync::Arc<dyn del_agent::AgentBackend>>(),
        )
        .unwrap();
        let backend = Backend {
            runtime: runtime.handle().clone(),
            manager,
        };
        let view = cx.new(|cx| ChatView::new(backend, None, None, cx));
        view.update(cx, |view, cx| {
            view.loading = true;
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "saved".into(),
                    text: "Old response".into(),
                },
                cx,
            );
            assert!(view.first_chunks.is_empty());
            view.loading = false;
            view.on_event(
                &AgentEvent::AssistantTextDelta {
                    turn_id: "saved".into(),
                    text: " continues".into(),
                },
                cx,
            );
            assert!(
                view.first_chunks.is_empty(),
                "restored streaming text must not fade again"
            );
            view.on_event(
                &AgentEvent::AssistantTextDelta {
                    turn_id: "live".into(),
                    text: "".into(),
                },
                cx,
            );
            assert!(view.first_chunks.is_empty());
            view.on_event(
                &AgentEvent::AssistantTextDelta {
                    turn_id: "live".into(),
                    text: "First".into(),
                },
                cx,
            );
            let started = view.first_chunks["live"];
            view.on_event(
                &AgentEvent::AssistantTextDelta {
                    turn_id: "live".into(),
                    text: " later".into(),
                },
                cx,
            );
            view.on_event(
                &AgentEvent::AssistantMessage {
                    turn_id: "live".into(),
                    text: "First later".into(),
                },
                cx,
            );
            assert_eq!(view.first_chunks.len(), 1);
            assert_eq!(view.first_chunks["live"], started);
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
