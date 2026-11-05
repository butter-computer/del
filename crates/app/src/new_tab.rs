//! Shared new-tab content for session tabs and the right sidebar.
use crate::animated_icons::Glyph;
use crate::appearance::rgba;
use crate::keyboard_control::KeyboardControl as _;
use crate::right_panel::Pane;
use crate::zoom::px as zpx;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub fn address_bar(address: &Entity<InputState>) -> AnyElement {
    h_flex()
        .h(zpx(44.))
        .flex_none()
        .px_3()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .text_color(rgba(0xffffff35))
                .child(
                    crate::icons::icon(IconName::ArrowLeft).with_size(zpx(crate::icons::MARK_SIZE)),
                )
                .child(
                    crate::icons::icon(IconName::ArrowRight)
                        .with_size(zpx(crate::icons::MARK_SIZE)),
                )
                .child(
                    crate::icons::path("icons/reload.svg").with_size(zpx(crate::icons::MARK_SIZE)),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .px_2()
                .h(zpx(28.))
                .rounded(zpx(8.))
                .bg(rgba(0xffffff14))
                .child(
                    Input::new(address)
                        .small()
                        .flex_1()
                        .min_w_0()
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false),
                ),
        )
        .into_any_element()
}

pub fn tools<T: 'static>(
    compact: bool,
    scroll: &ScrollHandle,
    choose: fn(&mut T, Pane, &mut Window, &mut Context<T>),
    cx: &mut Context<T>,
) -> AnyElement {
    let tool = |pane: Pane, label: &'static str, cx: &mut Context<T>| {
        let row = h_flex()
            .id(label)
            .keyboard_control()
            .debug_selector(move || label.into())
            .role(gpui::Role::Button)
            .aria_label(format!("Open {label}"))
            .flex_none()
            .w_full()
            .min_w_0()
            .h(zpx(44.))
            .px_3()
            .gap_2()
            .rounded_lg()
            .bg(rgba(0xffffff10))
            .hover(|b| b.bg(rgba(0xffffff18)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| choose(this, pane, window, cx)));
        crate::micro_motion::control((ElementId::from(label), "motion"), true, move |motion| {
            row.child(
                motion
                    .glyph(match pane {
                        Pane::Files => Glyph::Folder,
                        Pane::Terminal => Glyph::Terminal,
                        Pane::SideChat => Glyph::SideChat,
                        _ => Glyph::Changes,
                    })
                    .text_color(rgba(0xffffffb3)),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(
                div()
                    .flex_none()
                    .rounded(zpx(4.))
                    .px_1p5()
                    .text_size(zpx(12.))
                    .text_color(crate::appearance::secondary_text())
                    .bg(rgba(0xffffff08))
                    .child(match pane {
                        Pane::Changes => "⌃⇧G",
                        Pane::Terminal => "⌃`",
                        Pane::Files => "⌘P",
                        Pane::SideChat => "⌥⌘S",
                        _ => "",
                    }),
            )
        })
    };
    let changes = tool(Pane::Changes, "Changes", cx);
    let terminal = tool(Pane::Terminal, "Terminal", cx);
    let files = tool(Pane::Files, "Files", cx);
    let owner = cx.weak_entity();
    let side_chat = SideChatLauncher {
        create: std::rc::Rc::new(move |window, cx| {
            let _ = owner.update(cx, |this, cx| choose(this, Pane::SideChat, window, cx));
        }),
    };
    div()
        .id("new-tab-tools")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .child(
            v_flex()
                .w_full()
                .min_h_full()
                .p(if compact { zpx(16.) } else { zpx(24.) })
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(zpx(320.))
                        .flex_none()
                        .gap_2()
                        .text_size(zpx(del_core::typography::TEXT_MD))
                        .child(changes)
                        .child(terminal)
                        .child(files)
                        .child(side_chat),
                ),
        )
        .into_any_element()
}
type LaunchAction = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
struct SideChatLauncher {
    create: LaunchAction,
}

impl RenderOnce for SideChatLauncher {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hovered = window.use_keyed_state("side-launcher-hover", cx, |_, _| false);
        let focus = window.use_keyed_state("side-launcher-focus", cx, |_, cx| cx.focus_handle());
        let focused = focus.read(cx).contains_focused(window, cx);
        let shown = *hovered.read(cx) || focused;
        let target = if shown { 1. } else { 0. };
        let progress = if cx.reduce_motion() {
            target
        } else {
            gpui_kit::base::transition(
                "side-history-reveal",
                target,
                gpui_kit::base::Transition::new(std::time::Duration::from_millis(240))
                    .ease(ease_in_out),
                window,
                cx,
            )
        };
        let history_focus = focus.read(cx).clone();
        let button_hover = hovered.clone();
        let card_hover = hovered.clone();
        v_flex()
            .id("Side chat")
            .debug_selector(|| "Side chat".into())
            .track_focus(&history_focus)
            .w_full()
            .min_w_0()
            // Reserve the small card's space so revealing it never shifts the tools.
            // The shared hover area keeps the attached card reachable.
            .relative()
            .h(zpx(76.))
            .flex_none()
            .items_end()
            .on_hover(move |inside, _, cx| {
                if !*inside {
                    hovered.update(cx, |hovered, cx| {
                        *hovered = false;
                        cx.notify();
                    });
                }
            })
            .child(
                h_flex()
                    .id("side-chat-create")
                    .keyboard_control()
                    .role(gpui::Role::Button)
                    .aria_label("Open Side chat")
                    .w_full()
                    .h(zpx(44.))
                    .flex_none()
                    .min_w_0()
                    .px_3()
                    .gap_2()
                    .rounded_lg()
                    .bg(rgba(0xffffff10))
                    .hover(|s| s.bg(rgba(0xffffff18)))
                    .cursor_pointer()
                    .on_hover(move |inside, _, cx| {
                        if *inside {
                            button_hover.update(cx, |hovered, cx| {
                                *hovered = true;
                                cx.notify();
                            });
                        }
                    })
                    .on_click(move |_, window, cx| (self.create)(window, cx))
                    .child(
                        Pane::SideChat
                            .icon()
                            .with_size(zpx(crate::icons::MARK_SIZE))
                            .text_color(rgba(0xffffffb3)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child("Side chat"))
                    .child(
                        div()
                            .text_size(zpx(12.))
                            .text_color(crate::appearance::secondary_text())
                            .rounded(zpx(4.))
                            .px_1p5()
                            .bg(rgba(0xffffff08))
                            .child("⌥⌘S"),
                    ),
            )
            .when(progress > 0.001, |launcher| {
                launcher.child(
                    h_flex()
                        .id("side-history-open")
                        .keyboard_control()
                        .role(gpui::Role::Button)
                        .aria_label("Open Side history, Option Shift Command S")
                        .h(zpx(36.))
                        .pt_1()
                        .flex_none()
                        .max_w_full()
                        .min_w_0()
                        .gap_2()
                        .px_2()
                        .rounded(zpx(8.))
                        .bg(rgba(0xffffff08))
                        .relative()
                        .top(zpx(if cx.reduce_motion() {
                            -4.
                        } else {
                            -4. - 4. * (1. - progress)
                        }))
                        .opacity(progress)
                        .text_size(zpx(12.))
                        .cursor_pointer()
                        .text_color(crate::appearance::secondary_text())
                        .hover(|s| s.bg(rgba(0xffffff10)).text_color(rgba(0xffffffff)))
                        .on_hover(move |inside, _, cx| {
                            if *inside {
                                card_hover.update(cx, |hovered, cx| {
                                    *hovered = true;
                                    cx.notify();
                                });
                            }
                        })
                        .child(crate::icons::path("icons/history.svg").with_size(zpx(16.)))
                        .child(div().min_w_0().truncate().child("Side history"))
                        .child(
                            div()
                                .flex_none()
                                .rounded(zpx(4.))
                                .px_1p5()
                                .text_size(zpx(11.))
                                .bg(rgba(0xffffff08))
                                .child("⌥⇧⌘S"),
                        )
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::OpenSideHistory), cx);
                        }),
                )
            })
    }
}

pub fn destination(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if input == "about:blank" {
        return Some(input.into());
    }
    if input.contains("://") {
        let url = url::Url::parse(input).ok()?;
        return matches!(url.scheme(), "http" | "https").then(|| url.into());
    }
    if !input.contains(char::is_whitespace)
        && (input.contains('.') || input.starts_with("localhost"))
    {
        let scheme = if input.starts_with("localhost") || input.starts_with("127.") {
            "http"
        } else {
            "https"
        };
        return url::Url::parse(&format!("{scheme}://{input}"))
            .ok()
            .map(Into::into);
    }
    let mut url = url::Url::parse("https://www.google.com/search").unwrap();
    url.query_pairs_mut().append_pair("q", input);
    Some(url.into())
}
