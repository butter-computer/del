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

/// Fill of the composer card: a faint white lift, like the page it sits on.
pub const COMPOSER_SURFACE_RGBA: u32 = 0xffffff12;
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
        Provider::Acp => ("icons/acp.svg", 0xb1b7c4),
    }
}

/// The agent's logo in its own colour, no background.
pub fn provider_logo(provider: Provider, size: f32) -> Svg {
    let (path, color) = provider_brand(provider);
    svg()
        .path(path)
        .size(zpx(size))
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
                .child(provider_logo(provider, 14.0).when(!provider_ready, |l| l.opacity(0.5)))
                .child(providers::display_name(provider)),
        );
        let agent_el = {
            let current = provider;
            agent.dropdown_menu(move |menu, _, _| {
                Provider::ALL
                    .iter()
                    .fold(menu.check_side(Side::Right), |menu, &p| {
                        let on_provider = on_provider.clone();
                        let (path, color) = provider_brand(p);
                        menu.item(
                            PopupMenuItem::new(providers::display_name(p))
                                .icon(Icon::default().path(path).text_color(rgb(color)))
                                .checked(p == current)
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
                .child(Icon::new(IconName::Folder).small())
                .child(folder)
                .into_any_element()
        } else {
            chip("folder")
                .disabled(starting)
                .icon(IconName::Folder)
                .label(folder)
                .text_color(muted)
                .on_click(move |_, w, cx| on_folder(w, cx))
                .into_any_element()
        };

        // One round button: send, or stop while a turn is running.
        let action = div()
            .id("send")
            .size(zpx(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full();
        let action =
            if starting {
                action.child(gpui_kit::component::spinner::Spinner::new().small())
            } else if running {
                action
                    .bg(rgb(0xffffff))
                    .cursor_pointer()
                    .on_click(move |_, w, cx| on_stop(w, cx))
                    .child(div().size(zpx(9.0)).rounded_xs().bg(rgb(0x111113)))
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
                    .child(Icon::new(IconName::ArrowUp).with_size(zpx(16.)).text_color(
                        if can_send {
                            rgb(0x111113)
                        } else {
                            rgb(0x8a8a8e)
                        },
                    ))
            };

        let voice_label = match dictation {
            crate::dictation::Phase::Idle => "Dictate with Apple speech recognition",
            crate::dictation::Phase::Starting => "Cancel dictation setup",
            crate::dictation::Phase::Listening => "Stop dictation",
            crate::dictation::Phase::Finishing => "Finishing dictation…",
        };
        let voice = chip("dictate")
            .size(zpx(28.))
            .disabled(starting && dictation == crate::dictation::Phase::Idle)
            .tooltip(voice_label)
            .accessibility_label(voice_label)
            .when(dictation == crate::dictation::Phase::Listening, |button| {
                button.bg(rgba(0xff505024))
            })
            .child(match dictation {
                crate::dictation::Phase::Idle => Icon::default()
                    .path("icons/microphone.svg")
                    .with_size(zpx(16.))
                    .text_color(muted)
                    .into_any_element(),
                crate::dictation::Phase::Listening => div()
                    .size(zpx(8.))
                    .rounded_xs()
                    .bg(rgb(0xff6666))
                    .into_any_element(),
                _ => gpui_kit::component::spinner::Spinner::new()
                    .small()
                    .into_any_element(),
            })
            .on_click(move |_, window, cx| on_voice(window, cx));

        // No border: the card is told apart from the page by its fill alone.
        v_flex()
            .w_full()
            .max_w(zpx(max_width))
            .p_2()
            .gap_1()
            .rounded(zpx(14.0))
            .bg(rgba(COMPOSER_SURFACE_RGBA))
            .when_some(attachment_content, |card, content| card.child(content))
            .child(
                Textarea::new(&input)
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
                                    .icon(IconName::Plus)
                                    .tooltip("Attach files")
                                    .accessibility_label("Attach files")
                                    .on_click(move |_, window, cx| on_attach(window, cx)),
                            )
                            .child(agent_el)
                            .child(model_picker)
                            .when(show_folder, |row| row.child(folder_el)),
                    )
                    .child(h_flex().items_center().gap_1().child(voice).child(action)),
            )
    }
}
