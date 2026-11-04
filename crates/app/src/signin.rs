//! Compact provider setup rows. Credentials remain managed by the provider CLI.
use crate::appearance::{rgb, rgba};
use crate::zoom::px as zpx;
use crate::{composer_ui::provider_brand, providers, state::AppState};
use del_agent::{LoginState, Provider, ProviderStatus};
use del_core::typography::{TEXT_MD, TEXT_SM};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
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
                        .text_size(zpx(del_core::typography::TEXT_XS))
                        .text_color(crate::appearance::secondary_text())
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
        crate::appearance::secondary_text()
    };
    h_flex()
        .id(SharedString::from(format!("provider-{name}")))
        .debug_selector(move || format!("provider-{name}"))
        .w_full()
        .min_h(zpx(48.))
        .px_3()
        .gap_2()
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
                .text_color(rgb(0xf5f5f5))
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
                        .ghost()
                        .size(crate::icons::button_size())
                        .p_0()
                        .text_color(rgba(0xffffff8c))
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
                        .text_color(crate::appearance::secondary_text())
                        .child("Settings"),
                )
                .children(
                    [
                        (
                            "Appearance",
                            SettingsSection::Appearance,
                            crate::icons::icon(IconName::Palette),
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
                        Button::new(label)
                            .ghost()
                            .w_full()
                            .h(zpx(30.))
                            .px_2()
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .child(icon)
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

fn theme_swatches(theme: crate::appearance::ThemePreset, light: bool) -> impl IntoElement {
    h_flex()
        .gap_1()
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
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .rounded(zpx(10.))
        .bg(appearance::composer_surface())
        .child(
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().child("Color mode")),
                )
                .children(
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
        )
        .child(div().h(zpx(1.)).bg(appearance::separator()))
        .child(
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().child("Theme"))
                        .child(
                            div()
                                .text_size(zpx(TEXT_SM))
                                .text_color(appearance::secondary_text())
                                .child(theme.description()),
                        ),
                )
                .child(
                    Button::new("appearance-theme")
                        .ghost()
                        .small()
                        .accessibility_label(format!("Theme: {}", theme.name()))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(theme_swatches(theme, light))
                                .child(theme.name())
                                .child(
                                    crate::icons::icon(IconName::ChevronDown)
                                        .with_size(zpx(11.))
                                        .text_color(crate::appearance::secondary_text()),
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
                                                .py_1()
                                                .gap_3()
                                                .items_center()
                                                .child(theme_swatches(option, light))
                                                .child(option.name())
                                        })
                                        .checked(option == theme)
                                        .on_click(
                                            move |_, _, cx| appearance::set_preset(option, cx),
                                        ),
                                    )
                                },
                            )
                        }),
                ),
        )
        .child(div().h(zpx(1.)).bg(appearance::separator()))
        .child(
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .child(div().flex_1().min_w_0().child("Background animation"))
                .child(
                    Button::new("appearance-background")
                        .ghost()
                        .small()
                        .accessibility_label(format!(
                            "Background animation: {}",
                            appearance::background().name()
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(appearance::background().name())
                                .child(
                                    crate::icons::icon(IconName::ChevronDown)
                                        .with_size(zpx(11.))
                                        .text_color(crate::appearance::secondary_text()),
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
            h_flex()
                .w_full()
                .gap_4()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(!animated, |label| {
                            label.text_color(appearance::secondary_text())
                        })
                        .child("Mouse reactive"),
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

pub fn render(state: &Entity<AppState>, cx: &App) -> impl IntoElement {
    let s = state.read(cx);
    let section = s.settings_section;
    let preferences = section == SettingsSection::Appearance;
    let back = state.clone();
    let scroll = s.settings_scroll.clone();
    let fades =
        crate::tab_transfer::vertical_edge_fades(&scroll, crate::appearance::popup_surface());
    div().relative().size_full().min_w_0().overflow_hidden()
        .child(v_flex().id("settings").track_scroll(&scroll).size_full().min_w_0().overflow_y_scroll().p_6().gap_5()
        .child(h_flex().w_full().items_center().justify_between().gap_4()
            .child(div().text_size(zpx(del_core::typography::TEXT_XL)).font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0xf5f5f5)).child(match section { SettingsSection::Appearance => "Appearance", SettingsSection::Providers => "Providers", SettingsSection::Keybinds => "Keybinds" }))
            .child(Button::new("settings-done").ghost().small().bg(rgba(crate::appearance::QUIET_FILL)).label("Done")
                .tooltip("Back to chat (Esc)")
                .on_click(move |_, _, cx| back.update(cx, |state, cx| state.toggle_providers(cx)))))
        .child(v_flex().w_full().min_w_0().max_w(zpx(640.)).gap_3()
            .when(preferences, |content| content.child(appearance_controls()))
            .when(preferences, |content| {
                let state = state.clone();
                content.child(h_flex().w_full().p_4().gap_4().items_center()
                    .child(v_flex().flex_1().min_w_0().gap_1()
                        .child(div().child("Global session tabs"))
                        .child(div().text_size(zpx(TEXT_SM)).text_color(crate::appearance::secondary_text()).child("Keep the same top tabs when switching Spaces.")))
                    .child(gpui_kit::component::switch::Switch::new("global-space-tabs")
                        .checked(s.spaces.global_tabs).accessibility_label("Global session tabs")
                        .on_click(move |checked, _, cx| state.update(cx, |s, cx| s.set_global_space_tabs(*checked, cx)))))
            })
            .when(section == SettingsSection::Keybinds, |content| content.child(crate::keybinds::render()))
            .when(section == SettingsSection::Providers, |content| content
                .children(providers::VISIBLE.into_iter().map(|provider| {
                    v_flex().rounded(zpx(10.)).bg(crate::appearance::composer_surface()).pb_1()
                        .child(provider_row(provider, s.status_of(provider), state, cx))
                        .when(provider != Provider::Acp, |rows| rows.child(div().h(zpx(1.)).mx_3().mb_1().bg(crate::appearance::separator())).child(
                            h_flex().pl(zpx(30.)).pr_3().py_1p5().gap_2().items_center()
                                .child(v_flex().flex_1().min_w_0().gap_1()
                                    .child(h_flex().gap_2().items_center()
                                        .child(div().min_w_0().truncate().text_size(zpx(TEXT_SM))
                                            .when(s.default_account(provider).is_none(), |label| label.text_color(crate::appearance::ActionTone::Success.foreground()))
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
                            h_flex().pl(zpx(30.)).pr_3().py_1p5().gap_2().items_center()
                                .child(v_flex().flex_1().min_w_0().gap_1()
                                    .child(h_flex().gap_2().items_center()
                                        .child(div().min_w_0().truncate().text_size(zpx(TEXT_SM))
                                            .when(account.is_default, |label| label.text_color(crate::appearance::ActionTone::Success.foreground()))
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
                .child(div().text_size(zpx(TEXT_SM)).text_color(crate::appearance::secondary_text())
                    .child("Sign in opens your provider’s login in Terminal. Status updates automatically.")))))
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
