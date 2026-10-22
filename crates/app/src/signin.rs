//! Compact provider setup rows. Credentials remain managed by the provider CLI.
use crate::appearance::{rgb, rgba};
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
        .min_h(px(58.))
        .px_3()
        .gap_3()
        .items_center()
        .rounded(px(14.))
        .bg(rgba(0xffffff12))
        .child(Icon::default().path(logo).text_color(rgb(color)).small())
        .child(
            div()
                .text_size(px(TEXT_MD))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xf5f5f5))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(TEXT_SM))
                .text_color(rgb(status_color))
                .when(checking, |el| {
                    el.child(
                        div()
                            .relative()
                            .w(px(94.))
                            .h(px(12.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(rgba(0xffffff0a))
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .h_full()
                                    .w(px(30.))
                                    .rounded_full()
                                    .bg(rgba(0xffffff20))
                                    .with_animation(
                                        "provider-shimmer",
                                        Animation::new(std::time::Duration::from_millis(1100))
                                            .repeat(),
                                        |strip, t| strip.left(px(-30. + 124. * t)),
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
                            .h(px(28.))
                            .px_3()
                            .rounded_full()
                            .primary()
                            .label(if signing_in {
                                "Signing in…"
                            } else {
                                "Sign in"
                            })
                            .loading(signing_in)
                            .disabled(checking || s.signin.is_some())
                            .on_click(move |_, _, cx| {
                                sign_state.update(cx, |s, cx| s.sign_in(provider, cx))
                            }),
                    )
                })
                .child(
                    Button::new("recheck")
                        .small()
                        .h(px(28.))
                        .px_3()
                        .rounded_full()
                        .label(if checking { "Checking…" } else { "Re-check" })
                        .loading(checking)
                        .disabled(checking)
                        .on_click(move |_, _, cx| {
                            recheck_state.update(cx, |s, cx| s.recheck_provider(provider, cx))
                        }),
                ),
        )
}

pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let preferences = s.preferences_open;
    v_flex().id("settings").size_full().overflow_y_scroll().p_6().gap_5()
        .child(div().text_size(px(24.)).font_weight(FontWeight::MEDIUM).text_color(rgb(0xf5f5f5)).child("Settings"))
        .child(h_flex().w_full().gap_6().items_start()
            .child(v_flex().w(px(140.)).flex_none().gap_1().children(
                [("Preferences", true), ("Providers", false)].into_iter().map(|(label, selected)| {
                    let state = state.clone();
                    div().id(label).px_3().py_2().rounded_md().cursor_pointer()
                        .text_color(rgb(0xf5f5f5)).when(preferences == selected, |item| item.bg(rgba(0xffffff14)))
                        .hover(|item| item.bg(rgba(0xffffff0a))).child(label)
                        .on_click(move |_, _, cx| state.update(cx, |s, cx| { s.preferences_open = selected; cx.notify(); }))
                })))
            .child(v_flex().flex_1().min_w_0().max_w(px(600.)).gap_3()
                .child(div().text_size(px(TEXT_LG)).text_color(rgb(0xf5f5f5)).child(if preferences { "Preferences" } else { "Providers" }))
                .when(preferences, |content| content.child(
                    h_flex().w_full().p_4().gap_4().items_center().rounded(px(14.)).bg(rgba(0xffffff12))
                        .child(v_flex().flex_1().gap_1().child(div().text_color(rgb(0xf5f5f5)).child("Appearance"))
                            .child(div().text_size(px(TEXT_SM)).text_color(rgba(0xffffff8c)).child("Choose a theme for Del.")))
                        .children([("Light", true), ("Dark", false)].into_iter().map(|(label, light)| {
                            Button::new(label).ghost().small().label(label)
                                .when(crate::appearance::light() == light, |button| button.bg(rgba(0xffffff20)))
                                .on_click(move |_, _, cx| crate::appearance::set(light, cx))
                        }))))
                .when(!preferences, |content| content
                    .children(Provider::ALL.into_iter().map(|provider| provider_row(provider, s.status_of(provider), state, cx)))
                    .child(div().text_size(px(TEXT_SM)).text_color(rgba(0xffffff8c)).child("Sign in opens your provider’s login in Terminal. Status updates automatically.")))))
}
