//! Read-only reference for app shortcuts and context-specific keyboard controls.
use crate::appearance::rgba;
use crate::zoom::px as zpx;
use gpui_kit::component::*;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Picker {
    Provider,
    Model,
    Effort,
    Branch,
}

const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Sessions and tabs",
        &[
            ("New session", "⌘N"),
            ("New inner tab in the last-used zone", "⌘T / ⌃T"),
            ("Close top-level session", "⌘⇧W"),
            ("Close active inner tab in the last-used zone", "⌘W"),
            ("Select top-level session by position", "⌘1–9"),
            ("Next / previous top-level session", "⌘⇧] / ⌘⇧["),
            ("Next / previous main-area inner tab", "⌃Tab / ⌃⇧Tab"),
        ],
    ),
    (
        "Workspace tools",
        &[
            ("Open Changes in the last-used zone", "⌃⇧G"),
            ("Open Files in the last-used zone", "⌘P"),
            ("Open a new Terminal in the last-used zone", "⌃` / ⌘J"),
            ("Open Side chat in the last-used zone", "⌥⌘S"),
            ("Toggle left sidebar", "⌘B"),
            ("Toggle right sidebar", "⌘\\ / ⌘⇧B"),
            ("Save the focused file editor", "⌘S"),
        ],
    ),
    (
        "Conversation",
        &[
            ("Focus main composer", "⌘L"),
            ("Select provider/account in the active chat", "⌃⇧P"),
            ("Select model in the active chat", "⌃M"),
            ("Select effort in the active chat", "⌃⇧M"),
            ("Select starting branch in a new-session draft", "⌃⇧B"),
            ("Send or queue a message", "Return"),
            ("Insert a newline", "⇧Return"),
            (
                "Steer draft or send first queued item in main chat",
                "⌘Return",
            ),
            ("Stop the main chat turn", "⌘."),
            ("Open / close conversation search", "⌘K"),
        ],
    ),
    (
        "App",
        &[
            ("Open / close Settings", "⌘,"),
            ("Close Settings or an open picker", "Esc"),
            ("Zoom in", "⌘+ / ⌘⇧="),
            ("Zoom out", "⌘−"),
            ("Reset zoom", "⌘0"),
            ("Quit Del", "⌘Q"),
        ],
    ),
    (
        "Search and model picker",
        &[
            ("Move through results", "↑ / ↓"),
            ("Choose highlighted result", "Return"),
            ("Change effort when effort controls are focused", "← / →"),
            ("Confirm effort and return to the composer", "Return"),
            ("Dismiss search or picker", "Esc"),
        ],
    ),
    (
        "Branch picker",
        &[
            ("Move through branches", "↑ / ↓"),
            ("Move by a visible page", "Page Up / Page Down"),
            ("Choose highlighted branch", "Return"),
            ("Dismiss branch picker", "Esc"),
        ],
    ),
    (
        "Text editing and terminal",
        &[
            ("Copy / paste", "⌘C / ⌘V"),
            ("Cut selected text in an editor", "⌘X"),
            ("Select all text in an editor", "⌘A"),
            ("Undo / redo in an editor", "⌘Z / ⌘⇧Z"),
            (
                "Terminal scrollback: page up / down",
                "⇧Page Up / ⇧Page Down",
            ),
            ("Terminal scrollback: start / end", "⇧Home / ⇧End"),
        ],
    ),
];

pub fn render() -> impl IntoElement {
    v_flex()
        .w_full()
        .min_w_0()
        .gap_5()
        .children(GROUPS.iter().map(|(title, shortcuts)| {
            v_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(
                    div()
                        .text_size(zpx(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(*title),
                )
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .rounded_lg()
                        .bg(rgba(0xffffff08))
                        .children(shortcuts.iter().map(|(label, shortcut)| {
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .px_3()
                                .py_2()
                                .gap_3()
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .whitespace_normal()
                                        .text_size(zpx(13.))
                                        .child(*label),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .rounded_md()
                                        .px_2()
                                        .py_1()
                                        .bg(rgba(0xffffff08))
                                        .text_size(zpx(12.))
                                        .text_color(rgba(0xffffffb3))
                                        .child(*shortcut),
                                )
                        })),
                )
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use gpui_kit::component::{button::Button, menu::DropdownMenu};
    #[::gpui::test]
    fn controlled_menu_can_open_dismiss_and_reopen(cx: &mut gpui::TestAppContext) {
        struct Controlled {
            open: bool,
        }
        impl Render for Controlled {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(
                    Button::new("controlled-trigger")
                        .label("Provider")
                        .dropdown_menu(|menu, _, _| {
                            menu.menu("Copy", Box::new(crate::SelectProvider))
                        })
                        .open(self.open)
                        .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                            this.open = *open;
                            cx.notify();
                        })),
                )
            }
        }
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (root, cx) = cx.add_window_view(|_, _| Controlled { open: true });
        cx.run_until_parked();
        assert!(root.read_with(cx, |root, _| root.open));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!root.read_with(cx, |root, _| root.open));
        root.update(cx, |root, cx| {
            root.open = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!root.read_with(cx, |root, _| root.open));
        for open in [true, false, true] {
            root.update(cx, |root, cx| {
                root.open = open;
                cx.notify();
            });
            cx.run_until_parked();
        }
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!root.read_with(cx, |root, _| root.open));
    }
}
