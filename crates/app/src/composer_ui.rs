//! The one composer used by both the new-chat page and the chat page: a
//! borderless card with the input on top; below it the agent and model
//! dropdowns, the working folder, and a single send / stop button.
//!
//! Standard chat-composer behaviour: Enter sends, Shift+Enter inserts a
//! newline, the send button turns into a stop button while a turn runs (Enter
//! then steers the turn), and the agent / model / folder are locked once the
//! session exists.

use crate::appearance::{rgb, rgba};
use crate::providers;
use crate::zoom::px as zpx;
use del_agent::Provider;
use del_core::composer;
use del_core::typography::TEXT_MD;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

pub const QUEUE_SURFACE_RGBA: u32 = 0xffffff10;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type PasteHandler = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;
type ChoiceHandler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ComposerCard {
    input: Entity<TextareaState>,
    max_width: f32,
    provider: Provider,
    provider_ready: bool,
    model_picker: Entity<crate::model_picker::ModelPicker>,
    folder: String,
    /// The session exists, so its working folder can no longer change.
    folder_locked: bool,
    show_folder: bool,
    running: bool,
    starting: bool,
    on_send: Handler,
    goal_mode: bool,
    on_cancel_goal: Handler,
    on_voice: Handler,
    dictation: crate::dictation::Phase,
    on_attach: Handler,
    on_paste: PasteHandler,
    has_attachments: bool,
    attachment_content: Option<AnyElement>,
    on_stop: Handler,
    on_folder: Handler,
    on_provider: ChoiceHandler<Provider>,
}

impl ComposerCard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input: &Entity<TextareaState>,
        max_width: f32,
        provider: Provider,
        provider_ready: bool,
        model_picker: Entity<crate::model_picker::ModelPicker>,
        folder: String,
        folder_locked: bool,
        running: bool,
    ) -> Self {
        let noop: Handler = Rc::new(|_, _| {});
        Self {
            input: input.clone(),
            max_width,
            provider,
            provider_ready,
            model_picker,
            folder,
            folder_locked,
            show_folder: true,
            running,
            starting: false,
            on_send: noop.clone(),
            goal_mode: false,
            on_cancel_goal: noop.clone(),
            on_voice: noop.clone(),
            dictation: Default::default(),
            on_attach: noop.clone(),
            on_paste: Rc::new(|_, _, _| false),
            has_attachments: false,
            attachment_content: None,
            on_stop: noop.clone(),
            on_folder: noop,
            on_provider: Rc::new(|_, _, _| {}),
        }
    }

    pub fn goal_mode(mut self, enabled: bool) -> Self {
        self.goal_mode = enabled;
        self
    }
    pub fn on_cancel_goal(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel_goal = Rc::new(handler);
        self
    }
    pub fn attachments(mut self, has: bool) -> Self {
        self.has_attachments = has;
        self
    }
    pub fn attachment_content(mut self, content: Option<AnyElement>) -> Self {
        self.attachment_content = content;
        self
    }
    pub fn on_attach(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_attach = Rc::new(handler);
        self
    }
    pub fn on_paste(
        mut self,
        handler: impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_paste = Rc::new(handler);
        self
    }
    pub fn show_folder(mut self, show: bool) -> Self {
        self.show_folder = show;
        self
    }

    pub fn starting(mut self, starting: bool) -> Self {
        self.starting = starting;
        self
    }

    pub fn on_send(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_send = Rc::new(f);
        self
    }
    pub fn dictation(mut self, phase: crate::dictation::Phase) -> Self {
        self.dictation = phase;
        self
    }
    pub fn on_voice(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_voice = Rc::new(handler);
        self
    }
    pub fn on_stop(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_stop = Rc::new(f);
        self
    }
    pub fn on_folder(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_folder = Rc::new(f);
        self
    }
    pub fn on_provider(mut self, f: impl Fn(Provider, &mut Window, &mut App) + 'static) -> Self {
        self.on_provider = Rc::new(f);
        self
    }
}

pub(crate) fn provider_brand(provider: Provider) -> (&'static str, u32) {
    match provider {
        Provider::Claude => ("icons/claude.svg", 0xd97757),
        Provider::Codex => ("icons/codex.svg", 0xf5f5f5),
        Provider::OpenCode => ("icons/opencode.svg", 0xf5f5f5),
        Provider::Acp => ("icons/acp.svg", 0xb1b7c4),
    }
}

/// The agent's logo in its own colour, no background.
pub fn provider_logo(provider: Provider) -> Svg {
    let (path, color) = provider_brand(provider);
    svg()
        .path(path)
        .size(crate::icons::size())
        .flex_none()
        .text_color(rgb(color))
}

/// A flat footer control.
fn chip(id: &'static str) -> Button {
    Button::new(id).ghost().small().compact().rounded_full()
}

impl RenderOnce for ComposerCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let can_send = self.dictation == crate::dictation::Phase::Idle
            && (self.has_attachments
                || composer::submission(&self.input.read(cx).value()).is_some());
        let muted = rgba(0xffffff8c);
        let ComposerCard {
            input,
            max_width,
            provider,
            provider_ready,
            model_picker,
            folder,
            folder_locked,
            show_folder,
            running,
            starting,
            on_send,
            goal_mode,
            on_cancel_goal,
            on_voice,
            dictation,
            on_attach,
            on_paste,
            has_attachments: _,
            attachment_content,
            on_stop,
            on_folder,
            on_provider,
        } = self;

        let agent = chip("agent").disabled(starting).child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(provider_logo(provider).when(!provider_ready, |l| l.opacity(0.5)))
                .child(providers::display_name(provider)),
        );
        let agent_el = {
            let current = provider;
            agent.dropdown_menu(move |menu, _, _| {
                Provider::ALL
                    .iter()
                    .fold(menu.check_side(Side::Right), |menu, &p| {
                        let on_provider = on_provider.clone();
                        menu.item(
                            PopupMenuItem::element(move |_, _| {
                                h_flex()
                                    .h(crate::icons::button_size())
                                    .gap_2()
                                    .items_center()
                                    .child(provider_logo(p))
                                    .child(providers::display_name(p))
                                    .child(div().flex_1())
                                    .when(p == current, |row| {
                                        row.child(crate::icons::icon(IconName::Check))
                                    })
                            })
                            .on_click(move |_, w, cx| on_provider(p, w, cx)),
                        )
                    })
            })
        };
        let folder_el = if folder_locked {
            h_flex()
                .h(zpx(20.0))
                .px_1()
                .gap_1()
                .items_center()
                .text_size(zpx(12.0))
                .text_color(muted)
                .child(crate::icons::icon(IconName::Folder))
                .child(folder)
                .into_any_element()
        } else {
            chip("folder")
                .disabled(starting)
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(crate::icons::icon(IconName::Folder))
                        .child(folder),
                )
                .text_color(muted)
                .on_click(move |_, w, cx| on_folder(w, cx))
                .into_any_element()
        };

        // One round button: send, or stop while a turn is running.
        let action = div()
            .id("send")
            .size(crate::icons::button_size())
            .flex()
            .items_center()
            .justify_center()
            .rounded_full();
        let action = if starting {
            action.child(crate::icons::spinner())
        } else if running {
            action
                .bg(rgb(0xffffff))
                .cursor_pointer()
                .on_click(move |_, w, cx| on_stop(w, cx))
                .child(crate::icons::icon(IconName::Square).text_color(rgb(0x111113)))
        } else {
            action
                .bg(if can_send {
                    rgb(0xffffff)
                } else {
                    rgb(0x4a4a4e)
                })
                .when(can_send, |a| {
                    a.cursor_pointer().on_click(move |_, w, cx| on_send(w, cx))
                })
                .child(
                    crate::icons::icon(IconName::ArrowUp).text_color(if can_send {
                        rgb(0x111113)
                    } else {
                        rgb(0x8a8a8e)
                    }),
                )
        };

        let voice_label = match dictation {
            crate::dictation::Phase::Idle => "Dictate with Apple speech recognition",
            crate::dictation::Phase::Starting => "Cancel dictation setup",
            crate::dictation::Phase::Listening => "Stop dictation",
            crate::dictation::Phase::Finishing => "Finishing dictation…",
        };
        let voice = chip("dictate")
            .size(crate::icons::button_size())
            .disabled(starting && dictation == crate::dictation::Phase::Idle)
            .tooltip(voice_label)
            .accessibility_label(voice_label)
            .when(dictation == crate::dictation::Phase::Listening, |button| {
                button.bg(rgba(0xff505024))
            })
            .child(match dictation {
                crate::dictation::Phase::Idle => crate::icons::path("icons/microphone.svg")
                    .text_color(muted)
                    .into_any_element(),
                crate::dictation::Phase::Listening => crate::icons::icon(IconName::Square)
                    .text_color(rgb(0xff6666))
                    .into_any_element(),
                _ => crate::icons::spinner().into_any_element(),
            })
            .on_click(move |_, window, cx| on_voice(window, cx));

        // The wrapper owns the tint; the textarea adds no second background.
        v_flex()
            .w_full()
            .max_w(zpx(max_width))
            .p_2()
            .gap_1()
            .rounded(zpx(14.0))
            .bg(crate::appearance::composer_surface())
            .when_some(attachment_content, |card, content| card.child(content))
            .child(
                Textarea::new(&input)
                    .token(|token, _, _| {
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_size(zpx(TEXT_MD))
                            .text_color(rgb(0xf5f5f5))
                            .child(
                                crate::icons::path("icons/skill.svg")
                                    .flex_none()
                                    .text_color(rgb(0xf5f5f5)),
                            )
                            .child(token.token().label().clone())
                    })
                    .on_paste(move |item, window, cx| on_paste(item, window, cx))
                    .appearance(false)
                    .bordered(false)
                    .text_size(zpx(TEXT_MD)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_0p5()
                            .child(
                                chip("attach-files")
                                    .size(crate::icons::button_size())
                                    .p_0()
                                    .child(crate::icons::icon(IconName::Plus))
                                    .tooltip("Attach files")
                                    .accessibility_label("Attach files")
                                    .on_click(move |_, window, cx| on_attach(window, cx)),
                            )
                            .child(agent_el)
                            .child(model_picker)
                            .when(goal_mode, |row| {
                                row.child(
                                    chip("goal-mode")
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_1()
                                                .child(crate::icons::icon(
                                                    gpui_kit::assets::IconName::Target,
                                                ))
                                                .child("Goal"),
                                        )
                                        .tooltip("Cancel goal mode")
                                        .accessibility_label("Cancel goal mode")
                                        .on_click(move |_, window, cx| on_cancel_goal(window, cx)),
                                )
                            })
                            .when(show_folder, |row| row.child(folder_el)),
                    )
                    .child(h_flex().items_center().gap_1().child(voice).child(action)),
            )
    }
}
