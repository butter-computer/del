//! The one composer used by both the new-chat page and the chat page: a
//! borderless card with the input on top; below it the agent and model
//! dropdowns, the working folder, and a single send / stop button.
//!
//! Standard chat-composer behaviour: Enter sends, Shift+Enter inserts a
//! newline, the send button turns into a stop button while a turn runs (Enter
//! then steers the turn), and the agent / model / folder are locked once the
//! session exists.

use crate::animated_icons::Glyph;
use crate::appearance::{rgb, rgba};
use crate::micro_motion::MotionIconExt as _;
use crate::providers;
use crate::zoom::px as zpx;
use del_agent::Provider;
use del_core::composer;
use del_core::typography::TEXT_PROSE;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

// Paint-only overlays preserve selection, scrolling, and pointer input. Read the
// laid-out textarea during paint so wrapping and auto-grow use the same geometry.
fn input_edge_fades(input: &Entity<TextareaState>) -> Vec<AnyElement> {
    [true, false]
        .into_iter()
        .map(|top| {
            let input = input.clone();
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, cx| {
                    let state = input.read(cx);
                    let Some(line_height) = state.line_height() else {
                        return;
                    };
                    let offset = state.scroll_offset().y;
                    let content_height = line_height * state.display_map().wrap_row_count() as f32;
                    let distance = if top {
                        -offset
                    } else {
                        content_height + offset - state.input_bounds().size.height
                    };
                    let alpha = (distance / zpx(16.)).clamp(0., 1.);
                    if alpha > 0. {
                        let surface = crate::appearance::composer_surface();
                        window.paint_quad(fill(
                            bounds,
                            linear_gradient(
                                if top { 180. } else { 0. },
                                linear_color_stop(surface.opacity(alpha), 0.),
                                linear_color_stop(surface.opacity(0.), 1.),
                            ),
                        ));
                    }
                },
            )
            .absolute()
            .left_0()
            .right_0()
            .h(zpx(16.))
            .when(top, |fade| fade.top_0())
            .when(!top, |fade| fade.bottom_0())
            .into_any_element()
        })
        .collect()
}

pub(crate) fn account_menu_label(
    name: String,
    remaining: Option<f64>,
    selected: bool,
) -> impl IntoElement {
    let scale = 18. / 26.;
    h_flex()
        .w_full()
        .gap(zpx(crate::ui::SPACE_LG))
        .items_center()
        .child(
            div()
                .flex_1()
                .when(selected, |label| {
                    label.text_color(crate::appearance::ActionTone::Success.foreground())
                })
                .child(name),
        )
        .children(remaining.map(|remaining| {
            div()
                .id("account-usage-ring")
                .size(zpx(26. * scale))
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .aria_label(format!("{remaining:.0}% usage left"))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let center = bounds.center();
                            let radius = bounds.size.width / 2. - zpx(1.5 * scale);
                            for (fraction, color) in [
                                (1., rgba(0xffffff26)),
                                ((remaining / 100.) as f32, rgb(0xebebeb)),
                            ] {
                                if fraction <= 0. {
                                    continue;
                                }
                                let mut path = PathBuilder::stroke(zpx(1.7 * scale));
                                let steps = 80;
                                for i in 0..=steps {
                                    let angle = -std::f32::consts::FRAC_PI_2
                                        + std::f32::consts::TAU * fraction * i as f32
                                            / steps as f32;
                                    let position = point(
                                        center.x + radius * angle.cos(),
                                        center.y + radius * angle.sin(),
                                    );
                                    if i == 0 {
                                        path.move_to(position);
                                    } else {
                                        path.line_to(position);
                                    }
                                }
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, color);
                                }
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
        }))
}

/// Composer controls stay readable but sit below what is being written.
pub(crate) const CONTROL_TEXT: u32 = 0xffffffb8;

/// The composer's prompt when idle, while a reply runs, and in goal mode.
pub(crate) const PLACEHOLDER: &str = "Ask anything…";
pub(crate) const QUEUE_PLACEHOLDER: &str = "Queue a follow-up…";
pub(crate) const GOAL_PLACEHOLDER: &str = "Describe the goal and how to measure success…";

/// What the round composer button does right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    /// Nothing to send yet.
    Idle,
    Send,
    /// A reply is running: Return queues the draft, and so does the button.
    Queue,
    /// A reply is running and the draft is empty.
    Stop,
    /// The first prompt is starting its session.
    Starting,
}

impl ComposerAction {
    pub(crate) fn new(running: bool, can_send: bool, starting: bool) -> Self {
        match (starting, running, can_send) {
            (true, _, _) => Self::Starting,
            (_, true, true) => Self::Queue,
            (_, true, false) => Self::Stop,
            (_, false, true) => Self::Send,
            (_, false, false) => Self::Idle,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Idle | Self::Send => "Send (Return)",
            Self::Queue => "Queue (Return)",
            Self::Stop => "Stop (⌘.)",
            Self::Starting => "Starting…",
        }
    }

    fn accessibility_label(self) -> &'static str {
        match self {
            Self::Idle | Self::Send => "Send message",
            Self::Queue => "Queue message",
            Self::Stop => "Stop response",
            Self::Starting => "Starting agent",
        }
    }
}

/// The working-folder chip, shared by new-session drafts and the composer:
/// one icon, one label, and the full path on hover.
pub(crate) fn folder_chip(
    id: &'static str,
    path: &std::path::Path,
    compact: bool,
    disabled_reason: Option<&'static str>,
) -> Button {
    let full = crate::ui::display_path(&path.to_string_lossy());
    Button::new(id)
        .ghost()
        .disabled(disabled_reason.is_some())
        .h(zpx(24.))
        .px(zpx(if compact {
            crate::ui::SPACE_XS
        } else {
            crate::ui::SPACE_MD
        }))
        .min_w_0()
        .max_w(zpx(150.))
        .text_size(zpx(12.))
        .text_color(crate::ui::secondary_text())
        .tooltip(match disabled_reason {
            Some(reason) => format!("{full}\n{reason}"),
            None => full,
        })
        .accessibility_label("Choose project folder")
        .child(
            h_flex()
                .gap(zpx(crate::ui::SPACE_XS))
                .items_center()
                .child(crate::icons::icon(IconName::Folder).with_size(zpx(crate::icons::MARK_SIZE)))
                .when(!compact, |row| {
                    row.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(crate::sessions::folder_name(path)),
                    )
                }),
        )
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type PasteHandler = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;
type ChoiceHandler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ComposerCard {
    input: Entity<TextareaState>,
    max_width: f32,
    provider: Provider,
    provider_ready: bool,
    account_state: Option<Entity<crate::state::AppState>>,
    provider_menu_open: bool,
    on_provider_menu: ChoiceHandler<bool>,
    model_picker: Option<Entity<crate::model_picker::ModelPicker>>,
    settings: Option<AnyElement>,
    show_voice: bool,
    folder: String,
    /// Shown on hover over the folder, e.g. the session's branch.
    folder_tooltip: Option<String>,
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
        model_picker: impl Into<Option<Entity<crate::model_picker::ModelPicker>>>,
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
            account_state: None,
            provider_menu_open: false,
            on_provider_menu: Rc::new(|_, _, _| {}),
            model_picker: model_picker.into(),
            settings: None,
            show_voice: false,
            folder,
            folder_tooltip: None,
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

    /// A conversation can supply independently scoped account/model controls
    /// while sharing the same input, attachments and send/stop presentation.
    pub fn provider_menu(
        mut self,
        open: bool,
        change: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.provider_menu_open = open;
        self.on_provider_menu = Rc::new(change);
        self
    }

    pub fn settings(mut self, controls: impl IntoElement) -> Self {
        self.settings = Some(controls.into_any_element());
        self
    }

    pub fn accounts(mut self, state: Entity<crate::state::AppState>) -> Self {
        self.account_state = Some(state);
        self
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
    pub fn folder_tooltip(mut self, tooltip: Option<String>) -> Self {
        self.folder_tooltip = tooltip;
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
        self.show_voice = true;
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
        .size(zpx(crate::icons::MARK_SIZE))
        .flex_none()
        .text_color(rgb(color))
}

/// A flat footer control.
fn chip(id: &'static str) -> Button {
    Button::new(id).ghost().small().compact().rounded_full()
}

/// The provider chip. A locked chip says why in its tooltip.
pub(crate) fn provider_button(
    id: &'static str,
    provider: Provider,
    provider_ready: bool,
    locked: Option<&'static str>,
) -> Button {
    let chip = chip(id);
    match locked {
        Some(reason) => chip.tooltip(reason),
        None => chip.tooltip_with_action("Select provider / account", &crate::SelectProvider, None),
    }
    .text_color(rgba(CONTROL_TEXT))
    .h(zpx(24.))
    .px(zpx(crate::ui::SPACE_MD))
    .disabled(locked.is_some())
    .child(
        h_flex()
            .gap(zpx(crate::ui::SPACE_SM))
            .items_center()
            .child(provider_logo(provider).when(!provider_ready, |l| l.opacity(0.5)))
            .child(providers::display_name(provider)),
    )
}

/// Why a provider cannot start yet, from its detected status.
pub(crate) fn provider_problem(status: &del_agent::ProviderStatus) -> Option<&'static str> {
    if !status.installed {
        Some("Not installed")
    } else if !providers::ready(status) {
        Some("Sign in")
    } else {
        None
    }
}

impl RenderOnce for ComposerCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let can_send = self.dictation == crate::dictation::Phase::Idle
            && (self.has_attachments
                || composer::submission(&self.input.read(cx).value()).is_some());
        let muted = crate::ui::secondary_text();
        // While a reply runs, Return queues: say so where the prompt goes.
        let placeholder = if self.goal_mode {
            GOAL_PLACEHOLDER
        } else if self.running {
            QUEUE_PLACEHOLDER
        } else {
            PLACEHOLDER
        };
        if self.input.read(cx).presentation().placeholder().as_ref() != placeholder {
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            });
        }
        let problem = self
            .account_state
            .as_ref()
            .filter(|_| !self.provider_ready)
            .and_then(|state| state.read(cx).status_of(self.provider))
            .and_then(provider_problem);
        let ComposerCard {
            input,
            max_width,
            provider,
            provider_ready,
            account_state,
            provider_menu_open,
            on_provider_menu,
            model_picker,
            settings,
            show_voice,
            folder,
            folder_tooltip,
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

        let agent = provider_button(
            "agent",
            provider,
            provider_ready,
            starting.then_some("Can’t switch provider while the session starts"),
        );
        let agent_el = {
            let current = provider;
            // The composer sits low in the window, so its menu opens upward.
            agent.dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, cx| {
                providers::VISIBLE
                    .iter()
                    .fold(menu.check_side(Side::Right), |menu, &p| {
                        let on_provider = on_provider.clone();
                        if let Some(state) = account_state.clone().filter(|_| p != Provider::Acp) {
                            let default_state = state.clone();
                            let (logo, color) = provider_brand(p);
                            // A provider that cannot start says why in its row.
                            let label = match state.read(cx).status_of(p).and_then(provider_problem)
                            {
                                Some(problem) => {
                                    format!("{} · {problem}", providers::display_name(p))
                                }
                                None => providers::display_name(p).to_string(),
                            };
                            menu.submenu_with_icon(
                                Some(crate::icons::path(logo).text_color(rgb(color))),
                                label,
                                window,
                                cx,
                                move |menu, _, cx| {
                                    let accounts = state
                                        .read(cx)
                                        .visible_accounts(p)
                                        .into_iter()
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    let system = state.clone();
                                    let system_label = state.clone();
                                    let mut menu =
                                        menu.item(
                                            PopupMenuItem::element(move |_, cx| {
                                                let s = system_label.read(cx);
                                                account_menu_label(
                                                    s.default_account_label(p),
                                                    s.account_usage_remaining(p, None),
                                                    s.account_is_selected(p, None),
                                                )
                                            })
                                            .on_click(move |_, _, cx| {
                                                system.update(cx, |s, cx| {
                                                    s.select_account(p, None, cx)
                                                })
                                            }),
                                        );
                                    for account in accounts {
                                        let state = state.clone();
                                        let id = account.id.clone();
                                        let selected =
                                            state.read(cx).account_is_selected(p, Some(&id));
                                        let label_state = state.clone();
                                        let label_id = id.clone();
                                        menu = menu.item(
                                            PopupMenuItem::element(move |_, cx| {
                                                account_menu_label(
                                                    account.display(),
                                                    label_state.read(cx).account_usage_remaining(
                                                        p,
                                                        Some(&label_id),
                                                    ),
                                                    selected,
                                                )
                                            })
                                            .on_click(move |_, _, cx| {
                                                state.update(cx, |s, cx| {
                                                    s.select_account(p, Some(id.clone()), cx)
                                                })
                                            }),
                                        );
                                    }
                                    menu
                                },
                            )
                            .submenu_on_click(move |_, _, cx| {
                                default_state.update(cx, |s, cx| s.select_default_account(p, cx))
                            })
                        } else {
                            menu.item(
                                PopupMenuItem::new(providers::display_name(p))
                                    .checked(p == current)
                                    .on_click(move |_, w, cx| on_provider(p, w, cx)),
                            )
                        }
                    })
            })
        };
        let agent_el = agent_el
            .open(provider_menu_open)
            .on_open_change(move |open, window, cx| on_provider_menu(*open, window, cx));
        // One chip for the folder, whether or not it can still change.
        let folder_el = {
            let path = std::path::PathBuf::from(&folder);
            let reason = if folder_locked {
                Some("The folder is fixed once a session starts")
            } else if starting {
                Some("Can’t change folder while the session starts")
            } else {
                None
            };
            let chip = folder_chip("folder", &path, false, reason);
            match folder_tooltip.filter(|_| folder_locked) {
                Some(tip) => chip.tooltip(tip),
                None => chip,
            }
            .on_click(move |_, w, cx| on_folder(w, cx))
            .into_any_element()
        };

        // One round button: send, queue while a reply runs, or stop when the
        // draft is empty. Its fill eases between lit and idle.
        let state = ComposerAction::new(running, can_send, starting);
        let lit = gpui_kit::base::transition(
            "composer-action-fill",
            if matches!(state, ComposerAction::Idle) {
                0.
            } else {
                1.
            },
            gpui_kit::base::Transition::new(std::time::Duration::from_millis(140))
                .ease(crate::ui::ease_out),
            window,
            cx,
        );
        let mix = |idle: u32, active: u32| {
            let channel = |shift: u32| {
                let a = ((idle >> shift) & 0xff) as f32;
                let b = ((active >> shift) & 0xff) as f32;
                (a + (b - a) * lit).round() as u32
            };
            rgb((channel(16) << 16) | (channel(8) << 8) | channel(0))
        };
        let action = div()
            .id("send")
            .debug_selector(|| "composer-action".into())
            .size(crate::icons::button_size())
            .flex()
            .items_center()
            .justify_center()
            .rounded_full();
        let action = if state == ComposerAction::Starting {
            action
                .role(Role::Button)
                .aria_label(state.accessibility_label())
                .child(crate::icons::spinner())
        } else {
            action
                .role(Role::Button)
                .aria_label(state.accessibility_label())
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(state.label()).build(window, cx)
                })
                .bg(mix(0x4a4a4e, 0xffffff))
                .when(state != ComposerAction::Idle, |a| {
                    a.cursor_pointer()
                        .hover(|a| a.bg(rgb(0xe4e4e4)))
                        .active(|a| a.bg(rgb(0xc8c8c8)))
                        .on_click(move |_, w, cx| {
                            if state == ComposerAction::Stop {
                                on_stop(w, cx);
                            } else {
                                // Send, or queue while a reply runs: what Return does.
                                on_send(w, cx);
                            }
                        })
                })
                .child(
                    crate::micro_motion::MorphIcon::new(
                        "send-stop-icon",
                        crate::animated_icons::Morph::SendStop,
                        state == ComposerAction::Stop,
                    )
                    .text_color(mix(0x8a8a8e, 0x111113)),
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
            .on_click(move |_, window, cx| on_voice(window, cx))
            .motion_child(
                !starting || dictation != crate::dictation::Phase::Idle,
                move |feedback| match dictation {
                    crate::dictation::Phase::Idle => feedback
                        .glyph(Glyph::Microphone)
                        .text_color(muted)
                        .into_any_element(),
                    crate::dictation::Phase::Listening => crate::icons::icon(IconName::Square)
                        .text_color(rgb(0xff6666))
                        .into_any_element(),
                    _ => crate::icons::spinner().into_any_element(),
                },
            );

        // The wrapper owns the tint; the textarea adds no second background.
        v_flex()
            .debug_selector(|| "composer-card".into())
            .w_full()
            .max_w(zpx(max_width))
            .p(zpx(crate::ui::SPACE_MD))
            .gap(zpx(crate::ui::SPACE_XS))
            .rounded(zpx(crate::ui::RADIUS_CARD))
            .bg(crate::appearance::composer_surface())
            // A hairline holds the edge over any background; focus firms it up.
            .border_1()
            .border_color(if focused {
                rgba(0xffffff1f)
            } else {
                crate::appearance::separator()
            })
            .when_some(problem, |card, problem| {
                // The selected provider cannot start: say what to fix before sending.
                card.child(
                    h_flex()
                        .px(zpx(crate::ui::SPACE_XS))
                        .gap(zpx(crate::ui::SPACE_SM))
                        .items_center()
                        .text_size(zpx(del_core::typography::TEXT_SM))
                        .text_color(crate::appearance::ActionTone::Warning.foreground())
                        .child(format!(
                            "{} · {}",
                            providers::display_name(provider),
                            if problem == "Sign in" {
                                "Sign in to send messages"
                            } else {
                                "Not installed"
                            }
                        )),
                )
            })
            .when_some(attachment_content, |card, content| card.child(content))
            .child(
                div()
                    .relative()
                    .mx(-zpx(4.))
                    .child(
                        Textarea::new(&input)
                            .token(|token, _, _| {
                                h_flex()
                                    .items_center()
                                    .gap(zpx(crate::ui::SPACE_XS))
                                    .text_size(zpx(TEXT_PROSE))
                                    .text_color(rgb(0x579fff))
                                    .child(
                                        crate::icons::path("icons/skill.svg")
                                            .flex_none()
                                            .text_color(rgb(0x579fff)),
                                    )
                                    .child(token.token().label().clone())
                            })
                            .on_paste(move |item, window, cx| on_paste(item, window, cx))
                            .appearance(false)
                            .bordered(false)
                            .text_size(zpx(TEXT_PROSE)),
                    )
                    .children(input_edge_fades(&input)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(zpx(crate::ui::SPACE_2XS))
                            .child(crate::micro_motion::control(
                                "attach-motion",
                                true,
                                move |motion| {
                                    chip("attach-files")
                                        .text_color(muted)
                                        .size(crate::icons::button_size())
                                        .p_0()
                                        .child(motion.glyph(Glyph::Add))
                                        .tooltip("Attach files")
                                        .accessibility_label("Attach files")
                                        .on_click(move |_, window, cx| on_attach(window, cx))
                                },
                            ))
                            .child(settings.unwrap_or_else(|| {
                                h_flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap(zpx(crate::ui::SPACE_2XS))
                                    .child(div().flex_none().child(agent_el))
                                    .when_some(model_picker, |row, picker| row.child(picker))
                                    .into_any_element()
                            }))
                            .when(goal_mode, |row| {
                                row.child(
                                    chip("goal-mode")
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap(zpx(crate::ui::SPACE_XS))
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
                            .when(show_folder, |row| {
                                row.child(div().flex_none().child(folder_el))
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap(zpx(crate::ui::SPACE_XS))
                            .when(show_voice, |row| row.child(voice))
                            .child(action),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::gpui;
    use core::prelude::v1::test;

    struct StopFixture {
        input: Entity<TextareaState>,
        picker: Entity<crate::model_picker::ModelPicker>,
        provider: Provider,
        running: bool,
        sent: std::rc::Rc<std::cell::Cell<usize>>,
        stopped: std::rc::Rc<std::cell::Cell<usize>>,
        width: f32,
    }
    impl Render for StopFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let stopped = self.stopped.clone();
            let sent = self.sent.clone();
            div().w(px(self.width)).child(
                ComposerCard::new(
                    &self.input,
                    self.width,
                    self.provider,
                    true,
                    self.picker.clone(),
                    "project".into(),
                    true,
                    self.running,
                )
                .on_send(move |_, _| sent.set(sent.get() + 1))
                .on_stop(move |_, _| stopped.set(stopped.get() + 1)),
            )
        }
    }
    #[gpui::test]
    fn send_and_stop_remain_immediate_during_icon_changes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        let stopped = std::rc::Rc::new(std::cell::Cell::new(0));
        let sent = std::rc::Rc::new(std::cell::Cell::new(0));
        let (root, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| TextareaState::new(window, cx));
            input.update(cx, |input, cx| input.focus(window, cx));
            let picker = cx.new(|cx| crate::model_picker::ModelPicker::new(state, window, cx));
            StopFixture {
                input,
                picker,
                provider: Provider::Claude,
                running: true,
                sent: sent.clone(),
                stopped: stopped.clone(),
                width: 600.,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("composer-action").unwrap().center();
        cx.simulate_mouse_move(target, None, Modifiers::default());
        cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        assert_eq!(stopped.get(), 1);
        for (index, provider) in [Provider::Claude, Provider::Codex, Provider::OpenCode]
            .into_iter()
            .enumerate()
        {
            for running in [false, true] {
                cx.update(|window, cx| {
                    root.update(cx, |view, cx| {
                        view.provider = provider;
                        view.running = running;
                        view.input
                            .update(cx, |input, cx| input.set_value("test", window, cx));
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                });
                assert_eq!(cx.debug_bounds("composer-action").unwrap().center(), target);
                // Click on the first frame, before the glyph finishes morphing.
                // A draft while a reply runs queues, exactly like Return.
                cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
                cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
                assert_eq!(sent.get(), 2 * index + 1 + usize::from(running));
                assert_eq!(stopped.get(), 1);
            }
        }
        // With the draft cleared, the running button is Stop again.
        cx.update(|window, cx| {
            root.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        cx.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        assert_eq!(stopped.get(), 2);
    }

    #[test]
    fn running_with_a_draft_queues_and_only_an_empty_draft_stops() {
        use ComposerAction::*;
        assert_eq!(ComposerAction::new(false, false, false), Idle);
        assert_eq!(ComposerAction::new(false, true, false), Send);
        assert_eq!(ComposerAction::new(true, true, false), Queue);
        assert_eq!(ComposerAction::new(true, false, false), Stop);
        assert_eq!(ComposerAction::new(true, true, true), Starting);
        assert_eq!(Queue.label(), "Queue (Return)");
        assert_eq!(Stop.label(), "Stop (⌘.)");
        assert_eq!(Send.label(), "Send (Return)");
    }

    #[gpui::test]
    fn long_model_name_keeps_send_inside_a_narrow_composer(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        state.update(cx, |state, cx| {
            state.set_model(
                Some("A deliberately long model name that must shrink".into()),
                cx,
            );
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| TextareaState::new(window, cx));
            let picker = cx.new(|cx| crate::model_picker::ModelPicker::new(state, window, cx));
            StopFixture {
                input,
                picker,
                provider: Provider::Claude,
                running: false,
                sent: Default::default(),
                stopped: Default::default(),
                width: 320.,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let card = cx.debug_bounds("composer-card").unwrap();
        let send = cx.debug_bounds("composer-action").unwrap();
        assert!(send.right() <= card.right());
        assert!(send.left() >= card.left());
    }
}
