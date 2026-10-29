//! Compact provider setup rows. Credentials remain managed by the provider CLI.
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use crate::{composer_ui::provider_brand, providers, state::AppState};
use del_agent::{LoginState, Provider, ProviderStatus};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Providers,
    Keybinds,
}

fn usage_bar(remaining: Option<f64>) -> impl IntoElement {
    div()
        .w_full()
        .max_w(zpx(240.))
        .children(remaining.map(|remaining| {
            h_flex()
                .w_full()
                .gap_2()
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
                        .text_size(zpx(11.))
                        .text_color(rgba(0xffffff80))
                        .child(format!("{remaining:.0}% left")),
                )
        }))
}

fn provider_row(
    provider: Provider,
    status: Option<&ProviderStatus>,
    state: &Entity<AppState>,
    cx: &App,
) -> impl IntoElement {
    let s = state.read(cx);
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
        0x4cc38a
    } else {
        0x99999f
    };
    h_flex()
        .id(SharedString::from(format!("provider-{name}")))
        .w_full()
        .min_h(zpx(58.))
        .px_3()
        .gap_3()
        .items_center()
        .rounded(zpx(14.))
        .bg(if crate::appearance::light() {
            gpui_kit::rgb(0xf9f9f9)
        } else {
            gpui_kit::rgb(0x171719)
        })
        .child(crate::icons::path(logo).text_color(rgb(color)))
        .child(
            div()
                .text_size(zpx(TEXT_MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xf5f5f5))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(zpx(TEXT_SM))
                .text_color(rgb(status_color))
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
                                        |strip, t| strip.left(zpx(-30. + 124. * t)),
                                    ),
                            ),
                    )
                })
                .when(!checking, |el| el.child(div().truncate().child(headline))),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .when(provider != Provider::Acp, |row| {
                    row.child(
                        Button::new(SharedString::from(format!("add-account-{name}")))
                            .ghost()
                            .small()
                            .child(crate::icons::icon(IconName::Plus))
                            .tooltip("Add account")
                            .disabled(s.account_login_pending(provider))
                            .accessibility_label(format!("Add {name} account"))
                            .on_click(move |_, _, cx| {
                                add_state.update(cx, |s, cx| s.add_account(provider, cx))
                            }),
                    )
                })
                .when(can_sign_in, |row| {
                    row.child(
                        Button::new("sign-in")
                            .small()
                            .h(zpx(28.))
                            .px_3()
                            .rounded_full()
                            .primary()
                            .label(if signing_in {
                                "Signing in…"
                            } else {
                                "Sign in"
                            })
                            .when(signing_in, |button| button.child(crate::icons::spinner()))
                            .disabled(checking || s.signin.is_some())
                            .on_click(move |_, _, cx| {
                                sign_state.update(cx, |s, cx| s.sign_in(provider, cx))
                            }),
                    )
                })
                .child(
                    Button::new("recheck")
                        .size(crate::icons::button_size())
                        .p_0()
                        .rounded_full()
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
        .pl_2()
        .py(zpx(8.))
        .gap_4()
        .child(
            v_flex()
                .w_full()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .text_size(zpx(TEXT_SM))
                        .text_color(rgba(0xffffff80))
                        .child("Settings"),
                )
                .children(
                    [
                        ("Appearance", SettingsSection::Appearance, IconName::Palette),
                        (
                            "Providers",
                            SettingsSection::Providers,
                            IconName::CircleUser,
                        ),
                        ("Keybinds", SettingsSection::Keybinds, IconName::Settings),
                    ]
                    .into_iter()
                    .map(|(label, category, icon)| {
                        let state = state.clone();
                        Button::new(label)
                            .ghost()
                            .w_full()
                            .h(zpx(34.))
                            .px_2()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .child(crate::icons::icon(icon))
                                    .child(label),
                            )
                            .selected(section == category)
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

pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let section = s.settings_section;
    let preferences = section == SettingsSection::Appearance;
    let back = state.clone();
    v_flex().id("settings").size_full().min_w_0().overflow_y_scroll().p_6().gap_5()
        .child(h_flex().w_full().items_center().justify_between().gap_4()
            .child(div().text_size(zpx(24.)).font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xf5f5f5)).child(match section { SettingsSection::Appearance => "Appearance", SettingsSection::Providers => "Providers", SettingsSection::Keybinds => "Keybinds" }))
            .child(Button::new("settings-done").ghost().small().label("Done")
                .tooltip("Back to chat (Esc)")
                .on_click(move |_, _, cx| back.update(cx, |state, cx| state.toggle_providers(cx)))))
        .child(v_flex().w_full().min_w_0().max_w(zpx(640.)).gap_4()
            .when(preferences, |content| content
                .child(h_flex().w_full().p_4().gap_4().items_center().rounded(zpx(14.)).bg(if crate::appearance::light() { gpui_kit::rgb(0xf9f9f9) } else { gpui_kit::rgb(0x171719) })
                    .child(v_flex().flex_1().min_w_0().gap_1()
                        .child(div().text_color(rgb(0xf5f5f5)).child("Theme"))
                        .child(div().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff8c)).child("Choose a theme for Del.")))
                    .children([("Light", true), ("Dark", false)].into_iter().map(|(label, light)| {
                        Button::new(label).ghost().small().label(label)
                            .when(crate::appearance::light() == light, |button| button.bg(rgba(0xffffff20)))
                            .on_click(move |_, _, cx| crate::appearance::set(light, cx))
                    }))))
            .when(section == SettingsSection::Keybinds, |content| content.child(crate::keybinds::render()))
            .when(section == SettingsSection::Providers, |content| content
                .children(providers::VISIBLE.into_iter().map(|provider| {
                    v_flex().gap_1().child(provider_row(provider, s.status_of(provider), state, cx))
                        .when(provider != Provider::Acp, |rows| rows.child(
                            h_flex().pl(zpx(38.)).pr_3().py_2().gap_2().items_center()
                                .child(v_flex().flex_1().min_w_0().gap_1()
                                    .child(h_flex().gap_2().items_center()
                                        .child(div().min_w_0().truncate().text_size(zpx(TEXT_SM))
                                            .when(s.default_account(provider).is_none(), |label| label.text_color(rgb(0x4cc38a)))
                                            .child(s.default_account_label(provider))))
                                    .child(usage_bar(s.account_usage_remaining(provider, None))))
                                .when(s.default_account(provider).is_some(), |row| {
                                    let state = state.clone();
                                    row.child(Button::new(SharedString::from(format!("system-default-{provider:?}"))).ghost().small().label("Make default")
                                        .disabled(s.account_login_pending(provider))
                                        .on_click(move |_, _, cx| state.update(cx, |s, cx| s.make_default_account(provider, None, cx))))
                                })))
                        .children(s.visible_accounts(provider).into_iter().map(|account| {
                            let id = account.id.clone();
                            let reconnect = state.clone();
                            let remove = state.clone();
                            let default = state.clone();
                            let remove_id = id.clone();
                            let default_id = id.clone();
                            let needs_login = s.account_needs_login(&id);
                            h_flex().pl(zpx(38.)).pr_3().py_2().gap_2().items_center()
                                .child(v_flex().flex_1().min_w_0().gap_1()
                                    .child(h_flex().gap_2().items_center()
                                        .child(div().min_w_0().truncate().text_size(zpx(TEXT_SM))
                                            .when(account.is_default, |label| label.text_color(rgb(0x4cc38a)))
                                            .child(account.display())))
                                    .child(usage_bar(s.account_usage_remaining(provider, Some(&id)))))
                                .when(needs_login, |row| row.child(Button::new(SharedString::from(format!("account-login-{id}"))).ghost().small().label("Sign in")
                                    .disabled(s.account_login_pending(provider))
                                    .on_click(move |_, _, cx| reconnect.update(cx, |s, cx| s.login_account(&id, cx)))))
                                .when(!account.is_default, |row| row.child(Button::new(SharedString::from(format!("account-default-{default_id}"))).ghost().small().label("Make default")
                                    .disabled(needs_login || s.account_login_pending(provider))
                                    .on_click(move |_, _, cx| default.update(cx, |s, cx| s.make_default_account(provider, Some(default_id.clone()), cx)))))
                                .child(Button::new(SharedString::from(format!("account-delete-{remove_id}"))).ghost().size(crate::icons::button_size()).flex_none().p_0()
                                    .child(crate::icons::icon(gpui_kit::assets::IconName::Trash).text_color(rgb(0xff646b)))
                                    .tooltip("Delete account").accessibility_label("Delete account")
                                    .disabled(s.account_login_pending(provider))
                                    .on_click(move |_, _, cx| remove.update(cx, |s, cx| s.delete_account(&remove_id, cx))))
                        }))
                }))
                .child(div().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff8c))
                    .child("Sign in opens your provider’s login in Terminal. Status updates automatically."))))
}
