//! The one composer used by both the new-chat page and the chat page: a
//! borderless card with the input on top; below it the agent and model
//! dropdowns, the working folder, and a single send / stop button.
//!
//! Standard chat-composer behaviour: Enter sends, Shift+Enter inserts a
//! newline, the send button turns into a stop button while a turn runs (Enter
//! then steers the turn), and the agent / model / folder are locked once the
//! session exists.

use crate::providers;
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

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type ChoiceHandler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ComposerCard {
    input: Entity<TextareaState>,
    max_width: f32,
    provider: Provider,
    provider_ready: bool,
    model: Option<String>,
    folder: String,
    /// The session exists, so its folder can no longer change. (Agent and
    /// model stay dropdowns; picking another opens a new tab.)
    folder_locked: bool,
    running: bool,
    on_send: Handler,
    on_stop: Handler,
    on_folder: Handler,
    on_provider: ChoiceHandler<Provider>,
    on_model: ChoiceHandler<Option<String>>,
}

impl ComposerCard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input: &Entity<TextareaState>,
        max_width: f32,
        provider: Provider,
        provider_ready: bool,
        model: Option<String>,
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
            model,
            folder,
            folder_locked,
            running,
            on_send: noop.clone(),
            on_stop: noop.clone(),
            on_folder: noop,
            on_provider: Rc::new(|_, _, _| {}),
            on_model: Rc::new(|_, _, _| {}),
        }
    }

    pub fn on_send(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_send = Rc::new(f);
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
    pub fn on_model(mut self, f: impl Fn(Option<String>, &mut Window, &mut App) + 'static) -> Self {
        self.on_model = Rc::new(f);
        self
    }
}

/// The agent's logo in its own colour, no background.
pub fn provider_logo(provider: Provider, size: f32) -> Svg {
    let (path, color) = match provider {
        Provider::Claude => ("icons/claude.svg", 0xd97757),
        Provider::Codex => ("icons/codex.svg", 0xf5f5f5),
        Provider::Acp => ("icons/acp.svg", 0xb1b7c4),
    };
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// A flat footer control.
fn chip(id: &'static str) -> Button {
    Button::new(id).ghost().small().compact().rounded_full()
}

impl RenderOnce for ComposerCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let can_send = composer::submission(&self.input.read(cx).value()).is_some();
        let muted = rgba(0xffffff8c);
        let ComposerCard {
            input,
            max_width,
            provider,
            provider_ready,
            model,
            folder,
            folder_locked,
            running,
            on_send,
            on_stop,
            on_folder,
            on_provider,
            on_model,
        } = self;

        let agent = chip("agent").child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(provider_logo(provider, 14.0).when(!provider_ready, |l| l.opacity(0.5)))
                .child(providers::display_name(provider)),
        );
        let agent_el = {
            let current = provider;
            agent.dropdown_menu(move |menu, _, _| {
                Provider::ALL.iter().fold(menu, |menu, &p| {
                    let on_provider = on_provider.clone();
                    menu.item(
                        PopupMenuItem::new(providers::display_name(p))
                            .checked(p == current)
                            .on_click(move |_, w, cx| on_provider(p, w, cx)),
                    )
                })
            })
        };
        let model_el = {
            let current = model.clone();
            chip("model")
                .label(providers::model_label(model.as_deref()))
                .text_color(muted)
                .dropdown_menu(move |menu, _, _| {
                    providers::models(provider).iter().fold(menu, |menu, m| {
                        let on_model = on_model.clone();
                        let value = m.clone();
                        menu.item(
                            PopupMenuItem::new(providers::model_label(m.as_deref()))
                                .checked(value.as_deref() == current.as_deref())
                                .on_click(move |_, w, cx| on_model(value.clone(), w, cx)),
                        )
                    })
                })
        };
        let folder_el = chip("folder")
            .icon(IconName::Folder)
            .label(folder)
            .text_color(muted)
            .when(!folder_locked, |b| {
                let on_folder = on_folder.clone();
                b.on_click(move |_, w, cx| on_folder(w, cx))
            });

        // One round button: send, or stop while a turn is running.
        let action = div()
            .id("send")
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full();
        let action = if running {
            action
                .bg(rgb(0xffffff))
                .cursor_pointer()
                .on_click(move |_, w, cx| on_stop(w, cx))
                .child(div().size(px(9.0)).rounded_xs().bg(rgb(0x111113)))
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
                    Icon::new(IconName::ArrowUp)
                        .small()
                        .text_color(if can_send {
                            rgb(0x111113)
                        } else {
                            rgb(0x8a8a8e)
                        }),
                )
        };

        // No border: the card is told apart from the page by its fill alone.
        v_flex()
            .w_full()
            .max_w(px(max_width))
            .p_2()
            .gap_1()
            .rounded(px(14.0))
            .bg(rgba(COMPOSER_SURFACE_RGBA))
            .child(
                Textarea::new(&input)
                    .appearance(false)
                    .bordered(false)
                    .text_size(px(TEXT_MD)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_0p5()
                            .child(agent_el)
                            .child(model_el)
                            .child(folder_el),
                    )
                    .child(action),
            )
    }
}
