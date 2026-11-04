//! Shared new-tab content for session tabs and the right sidebar.
use crate::animated_icons::Glyph;
use crate::appearance::rgba;
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
    let side_chat = tool(Pane::SideChat, "Side chat", cx);
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
