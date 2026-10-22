//! Providers panel: for Codex and Claude, whether the CLI is installed, its
//! version and whether the user is signed in, with a Sign in button that
//! launches the CLI's own login in the user's terminal.
//!
//! del only runs the CLI's version and status commands. It never reads or
//! stores tokens.

use crate::providers;
use crate::state::AppState;
use del_agent::{LoginState, Provider, ProviderStatus};
use del_core::typography::{TEXT_LG, TEXT_MD, TEXT_SM};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const MUTED: u32 = 0xffffff8c;

fn pill(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px_2p5()
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(TEXT_MD))
        .when(primary, |p| p.bg(rgb(0xffffff)).text_color(rgb(0x111113)))
        .when(!primary, |p| {
            p.bg(rgba(0xffffff14))
                .text_color(rgb(0xf0f0f0))
                .hover(|p| p.bg(rgba(0xffffff22)))
        })
        .child(label)
}

fn provider_card(
    status: &ProviderStatus,
    signing_in: bool,
    state: &Entity<AppState>,
) -> impl IntoElement {
    let provider = status.provider;
    let headline = providers::summary(status);
    let dot = match (status.installed, status.login) {
        (true, _) if provider == Provider::Acp => 0x4cc38a,
        (false, _) => 0xf0524f,
        (true, LoginState::LoggedIn) => 0x4cc38a,
        (true, LoginState::LoggedOut) => 0xf5a524,
        (true, LoginState::Unknown) => 0x6e6e73,
    };
    let name = providers::display_name(provider);
    let sign_state = state.clone();
    let recheck_state = state.clone();

    let detail: String = if !status.installed {
        providers::install_hint(provider).to_string()
    } else {
        let version = status
            .version
            .clone()
            .unwrap_or_else(|| "unknown version".into());
        let path = status
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if provider == Provider::Acp {
            version
        } else {
            format!("{version}  ·  {path}")
        }
    };

    let instructions: Option<String> = if provider == Provider::Acp {
        Some("Choose an installed ACP agent in the composer. Sign in through that agent’s CLI if it asks.".into())
    } else {
        match (status.installed, status.login) {
            (true, LoginState::LoggedOut) => Some(format!(
                "Click Sign in to run `{}` in Terminal, finish in your browser, then come back. del checks again automatically.",
                providers::login_command(provider)
            )),
            (true, LoginState::Unknown) => Some(format!(
                "del could not read the sign-in state. Try `{}` in a terminal, then re-check.",
                providers::login_command(provider)
            )),
            _ => None,
        }
    };

    v_flex()
        .w_full()
        .p_3()
        .gap_1p5()
        .rounded(px(14.0))
        .bg(rgba(0xffffff12))
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(div().size(px(7.0)).rounded_full().bg(rgb(dot)))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(TEXT_LG))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xf5f5f5))
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(TEXT_MD))
                        .text_color(rgba(MUTED))
                        .child(headline),
                ),
        )
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(rgba(0xffffff66))
                .child(detail),
        )
        .when_some(instructions, |c, text| {
            c.child(
                div()
                    .text_size(px(TEXT_MD))
                    .text_color(rgba(0xffffffb3))
                    .child(text),
            )
        })
        .child(
            h_flex()
                .gap_1()
                .pt_1()
                .when(
                    provider != Provider::Acp
                        && status.installed
                        && status.login != LoginState::LoggedIn,
                    |row| {
                        let label = if signing_in {
                            "Waiting for sign-in…"
                        } else {
                            "Sign in"
                        };
                        row.child(
                            pill(
                                match provider {
                                    Provider::Codex => "signin-codex",
                                    Provider::Claude => "signin-claude",
                                    Provider::Acp => "signin-acp",
                                },
                                label,
                                !signing_in,
                            )
                            .on_click(move |_, _, cx| {
                                sign_state.update(cx, |s, cx| s.sign_in(provider, cx))
                            }),
                        )
                    },
                )
                .child(
                    pill(
                        match provider {
                            Provider::Codex => "recheck-codex",
                            Provider::Claude => "recheck-claude",
                            Provider::Acp => "recheck-acp",
                        },
                        "Re-check",
                        false,
                    )
                    .on_click(move |_, _, cx| recheck_state.update(cx, |s, cx| s.recheck(cx))),
                ),
        )
}

pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .px_3()
        .child(
            v_flex()
                .w_full()
                .max_w(px(560.0))
                .gap_2()
                .child(
                    div()
                        .pl(px(17.5))
                        .pb_1()
                        .text_size(px(TEXT_LG))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(0xf5f5f5))
                        .child("Providers"),
                )
                .children(
                    s.providers
                        .iter()
                        .map(|p| provider_card(p, s.signin == Some(p.provider), state)),
                )
                .child(
                    div()
                        .pl(px(17.5))
                        .pt_1()
                        .text_size(px(TEXT_SM))
                        .text_color(rgba(0xffffff52))
                        .child("Sign-in is handled by each CLI. del never sees or stores your credentials."),
                ),
        )
}
