//! Compact provider setup rows. Credentials remain managed by the provider CLI.
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use crate::{composer_ui::provider_brand, providers, state::AppState};
use del_agent::{LoginState, Provider, ProviderStatus};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    *,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

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
        .bg(rgba(0xffffff12))
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
    let preferences = state.read(cx).preferences_open;
    let back = state.clone();
    v_flex().size_full().min_w_0().pl_2().py(zpx(8.)).gap_4()
        .child(Button::new("settings-back").ghost().w_full().h(zpx(34.)).px_2()
            .child(crate::icons::icon(IconName::ArrowLeft)).label("Back to chat")
            .accessibility_label("Back to chat")
            .on_click(move |_, _, cx| back.update(cx, |state, cx| state.toggle_providers(cx))))
        .child(v_flex().w_full().min_w_0().gap_1()
            .child(div().px_2().pb_2().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff80)).child("Settings"))
            .children([("Appearance", true, IconName::Palette), ("Accounts", false, IconName::CircleUser)]
                .into_iter().map(|(label, category, icon)| {
                    let state = state.clone();
                    Button::new(label).ghost().w_full().h(zpx(34.)).px_2()
                        .child(crate::icons::icon(icon)).label(label)
                        .selected(preferences == category)
                        .accessibility_label(format!("Settings: {label}"))
                        .on_click(move |_, _, cx| state.update(cx, |state, cx| {
                            state.preferences_open = category;
                            cx.notify();
                        }))
                })))
}

pub fn render(
    state: &Entity<AppState>,
    opacity: &Entity<gpui_kit::component::slider::SliderState>,
    cx: &App,
) -> impl IntoElement {
    use gpui_kit::component::slider::Slider;
    let s = state.read(cx);
    let preferences = s.preferences_open;
    let back = state.clone();
    v_flex().id("settings").size_full().min_w_0().overflow_y_scroll().p_6().gap_5()
        .child(h_flex().w_full().items_center().justify_between().gap_4()
            .child(div().text_size(zpx(24.)).font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xf5f5f5)).child("Settings"))
            .child(Button::new("settings-done").ghost().small().label("Done")
                .tooltip("Back to chat (Esc)")
                .on_click(move |_, _, cx| back.update(cx, |state, cx| state.toggle_providers(cx)))))
        .child(v_flex().w_full().min_w_0().max_w(zpx(640.)).gap_4()
            .child(div().text_size(zpx(TEXT_LG)).text_color(rgb(0xf5f5f5))
                .child(if preferences { "Appearance" } else { "Accounts" }))
            .when(preferences, |content| content
                .child(h_flex().w_full().p_4().gap_4().items_center().rounded(zpx(14.)).bg(rgba(0xffffff12))
                    .child(v_flex().flex_1().min_w_0().gap_1()
                        .child(div().text_color(rgb(0xf5f5f5)).child("Theme"))
                        .child(div().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff8c)).child("Choose a theme for Del.")))
                    .children([("Light", true), ("Dark", false)].into_iter().map(|(label, light)| {
                        Button::new(label).ghost().small().label(label)
                            .when(crate::appearance::light() == light, |button| button.bg(rgba(0xffffff20)))
                            .on_click(move |_, _, cx| crate::appearance::set(light, cx))
                    })))
                .child(v_flex().w_full().p_4().gap_4().rounded(zpx(14.)).bg(rgba(0xffffff12))
                    .child(h_flex().w_full().items_center().justify_between().gap_4()
                        .child(v_flex().flex_1().min_w_0().gap_1()
                            .child(div().text_color(rgb(0xf5f5f5)).child("Opacity"))
                            .child(div().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff8c))
                                .child("Higher values make the window less see-through.")))
                        .child(div().flex_none().text_color(rgb(0xf5f5f5))
                            .child(format!("{}%", crate::appearance::opacity()))))
                    .child(div().w_full().py_2().child(Slider::new(opacity).horizontal().w_full()
                        .accessibility_label("Window background opacity")
                        .bg(rgb(0x99999f)).text_color(rgb(0xf5f5f5))))
                    .child(h_flex().w_full().justify_between().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff80))
                        .child("40% · More transparent").child("95% · More opaque"))))
            .when(!preferences, |content| content
                .children(Provider::ALL.into_iter().map(|provider| provider_row(provider, s.status_of(provider), state, cx)))
                .child(div().text_size(zpx(TEXT_SM)).text_color(rgba(0xffffff8c))
                    .child("Sign in opens your provider’s login in Terminal. Status updates automatically."))))
}
