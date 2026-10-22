//! `ChatView`: session plumbing, the message list and the composer.

use super::model::{Change, ChatModel, Phase, compact_count};
use crate::composer_ui::ComposerCard;
use del_agent::{
    AgentError, AgentEvent, ApprovalResponse, SessionConfig, SessionId, SessionManager, UserInput,
};
use del_core::composer;
use del_core::typography::TEXT_SM;
use gpui_kit::component::input::{InputEvent, TextareaState};
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
    pub config: SessionConfig,
}

pub struct ChatView {
    pub(super) model: ChatModel,
    pub(super) scroller: Entity<MessageScrollerState>,
    pub(super) composer: Entity<TextareaState>,
    backend: Backend,
    session: Option<SessionId>,
    /// A session is being started; prompts wait until it is ready.
    starting: bool,
}

impl ChatView {
    pub fn new(backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Reply, or steer the turn...")
                .auto_grow(1, 8)
                .submit_on_enter(true);
            state.set_submit_on_enter(true, cx);
            state
        });
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )
        .detach();
        composer.update(cx, |state, cx| state.focus(window, cx));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        Self {
            model: ChatModel::default(),
            scroller,
            composer,
            backend,
            session: None,
            starting: false,
        }
    }

    /// Send the first prompt of a conversation (from the new-chat page).
    pub fn send_first(&mut self, text: String, cx: &mut Context<Self>) {
        self.send(text, cx);
    }

    /// Fold a canned event list into the transcript, for demos and screenshots.
    pub fn preload(&mut self, events: &[AgentEvent], cx: &mut Context<Self>) {
        for event in events {
            self.on_event(event, cx);
        }
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
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

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = composer::submission(&self.composer.read(cx).value()) else {
            return;
        };
        if self.starting {
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
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

        let backend = self.backend.clone();
        match self.session.clone() {
            Some(id) => self.run(cx, async move {
                backend
                    .manager
                    .send_prompt(&id, UserInput::text(text))
                    .await
            }),
            None => self.start_session(text, cx),
        }
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

    fn stop(&mut self, cx: &mut Context<Self>) {
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

    /// Start a session, subscribe before the first prompt so no event is
    /// missed, then pump events into the model until the stream closes.
    fn start_session(&mut self, first_prompt: String, cx: &mut Context<Self>) {
        self.starting = true;
        let backend = self.backend.clone();
        let started = self.backend.runtime.spawn(async move {
            let handle = backend
                .manager
                .start_session(backend.config.clone())
                .await?;
            let rx = backend.manager.subscribe(&handle.id)?;
            backend
                .manager
                .send_prompt(&handle.id, UserInput::text(first_prompt))
                .await?;
            Ok::<_, AgentError>((handle.id, rx))
        });
        cx.spawn(async move |this, cx| {
            let (id, mut rx) = match started.await {
                Ok(Ok(started)) => started,
                Ok(Err(err)) => return report(&this, cx, err.to_string()),
                Err(err) => return report(&this, cx, err.to_string()),
            };
            let _ = this.update(cx, |this, _| {
                this.session = Some(id);
                this.starting = false;
            });
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
        })
        .detach();
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
        this.starting = false;
        let change = this.model.push_error(message, false);
        this.sync(Some(change), cx);
    });
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let running = self.model.is_running();

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
                    .jump_button(true),
                ),
            )
            .child(
                v_flex()
                    .w_full()
                    .items_center()
                    .px_3()
                    .pb_3()
                    .child(self.render_status())
                    .child({
                        let card = ComposerCard::new(
                            &self.composer,
                            COLUMN_MAX_WIDTH,
                            cx.listener(|this, _, window, cx| this.submit(window, cx)),
                        );
                        if running {
                            card.on_stop(cx.listener(|this, _, _, cx| this.stop(cx)))
                        } else {
                            card
                        }
                    }),
            )
    }
}
