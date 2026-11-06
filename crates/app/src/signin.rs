//! Compact provider setup rows. Credentials remain managed by the provider CLI.
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use crate::{composer_ui::provider_brand, providers, state::AppState, ui};
use del_agent::{LoginState, Provider, ProviderStatus};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Providers,
    Sessions,
    Keybinds,
}

fn usage_bar(remaining: Option<f64>) -> impl IntoElement {
    div()
        .w_full()
        .max_w(zpx(240.))
        .children(remaining.map(|remaining| {
            h_flex()
                .w_full()
                .gap(zpx(ui::SPACE_SM))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .h(zpx(4.))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(rgba(0xffffff20))
                        .child(
                            div()
                                .h_full()
                                .w(relative((remaining / 100.) as f32))
                                .rounded_full()
                                .bg(rgb(0xebebeb)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(zpx(del_core::typography::TEXT_XS))
                        .text_color(ui::secondary_text())
                        .child(format!("{remaining:.0}% left")),
                )
        }))
}

/// Why account actions are disabled while a provider login is running.
const FINISH_SIGN_IN: &str = "Finish the current sign-in first";

/// One card for every Settings group: composer surface, card radius, one padding.
fn settings_card() -> Div {
    v_flex()
        .w_full()
        .min_w_0()
        .px(zpx(ui::SPACE_LG))
        .py(zpx(ui::SPACE_XS))
        .rounded(zpx(ui::RADIUS_CARD))
        .bg(crate::appearance::composer_surface())
}

/// One Settings row: the label, an optional description below it, then the
/// caller's control on the right.
fn settings_row(label: impl IntoElement, description: Option<SharedString>) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .py(zpx(ui::SPACE_MD))
        .gap(zpx(ui::SPACE_XL))
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(zpx(ui::SPACE_2XS))
                .child(label)
                .when_some(description, |text, description| {
                    text.child(
                        div()
                            .text_size(zpx(TEXT_SM))
                            .text_color(ui::secondary_text())
                            .child(description),
                    )
                }),
        )
}

fn settings_divider() -> Div {
    div().h(zpx(1.)).bg(crate::appearance::separator())
}

/// A missing CLI gets its install command, ready to copy.
fn install_hint(provider: Provider, command: &'static str) -> Div {
    let name = providers::display_name(provider);
    h_flex()
        .debug_selector(move || format!("install-hint-{name}"))
        .w_full()
        .min_w_0()
        .pl(zpx(crate::icons::MARK_SIZE + ui::SPACE_MD))
        .pb(zpx(ui::SPACE_SM))
        .gap(zpx(ui::SPACE_SM))
        .items_center()
        .text_size(zpx(TEXT_SM))
        .text_color(ui::secondary_text())
        .child(div().flex_none().child("Install with"))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(del_core::typography::MONO_FONT)
                .text_color(ui::primary_text())
                .child(command),
        )
        .child(
            gpui_kit::component::clipboard::Clipboard::new(SharedString::from(format!(
                "install-{name}"
            )))
            .value(command)
            .tooltip("Copy install command")
            .accessibility_label(format!("Copy the {name} install command")),
        )
}

fn provider_row(
    provider: Provider,
    status: Option<&ProviderStatus>,
    state: &Entity<AppState>,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
    let reduced = cx.reduce_motion();
    let checking = s.checking.contains(&provider) || status.is_none();
    let signing_in = s.signin == Some(provider);
    let can_sign_in = provider != Provider::Acp
        && status.is_some_and(|p| p.installed && p.login != LoginState::LoggedIn);
    let add_state = state.clone();
    let sign_state = state.clone();
    let recheck_state = state.clone();
    let (logo, color) = provider_brand(provider);
    let name = providers::display_name(provider);
    let headline = if signing_in {
        "Waiting for sign-in…"
    } else {
        status.map(providers::summary).unwrap_or("Checking…")
    };
    let status_color = if status.is_some_and(providers::ready) {
        crate::appearance::ActionTone::Success.foreground()
    } else {
        ui::secondary_text()
    };
    // Another provider's sign-in owns the Terminal flow until it finishes.
    let other_sign_in = s.signin.is_some_and(|other| other != provider);
    let add_pending = s.account_login_pending(provider);
    h_flex()
        .id(SharedString::from(format!("provider-{name}")))
        .debug_selector(move || format!("provider-{name}"))
        .w_full()
        .min_h(zpx(48.))
        .gap(zpx(ui::SPACE_MD))
        .items_center()
        .child(
            crate::icons::path(logo)
                .with_size(zpx(crate::icons::MARK_SIZE))
                .text_color(rgb(color)),
        )
        .child(
            div()
                .text_size(zpx(TEXT_MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui::primary_text())
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(zpx(TEXT_SM))
                .text_color(status_color)
                .when(checking, |el| {
                    el.child(
                        div()
                            .relative()
                            .w(zpx(94.))
                            .h(zpx(12.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(rgba(0xffffff0a))
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .h_full()
                                    .w(zpx(30.))
                                    .rounded_full()
                                    .bg(rgba(0xffffff20))
                                    .with_animation(
                                        "provider-shimmer",
                                        Animation::new(std::time::Duration::from_millis(1100))
                                            .repeat(),
                                        move |strip, t| {
                                            // Reduce Motion keeps a static placeholder.
                                            strip.left(zpx(if reduced {
                                                0.
                                            } else {
                                                -30. + 124. * t
                                            }))
                                        },
                                    ),
                            ),
                    )
                })
                .when(!checking, |el| el.child(div().truncate().child(headline))),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(zpx(ui::SPACE_SM))
                .items_center()
                .when(provider != Provider::Acp, |row| {
                    row.child(
                        Button::new(SharedString::from(format!("add-account-{name}")))
                            .ghost()
                            .small()
                            .child(crate::icons::icon(IconName::Plus))
                            .tooltip(if add_pending {
                                FINISH_SIGN_IN
                            } else {
                                "Add account"
                            })
                            .disabled(add_pending)
                            .accessibility_label(format!("Add {name} account"))
                            .on_click(move |_, _, cx| {
                                add_state.update(cx, |s, cx| s.add_account(provider, cx))
                            }),
                    )
                })
                // The headline already says "Waiting for sign-in…"; one quiet
                // spinner replaces the button. The login itself runs in Terminal,
                // where Ctrl-C cancels it.
                .when(can_sign_in && signing_in, |row| {
                    row.child(
                        div()
                            .size(crate::icons::button_size())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(ui::secondary_text())
                            .child(crate::icons::spinner()),
                    )
                })
                .when(can_sign_in && !signing_in, |row| {
                    row.child(
                        Button::new("sign-in")
                            .small()
                            .primary()
                            .label("Sign in")
                            .when(other_sign_in, |button| button.tooltip(FINISH_SIGN_IN))
                            .disabled(checking || s.signin.is_some())
                            .on_click(move |_, _, cx| {
                                sign_state.update(cx, |s, cx| s.sign_in(provider, cx))
                            }),
                    )
                })
                .child(
                    Button::new("recheck")
                        .ghost()
                        .size(crate::icons::button_size())
                        .p_0()
                        .text_color(ui::secondary_text())
                        .child(if checking {
                            crate::icons::spinner().into_any_element()
                        } else {
                            crate::icons::icon(gpui_kit::assets::IconName::RefreshCw)
                                .into_any_element()
                        })
                        .tooltip("Refresh provider status")
                        .accessibility_label("Refresh provider status")
                        .disabled(checking)
                        .on_click(move |_, _, cx| {
                            recheck_state.update(cx, |s, cx| s.recheck_provider(provider, cx))
                        }),
                ),
        )
}

/// Category navigation replaces session navigation only while Settings is open.
pub fn navigation(state: &Entity<AppState>, cx: &App) -> Div {
    let section = state.read(cx).settings_section;
    v_flex()
        .size_full()
        .min_w_0()
        .pl(zpx(ui::SPACE_SM))
        .py(zpx(ui::SPACE_MD))
        .child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap(zpx(ui::SPACE_2XS))
                .child(
                    ui::section_label("Settings")
                        .px(zpx(ui::SPACE_SM))
                        .pb(zpx(ui::SPACE_SM)),
                )
                .children(
                    [
                        (
                            "Appearance",
                            SettingsSection::Appearance,
                            crate::icons::icon(IconName::Palette),
                        ),
                        (
                            "Sessions",
                            SettingsSection::Sessions,
                            crate::icons::icon(IconName::GalleryVerticalEnd),
                        ),
                        (
                            "Providers",
                            SettingsSection::Providers,
                            crate::icons::icon(IconName::CircleUser),
                        ),
                        (
                            "Keybinds",
                            SettingsSection::Keybinds,
                            crate::icons::path("icons/keyboard.svg"),
                        ),
                    ]
                    .into_iter()
                    .map(|(label, category, icon)| {
                        let state = state.clone();
                        let selected = section == category;
                        // The sidebar's list fills: selected stays put, others take the hover fill.
                        let rest: Hsla = if selected {
                            ui::row_selected().into()
                        } else {
                            transparent_black()
                        };
                        Button::new(label)
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(rest)
                                    .hover(
                                        if selected {
                                            ui::row_selected()
                                        } else {
                                            ui::row_hover()
                                        }
                                        .into(),
                                    )
                                    .active(
                                        if selected {
                                            ui::row_selected()
                                        } else {
                                            ui::row_pressed()
                                        }
                                        .into(),
                                    )
                                    .foreground(ui::primary_text().into())
                                    .shadow(false),
                            )
                            .border_0()
                            .w_full()
                            .h(zpx(ui::ROW_SIDEBAR))
                            .px(zpx(ui::SPACE_SM))
                            .rounded(zpx(ui::RADIUS_ROW))
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap(zpx(ui::SPACE_SM))
                                    .child(icon)
                                    .child(label),
                            )
                            .selected(selected)
                            .accessibility_label(format!("Settings: {label}"))
                            .on_click(move |_, _, cx| {
                                state.update(cx, |state, cx| {
                                    state.settings_section = category;
                                    cx.notify();
                                })
                            })
                    }),
                ),
        )
}

fn theme_swatches(theme: crate::appearance::ThemePreset, light: bool) -> impl IntoElement {
    h_flex()
        .gap(zpx(ui::SPACE_XS))
        .flex_none()
        .children(theme.swatches(light).into_iter().map(|color| {
            div()
                .size(zpx(12.))
                .rounded_full()
                .bg(gpui_kit::rgb(color))
                .border_1()
                .border_color(crate::appearance::separator())
        }))
}

fn appearance_controls() -> impl IntoElement {
    use crate::appearance::{self, ThemePreset};
    let theme = appearance::preset();
    let light = appearance::light();
    let animated = appearance::background() != appearance::BackgroundAnimation::Off;
    settings_card()
        .child(
            settings_row("Color mode", None).child(
                h_flex().flex_none().gap(zpx(ui::SPACE_XS)).children(
                    [("Light", true), ("Dark", false)]
                        .into_iter()
                        .map(|(label, mode)| {
                            Button::new(label)
                                .ghost()
                                .small()
                                .label(label)
                                .when(light != mode, |button| {
                                    button.text_color(appearance::secondary_text())
                                })
                                .when(light == mode, |button| button.bg(rgba(0xffffff20)))
                                .on_click(move |_, _, cx| appearance::set(mode, cx))
                        }),
                ),
            ),
        )
        .child(settings_divider())
        .child(
            settings_row("Theme", Some(theme.description().into())).child(
                Button::new("appearance-theme")
                    .ghost()
                    .small()
                    .accessibility_label(format!("Theme: {}", theme.name()))
                    .child(
                        h_flex()
                            .gap(zpx(ui::SPACE_SM))
                            .items_center()
                            .child(theme_swatches(theme, light))
                            .child(theme.name())
                            .child(
                                crate::icons::icon(IconName::ChevronDown)
                                    .with_size(zpx(11.))
                                    .text_color(ui::faint_text()),
                            ),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        ThemePreset::ALL.into_iter().fold(
                            menu.scrollable(true).max_h(zpx(360.)),
                            |menu, option| {
                                menu.item(
                                    PopupMenuItem::element(move |_, _| {
                                        h_flex()
                                            .id(option.name())
                                            .role(Role::MenuItem)
                                            .aria_label(option.name())
                                            .min_w(zpx(210.))
                                            .py(zpx(ui::SPACE_XS))
                                            .gap(zpx(ui::SPACE_MD))
                                            .items_center()
                                            .child(theme_swatches(option, light))
                                            .child(option.name())
                                    })
                                    .checked(option == theme)
                                    .on_click(move |_, _, cx| appearance::set_preset(option, cx)),
                                )
                            },
                        )
                    }),
            ),
        )
        .child(settings_divider())
        .child(
            settings_row("Background animation", None).child(
                Button::new("appearance-background")
                    .ghost()
                    .small()
                    .accessibility_label(format!(
                        "Background animation: {}",
                        appearance::background().name()
                    ))
                    .child(
                        h_flex()
                            .gap(zpx(ui::SPACE_SM))
                            .items_center()
                            .child(appearance::background().name())
                            .child(
                                crate::icons::icon(IconName::ChevronDown)
                                    .with_size(zpx(11.))
                                    .text_color(ui::faint_text()),
                            ),
                    )
                    .dropdown_menu(|menu, _, _| {
                        appearance::BackgroundAnimation::ALL.into_iter().fold(
                            menu,
                            |menu, choice| {
                                menu.item(
                                    PopupMenuItem::new(choice.name())
                                        .checked(choice == appearance::background())
                                        .on_click(move |_, _, cx| {
                                            appearance::set_background(choice, cx)
                                        }),
                                )
                            },
                        )
                    }),
            ),
        )
        .child(
            settings_row(
                div()
                    .when(!animated, |label| {
                        label.text_color(appearance::secondary_text())
                    })
                    .child("Mouse reactive"),
                None,
            )
            .child(
                gpui_kit::component::switch::Switch::new("background-mouse-reactive")
                    // With nothing animating it reads as off; the saved choice returns
                    // when an animation is picked again.
                    .checked(animated && appearance::mouse_reactive())
                    .disabled(!animated)
                    .accessibility_label("Mouse reactive background")
                    .on_click(|enabled, _, cx| appearance::set_mouse_reactive(*enabled, cx)),
            ),
        )
}

/// An account under its provider: name and meter, then its actions.
fn account_row(label: impl IntoElement, remaining: Option<f64>) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .pl(zpx(crate::icons::MARK_SIZE + ui::SPACE_MD))
        .py(zpx(ui::SPACE_SM))
        .gap(zpx(ui::SPACE_SM))
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(zpx(ui::SPACE_XS))
                .child(label)
                .child(usage_bar(remaining)),
        )
}

fn account_name(name: String, default: bool) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_size(zpx(TEXT_SM))
        .when(default, |label| {
            label.text_color(crate::appearance::ActionTone::Success.foreground())
        })
        .child(name)
}

fn provider_card(provider: Provider, state: &Entity<AppState>, cx: &App) -> Div {
    let s = state.read(cx);
    let status = s.status_of(provider);
    let pending = s.account_login_pending(provider);
    let install = status
        .filter(|status| !status.installed)
        .and_then(|_| providers::install_command(provider));
    settings_card()
        .child(provider_row(provider, status, state, cx))
        .when_some(install, |card, command| {
            card.child(install_hint(provider, command))
        })
        .when(provider != Provider::Acp, |card| {
            let system_default = s.default_account(provider).is_some();
            let state = state.clone();
            card.child(settings_divider()).child(
                account_row(
                    account_name(s.default_account_label(provider), !system_default),
                    s.account_usage_remaining(provider, None),
                )
                .when(system_default, |row| {
                    row.child(
                        Button::new(SharedString::from(format!("system-default-{provider:?}")))
                            .ghost()
                            .small()
                            .label("Make default")
                            .when(pending, |button| button.tooltip(FINISH_SIGN_IN))
                            .disabled(pending)
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.make_default_account(provider, None, cx))
                            }),
                    )
                }),
            )
        })
        .children(s.visible_accounts(provider).into_iter().map(|account| {
            let id = account.id.clone();
            let reconnect = state.clone();
            let remove = state.clone();
            let default = state.clone();
            let remove_id = id.clone();
            let default_id = id.clone();
            let needs_login = s.account_needs_login(&id);
            account_row(
                account_name(account.display(), account.is_default),
                s.account_usage_remaining(provider, Some(&id)),
            )
            .when(needs_login, |row| {
                row.child(
                    Button::new(SharedString::from(format!("account-login-{id}")))
                        .ghost()
                        .small()
                        .label("Sign in")
                        .when(pending, |button| button.tooltip(FINISH_SIGN_IN))
                        .disabled(pending)
                        .on_click(move |_, _, cx| {
                            reconnect.update(cx, |s, cx| s.login_account(&id, cx))
                        }),
                )
            })
            .when(!account.is_default, |row| {
                row.child(
                    Button::new(SharedString::from(format!("account-default-{default_id}")))
                        .ghost()
                        .small()
                        .label("Make default")
                        .when(pending || needs_login, |button| {
                            button.tooltip(if pending {
                                FINISH_SIGN_IN
                            } else {
                                "Sign in to this account first"
                            })
                        })
                        .disabled(needs_login || pending)
                        .on_click(move |_, _, cx| {
                            default.update(cx, |s, cx| {
                                s.make_default_account(provider, Some(default_id.clone()), cx)
                            })
                        }),
                )
            })
            .child(
                Button::new(SharedString::from(format!("account-delete-{remove_id}")))
                    .ghost()
                    .size(crate::icons::button_size())
                    .flex_none()
                    .p_0()
                    .child(
                        crate::icons::icon(gpui_kit::assets::IconName::Trash)
                            .text_color(crate::appearance::ActionTone::Danger.foreground()),
                    )
                    .tooltip(if pending {
                        FINISH_SIGN_IN
                    } else {
                        "Delete account"
                    })
                    .accessibility_label("Delete account")
                    .disabled(pending)
                    .on_click(move |_, window, cx| {
                        let removed = remove.update(cx, |s, cx| {
                            s.delete_account(&remove_id, cx);
                            !s.accounts.iter().any(|account| account.id == remove_id)
                        });
                        // Deleting is reversible for a while; say so instead of vanishing.
                        if removed {
                            window.push_notification(
                                gpui_kit::component::notification::Notification::info(
                                    account_removed_note(),
                                ),
                                cx,
                            );
                        }
                    }),
            )
        }))
}

/// Removed profiles stay in recovery storage until startup purges them.
fn account_removed_note() -> String {
    let days = crate::accounts::REMOVED_RETENTION.as_secs() / 86_400;
    format!(
        "Account removed — kept in recovery for {}",
        ui::plural(days as usize, "day", "days")
    )
}

pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let section = s.settings_section;
    let preferences = section == SettingsSection::Appearance;
    let back = state.clone();
    let scroll = s.settings_scroll.clone();
    let fades =
        crate::tab_transfer::vertical_edge_fades(&scroll, crate::appearance::popup_surface());
    let title = match section {
        SettingsSection::Appearance => "Appearance",
        SettingsSection::Providers => "Providers",
        SettingsSection::Sessions => "Sessions",
        SettingsSection::Keybinds => "Keybinds",
    };
    let done = div()
        .id("settings-done-tip")
        .flex_none()
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Back to chat")
                .key_binding(
                    Keystroke::parse("escape")
                        .ok()
                        .map(gpui_kit::component::kbd::Kbd::new),
                )
                .build(window, cx)
        })
        .child(
            Button::new("settings-done")
                .ghost()
                .small()
                .bg(rgba(crate::appearance::QUIET_FILL))
                .label("Done")
                .on_click(move |_, _, cx| back.update(cx, |state, cx| state.toggle_providers(cx))),
        );
    let content = v_flex()
        .w_full()
        .min_w_0()
        .max_w(zpx(640.))
        .gap(zpx(ui::SPACE_LG))
        .when(preferences, |content| {
            let state = state.clone();
            content.child(appearance_controls()).child(
                // A plain row like Theme, aligned with the card's content and without its background.
                settings_row(
                    "Global session tabs",
                    Some("Keep the same top tabs when switching Spaces.".into()),
                )
                .px(zpx(ui::SPACE_LG))
                .child(
                    gpui_kit::component::switch::Switch::new("global-space-tabs")
                        .checked(s.spaces.global_tabs)
                        .accessibility_label("Global session tabs")
                        .on_click(move |checked, _, cx| {
                            state.update(cx, |s, cx| s.set_global_space_tabs(*checked, cx))
                        }),
                ),
            )
        })
        .when(section == SettingsSection::Sessions, |content| {
            content.child(
                settings_card().child(
                    settings_row(
                        "Keep sessions running when the window closes",
                        Some(
                            "Closing the window keeps chats, Side chats, and terminals running. Open Del to return. Quit Del stops the app."
                                .into(),
                        ),
                    )
                    .child(
                        gpui_kit::component::switch::Switch::new("keep-sessions-running")
                            .checked(crate::session_lifecycle::keep_running())
                            .accessibility_label("Keep sessions running when the window closes")
                            .on_click(|enabled, window, cx| {
                                if let Err(error) =
                                    crate::session_lifecycle::set_keep_running(*enabled)
                                {
                                    window.push_notification(
                                        gpui_kit::component::notification::Notification::error(
                                            format!("Could not save preference: {error}"),
                                        ),
                                        cx,
                                    );
                                }
                                cx.refresh_windows();
                            }),
                    ),
                ),
            )
        })
        .when(section == SettingsSection::Keybinds, |content| {
            content.child(crate::keybinds::render())
        })
        .when(section == SettingsSection::Providers, |content| {
            content
                .children(
                    providers::VISIBLE
                        .into_iter()
                        .map(|provider| provider_card(provider, state, cx)),
                )
                .child(
                    div()
                        .text_size(zpx(TEXT_SM))
                        .text_color(ui::secondary_text())
                        .child(
                            "Sign in opens your provider’s login in Terminal. Status updates automatically.",
                        ),
                )
        });
    div()
        .relative()
        .size_full()
        .min_w_0()
        .overflow_hidden()
        .child(
            v_flex()
                .id("settings")
                .track_scroll(&scroll)
                .size_full()
                .min_w_0()
                .overflow_y_scroll()
                .p(zpx(ui::SPACE_XL))
                .gap(zpx(ui::SPACE_XL))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap(zpx(ui::SPACE_LG))
                        .child(
                            div()
                                .text_size(zpx(del_core::typography::TEXT_XL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::primary_text())
                                .child(title),
                        )
                        .child(done),
                )
                .child(content),
        )
        .children(fades)
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use ::core::prelude::v1::test;

    struct Fixture(Entity<AppState>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(zpx(570.)).h(zpx(420.)).child(render(&self.0, cx))
        }
    }

    #[::gpui::test]
    fn provider_settings_keep_controls_inside_the_modal_with_long_account_names(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        state.update(cx, |state, _| {
            state.settings_section = SettingsSection::Providers;
            state.providers = providers::VISIBLE
                .into_iter()
                .map(|provider| del_agent::ProviderStatus {
                    provider,
                    installed: true,
                    login: LoginState::LoggedIn,
                    path: None,
                    version: None,
                })
                .collect();
            for provider in providers::VISIBLE {
                state.default_identities.insert(
                    provider,
                    format!("{}@example.test", "long-account-".repeat(20)),
                );
            }
        });
        let (_root, cx) = cx.add_window_view(|_, _| Fixture(state));
        cx.run_until_parked();
        for selector in ["provider-Codex", "provider-Claude", "provider-OpenCode"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            assert!(bounds.left() >= zpx(0.));
            assert!(bounds.right() <= zpx(570.));
            assert!(bounds.size.height >= zpx(48.));
        }
    }

    #[::gpui::test]
    fn missing_providers_offer_their_install_command(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        state.update(cx, |state, _| {
            state.settings_section = SettingsSection::Providers;
            state.providers = providers::VISIBLE
                .into_iter()
                .map(|provider| del_agent::ProviderStatus {
                    provider,
                    installed: provider != Provider::Claude,
                    login: LoginState::LoggedIn,
                    path: None,
                    version: None,
                })
                .collect();
        });
        let (_root, cx) = cx.add_window_view(|_, _| Fixture(state));
        cx.run_until_parked();
        let hint = cx.debug_bounds("install-hint-Claude").unwrap();
        assert!(hint.right() <= zpx(570.));
        assert!(cx.debug_bounds("install-hint-Codex").is_none());
        assert_eq!(
            account_removed_note(),
            "Account removed — kept in recovery for 30 days"
        );
    }
}

#[cfg(test)]
mod settings_scroll_tests {
    use ::core::prelude::v1::test;
    use ::gpui::{
        AppContext as _, Context, InteractiveElement, IntoElement, Modifiers, ParentElement,
        Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled,
        Window, div, point, px,
    };

    struct Fixture;
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A Settings page is taller than its dialog, so the wheel must reach its own scroller.
    #[::gpui::test]
    fn a_long_settings_page_scrolls_inside_its_dialog(cx: &mut ::gpui::TestAppContext) {
        use gpui_kit::component::WindowExt as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Fixture);
            gpui_kit::component::Root::new(view, window, cx)
        });
        let scroll = ScrollHandle::new();
        let page = scroll.clone();
        cx.update(|window, cx| {
            window.open_dialog(cx, move |dialog, _, _| {
                dialog.p_0().close_button(false).child(
                    div()
                        .h(px(300.))
                        .w_full()
                        .debug_selector(|| "page".into())
                        .child(
                            div()
                                .id("settings")
                                .track_scroll(&page)
                                .size_full()
                                .overflow_y_scroll()
                                .child(div().h(px(2000.)).w_full()),
                        ),
                )
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("page").unwrap().center();
        cx.simulate_mouse_move(target, None, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent {
            position: target,
            delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(scroll.offset().y, px(-200.));
    }
}
