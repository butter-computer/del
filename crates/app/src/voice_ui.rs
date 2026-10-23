use crate::{
    AppShell,
    dictation::{Dictation, Draft, Event, Phase, Recorder},
};
use gpui_kit::*;
use std::time::{Duration, Instant};

impl AppShell {
    pub(super) fn cancel_dictation(&mut self, cx: &mut Context<Self>) {
        if self.dictation.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn settle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.advance_dictation(true, window, cx);
    }

    fn advance_dictation(
        &mut self,
        immediate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = self.composer.read(cx).value().to_string();
        let Some(voice) = &mut self.dictation else {
            return false;
        };
        let Some(value) =
            voice
                .draft
                .advance(&current, Instant::now(), immediate || cx.reduce_motion())
        else {
            self.cancel_dictation(cx);
            return false;
        };
        let animating = voice.draft.animating();
        voice.animation_running = animating;
        if value != current {
            let end = value.len();
            self.composer.update(cx, |input, cx| {
                input.set_value(value, window, cx);
                input.set_selected_range(end..end, cx);
            });
        }
        animating
    }

    fn animate_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(voice) = &mut self.dictation else {
            return;
        };
        if voice.animation_running {
            return;
        }
        let generation = voice.generation;
        if !self.advance_dictation(false, window, cx) {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_going = this.update_in(cx, |this, window, cx| {
                    if !this
                        .dictation
                        .as_ref()
                        .is_some_and(|v| v.generation == generation)
                    {
                        return false;
                    }
                    this.advance_dictation(false, window, cx)
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle_dictation(window, cx);
        if let Some(voice) = &mut self.dictation {
            if voice.phase == Phase::Listening {
                voice.phase = Phase::Finishing;
                voice.recorder.finish();
                cx.notify();
            } else {
                self.cancel_dictation(cx);
                self.composer
                    .update(cx, |input, cx| input.focus(window, cx));
            }
            return;
        }
        let owner = self.composer_owner(cx);
        self.dictation_generation += 1;
        let generation = self.dictation_generation;
        let (recorder, mut events) = Recorder::start();
        self.dictation = Some(Dictation {
            recorder,
            owner,
            generation,
            phase: Phase::Starting,
            draft: Draft::new(self.composer.read(cx).value().to_string()),
            animation_running: false,
        });
        // Dictation owns keyboard focus while recording, so no input caret is
        // painted. Clicking the composer explicitly returns to editing.
        window.focus(&self.shell_focus, cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        if !this
                            .dictation
                            .as_ref()
                            .is_some_and(|voice| voice.generation == generation)
                        {
                            return;
                        }
                        if this.composer_owner(cx) != owner {
                            this.cancel_dictation(cx);
                            return;
                        }
                        match event {
                            Event::Listening => {
                                this.dictation.as_mut().unwrap().phase = Phase::Listening
                            }
                            Event::Finishing => {
                                this.dictation.as_mut().unwrap().phase = Phase::Finishing
                            }
                            Event::Text(text) => {
                                let current = this.composer.read(cx).value().to_string();
                                let accepted = this.dictation.as_mut().unwrap().draft.queue(
                                    &current,
                                    &text,
                                    Instant::now(),
                                );
                                if accepted {
                                    this.animate_dictation(window, cx);
                                } else {
                                    this.cancel_dictation(cx);
                                }
                            }
                            Event::Done => {
                                this.settle_dictation(window, cx);
                                this.cancel_dictation(cx);
                                if this.shell_focus.is_focused(window) {
                                    this.composer
                                        .update(cx, |input, cx| input.focus(window, cx));
                                }
                            }
                            Event::Error(message) => {
                                this.settle_dictation(window, cx);
                                this.cancel_dictation(cx);
                                if this.shell_focus.is_focused(window) {
                                    this.composer
                                        .update(cx, |input, cx| input.focus(window, cx));
                                }
                                this.state.update(cx, |state, cx| {
                                    state.error = Some(message);
                                    cx.notify();
                                });
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}
