//! `ChatView`: session plumbing and the message list. The composer is shared
//! with the new-chat page and lives in the app shell.

use super::model::{Change, ChatModel, Phase, compact_count};
use del_agent::{AgentError, AgentEvent, ApprovalResponse, SessionId, SessionManager, UserInput};
use del_core::typography::TEXT_SM;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
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

pub struct ChatView {
    pub(super) model: ChatModel,
    pub(super) scroller: Entity<MessageScrollerState>,
    backend: Backend,
    session: Option<SessionId>,
    /// The session's event stream ended (session closed or app quitting).
    closed: bool,
}

impl ChatView {
    /// A chat for `session`, which the app has already started. The view
    /// replays what the session emitted so far (so nothing sent before this
    /// view existed is missed), then follows it live. `first_prompt` is the
    /// text the app sent to start the session, shown as the opening user row.
    pub fn new(
        backend: Backend,
        session: Option<SessionId>,
        first_prompt: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        let mut view = Self {
            model: ChatModel::default(),
            scroller,
            backend,
            session: session.clone(),
            closed: false,
        };
        if let Some(text) = first_prompt {
            let change = view.model.push_user(text, false);
            view.sync(Some(change), cx);
        }
        if let Some(id) = session {
            view.follow(id, cx);
        }
        view
    }

    fn on_event(&mut self, event: &AgentEvent, cx: &mut Context<Self>) {
        let change = self.model.apply(event);
        self.sync(change, cx);
    }

    /// Keep the virtual list's rows in step with the model.
    fn sync(&mut self, change: Option<Change>, cx: &mut Context<Self>) {
        if let Some(change) = change {
            self.scroller.update(cx, |scroller, cx| match change {
                Change::Pushed(_) => {
                    scroller.append(1, cx);
                }
                Change::Updated(ix) => {
                    scroller.remeasure_items(ix..ix + 1, cx);
                }
            });
        }
        cx.notify();
    }

    /// Whether a turn is in flight.
    pub fn is_running(&self) -> bool {
        self.model.is_running()
    }

    /// Send text from the shared composer: steers the turn if one is running,
    /// otherwise starts a new turn.
    pub fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        if self.model.is_running() && self.session.is_some() {
            self.steer(text, cx);
        } else {
            self.send(text, cx);
        }
    }

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        let change = self.model.push_user(text.clone(), false);
        self.sync(Some(change), cx);
        // Sending is an explicit "show me the reply": follow the tail again.
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));

        let Some(id) = self.session.clone() else {
            return;
        };
        let manager = self.backend.manager.clone();
        self.run(cx, async move {
            manager.resume(&id).await?;
            manager.send_prompt(&id, UserInput::text(text)).await
        });
    }

    fn steer(&mut self, text: String, cx: &mut Context<Self>) {
        let change = self.model.push_user(text.clone(), true);
        self.sync(Some(change), cx);
        let Some(id) = self.session.clone() else {
            return;
        };
        let manager = self.backend.manager.clone();
        self.run(cx, async move { manager.steer(&id, text).await });
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
        let manager = self.backend.manager.clone();
        let attached = self.backend.runtime.spawn(async move {
            manager.resume(&id).await?;
            manager.subscribe_with_history(&id)
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
                this.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
            });
            if replayed.is_err() {
                return;
            }
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if this
                            .update(cx, |this, cx| this.on_event(&event, cx))
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

    /// Run a backend call on the tokio runtime; failures become error banners.
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
            (Phase::Running, _) => ("Working", "Enter steers the turn"),
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
            .max_w(px(COLUMN_MAX_WIDTH))
            .h(px(20.0))
            .px_2()
            .items_center()
            .justify_between()
            .text_size(px(TEXT_SM))
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

/// Show a backend failure as a banner (and stop the running phase).
fn report(this: &WeakEntity<ChatView>, cx: &mut AsyncApp, message: String) {
    let _ = this.update(cx, |this, cx| {
        let change = this.model.push_error(message, false);
        this.sync(Some(change), cx);
    });
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        v_flex()
            .size_full()
            .child(
                div().flex_1().min_h_0().child(
                    MessageScroller::new("messages", self.scroller.clone(), move |ix, _, cx| {
                        view.update(cx, |this, cx| this.render_row(ix, cx))
                    })
                    .size_full()
                    // The default 32px gap is too airy for compact cards.
                    .with_row_style(StyleRefinement::default().pb(px(6.0)))
                    .jump_button(false),
                ),
            )
            .child(
                v_flex()
                    .w_full()
                    .items_center()
                    .px_3()
                    .child(self.render_status()),
            )
    }
}
