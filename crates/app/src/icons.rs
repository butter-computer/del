//! Shared application icon metrics and semantic Apple SF Symbols assets.
//! macOS asks AppKit for symbols at runtime; other platforms and unavailable
//! symbols retain the existing SVG fallback. Provider marks are identities.
use crate::zoom::px as zpx;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::{Pixels, SharedString};

pub const GLYPH_SIZE: f32 = 16.0;
pub const BUTTON_SIZE: f32 = 28.0;

pub fn size() -> Pixels {
    zpx(GLYPH_SIZE)
}
pub fn button_size() -> Pixels {
    zpx(BUTTON_SIZE)
}
pub fn icon(value: impl Into<Icon>) -> Icon {
    Icon::new(value).with_size(size())
}
pub fn path(value: impl Into<SharedString>) -> Icon {
    icon(Icon::default().path(value))
}
pub fn spinner() -> gpui_kit::component::spinner::Spinner {
    gpui_kit::component::spinner::Spinner::new().with_size(size())
}

// Source asset name -> public SF Symbols name. This also covers component
// controls (menus, carets, inputs, loading states), not just the toolbar.
pub const SYMBOLS: &[(&str, &str)] = &[
    ("activity", "waveform.path.ecg"),
    ("file-pdf", "doc.richtext"),
    ("file-doc", "doc.text"),
    ("file-xls", "tablecells"),
    ("file-ppt", "doc.richtext"),
    ("a-large-small", "textformat.size"),
    ("arrow-down", "arrow.down"),
    ("arrow-left", "arrow.left"),
    ("arrow-right", "arrow.right"),
    ("arrow-up", "arrow.up"),
    ("asterisk", "asterisk"),
    ("ban", "nosign"),
    ("battery-charging", "battery.100.bolt"),
    ("battery-full", "battery.100"),
    ("battery-low", "battery.25"),
    ("battery-medium", "battery.50"),
    ("battery-warning", "battery.0"),
    ("battery", "battery.100"),
    ("bell", "bell"),
    ("book-open", "book"),
    ("bot", "brain"),
    ("building-2", "building.2"),
    ("calendar", "calendar"),
    ("case-sensitive", "textformat"),
    ("chart-pie", "chart.pie"),
    ("check", "checkmark"),
    ("chevron-down", "chevron.down"),
    ("chevron-left", "chevron.left"),
    ("chevron-right", "chevron.right"),
    ("chevron-up", "chevron.up"),
    ("chevrons-up-down", "chevron.up.chevron.down"),
    ("circle-alert", "exclamationmark.circle"),
    ("circle-check", "checkmark.circle"),
    ("circle-user", "person.circle"),
    ("circle-x", "xmark.circle"),
    ("close", "xmark"),
    ("copy", "doc.on.doc"),
    ("cpu", "cpu"),
    ("dash", "minus"),
    ("delete", "delete.left"),
    ("ellipsis-vertical", "ellipsis"),
    ("ellipsis", "ellipsis"),
    ("external-link", "arrow.up.right.square"),
    ("eye-off", "eye.slash"),
    ("eye", "eye"),
    ("file-text", "doc.text"),
    ("file", "doc"),
    ("folder-closed", "folder"),
    ("folder-open", "folder"),
    ("folder", "folder"),
    ("frame", "square.dashed"),
    ("gallery-vertical-end", "rectangle.stack"),
    ("globe", "globe"),
    ("hard-drive", "externaldrive"),
    ("heart-off", "heart.slash"),
    ("heart", "heart"),
    ("inbox", "tray"),
    ("info", "info.circle"),
    ("inspector", "sidebar.right"),
    ("layout-dashboard", "square.grid.2x2"),
    ("loader-circle", "arrow.triangle.2.circlepath"),
    ("loader", "arrow.triangle.2.circlepath"),
    ("map", "map"),
    ("maximize", "arrow.up.left.and.arrow.down.right"),
    ("memory-stick", "memorychip"),
    ("menu", "line.3.horizontal"),
    ("mic", "mic"),
    ("minimize", "arrow.down.right.and.arrow.up.left"),
    ("minus", "minus"),
    ("moon", "moon"),
    ("network", "network"),
    ("palette", "paintpalette"),
    ("panel-bottom-open", "rectangle.bottomthird.inset.filled"),
    ("panel-bottom", "rectangle.bottomthird.inset.filled"),
    ("panel-left-close", "sidebar.left"),
    ("panel-left-open", "sidebar.left"),
    ("panel-left", "sidebar.left"),
    ("panel-right-close", "sidebar.right"),
    ("panel-right-open", "sidebar.right"),
    ("panel-right", "sidebar.right"),
    ("pause", "pause"),
    ("play", "play"),
    ("plus", "plus"),
    ("redo-2", "arrow.uturn.forward"),
    ("redo", "arrow.uturn.forward"),
    ("refresh-cw", "arrow.clockwise"),
    ("replace", "arrow.left.arrow.right"),
    ("resize-corner", "arrow.up.left.and.arrow.down.right"),
    ("rotate-cw", "arrow.clockwise"),
    ("search", "magnifyingglass"),
    ("settings-2", "gearshape"),
    ("settings", "gearshape"),
    ("sort-ascending", "arrow.up"),
    ("sort-descending", "arrow.down"),
    ("square-terminal", "terminal"),
    ("square", "stop.fill"),
    ("star-fill", "star.fill"),
    ("star-off", "star.slash"),
    ("star", "star"),
    ("sun", "sun.max"),
    ("thumbs-down", "hand.thumbsdown"),
    ("thumbs-up", "hand.thumbsup"),
    ("triangle-alert", "exclamationmark.triangle"),
    ("undo-2", "arrow.uturn.backward"),
    ("undo", "arrow.uturn.backward"),
    ("user", "person"),
    ("window-close", "xmark"),
    ("window-maximize", "arrow.up.left.and.arrow.down.right"),
    ("window-minimize", "minus"),
    ("window-restore", "macwindow"),
    ("pencil", "square.and.pencil"),
    ("archive", "archivebox"),
    ("pin", "pin"),
    ("pin-off", "pin.slash"),
    ("coffee", "cup.and.saucer"),
    ("trash", "trash"),
    ("target", "scope"),
    ("messages-square", "bubble.left.and.bubble.right"),
    ("terminal", "terminal"),
    ("microphone", "mic"),
    ("zap", "bolt"),
    ("pin-outline", "pin"),
    ("pin-filled", "pin.fill"),
    ("coffee-cup", "cup.and.saucer"),
    ("steer", "arrow.turn.up.right"),
    ("grip", "line.3.horizontal"),
    ("queue", "list.bullet"),
    ("restore", "arrow.uturn.backward"),
    ("skill", "sparkles"),
    ("stop", "stop.fill"),
    ("return", "return"),
    ("success", "checkmark.circle"),
    ("error", "exclamationmark.circle"),
    ("declined", "minus.circle"),
];

/// SVG fallback aliases for symbols without a standalone local source asset.
pub fn fallback_path(path: &str) -> &str {
    match path {
        "icons/return.svg" => "icons/redo-2.svg",
        "icons/file-pdf.svg" | "icons/file-doc.svg" | "icons/file-xls.svg"
        | "icons/file-ppt.svg" => "icons/file-text.svg",
        _ => path,
    }
}

pub fn symbol_name(path: &str) -> Option<&'static str> {
    let stem = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    SYMBOLS
        .iter()
        .find_map(|(source, symbol)| (*source == stem).then_some(*symbol))
}

#[cfg(target_os = "macos")]
fn render_symbol(name: &str) -> Option<Vec<u8>> {
    use std::ffi::{CStr, CString, c_char};
    unsafe extern "C" {
        fn del_system_symbol_svg(name: *const c_char) -> *mut c_char;
        fn del_free_system_symbol_svg(bytes: *mut c_char);
    }
    let name = CString::new(name).ok()?;
    let pointer = unsafe { del_system_symbol_svg(name.as_ptr()) };
    if pointer.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(pointer).to_bytes().to_vec() };
    unsafe { del_free_system_symbol_svg(pointer) };
    Some(bytes)
}

pub fn system_asset(path: &str) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            collections::HashMap,
            sync::{Mutex, OnceLock},
        };
        static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Vec<u8>>>>> = OnceLock::new();
        let name = symbol_name(path)?;
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache
            .entry(name)
            .or_insert_with(|| {
                let symbol = render_symbol(name);
                if symbol.is_none() {
                    eprintln!(
                        "[del icons] SF Symbol {name} unavailable; using SVG fallback for {path}"
                    );
                }
                symbol
            })
            .clone()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_actions_use_sf_symbols() {
        assert_eq!(symbol_name("icons/archive.svg"), Some("archivebox"));
        assert_eq!(symbol_name("icons/pin.svg"), Some("pin"));
        assert_eq!(symbol_name("icons/pin-filled.svg"), Some("pin.fill"));
    }

    #[test]
    fn provider_identity_assets_are_not_replaced() {
        for path in [
            "icons/claude.svg",
            "icons/codex.svg",
            "icons/acp.svg",
            "icons/opencode.svg",
            "icons/github.svg",
        ] {
            assert_eq!(symbol_name(path), None);
        }
    }

    #[gpui::test]
    fn glyphs_and_icon_buttons_keep_shared_rendered_bounds(cx: &mut gpui::TestAppContext) {
        use gpui_kit::component::{
            IconName,
            button::{Button, ButtonVariants},
            h_flex,
        };
        use gpui_kit::prelude::*;
        struct MetricsFixture;
        impl gpui::Render for MetricsFixture {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex()
                            .debug_selector(|| "standalone-glyph".into())
                            .flex_none()
                            .child(crate::icons::icon(IconName::Plus)),
                    )
                    .child(
                        h_flex()
                            .debug_selector(|| "icon-button-target".into())
                            .flex_none()
                            .child(
                                Button::new("metrics-button")
                                    .ghost()
                                    .size(crate::icons::button_size())
                                    .p_0()
                                    .child(
                                        h_flex()
                                            .debug_selector(|| "button-glyph".into())
                                            .flex_none()
                                            .child(crate::icons::icon(IconName::Plus)),
                                    ),
                            ),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|window, _| {
            window.set_rem_size(gpui::px(14.));
            MetricsFixture
        });
        for (zoom, glyph, button) in [(100, 16., 28.), (150, 24., 42.)] {
            cx.update(|window, cx| {
                del_core::zoom::set(zoom);
                window.set_rem_size(gpui::px(14. * zoom as f32 / 100.));
                window.refresh();
                window.draw(cx).clear(cx);
            });
            for selector in ["standalone-glyph", "button-glyph"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert_eq!(bounds.size.width, gpui::px(glyph));
                assert_eq!(bounds.size.height, gpui::px(glyph));
            }
            let target = cx.debug_bounds("icon-button-target").unwrap();
            assert_eq!(target.size.width, gpui::px(button));
            assert_eq!(target.size.height, gpui::px(button));
        }
        cx.update(|_, _| del_core::zoom::set(100));
    }

    #[gpui::test]
    fn tab_hint_yields_to_hover_close_without_growing_or_overlapping(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui_kit::prelude::*;
        use std::{cell::Cell, rc::Rc};
        struct TabFixture(Rc<Cell<usize>>);
        impl gpui::Render for TabFixture {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let closed = self.0.clone();
                gpui::div()
                    .id("tab-fixture")
                    .group("tab-fixture")
                    .flex()
                    .items_center()
                    .w(gpui::px(160.))
                    .h(gpui::px(24.))
                    .child(
                        gpui::div()
                            .flex_1()
                            .child(crate::tab_title("A long tab title".into())),
                    )
                    .child(
                        gpui::div()
                            .id("close-fixture")
                            .debug_selector(|| "close-fixture".into())
                            .size(button_size())
                            .flex_none()
                            .on_click(move |_, _, _| closed.set(closed.get() + 1))
                            .child(crate::tab_action_content(
                                "tab-fixture".into(),
                                Some(1),
                                gpui::white(),
                            )),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let closed = Rc::new(Cell::new(0));
        let fixture_closed = closed.clone();
        let (_, cx) = cx.add_window_view(|window, _| {
            window.set_rem_size(gpui::px(14.));
            TabFixture(fixture_closed)
        });
        cx.simulate_mouse_move(
            gpui::point(gpui::px(500.), gpui::px(500.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("close-fixture").unwrap();
        assert_eq!(target.size, gpui::size(button_size(), button_size()));
        let visible_hint = |window: &gpui::Window| {
            let hint: gpui::Hsla =
                crate::appearance::rgb(0x303033).into();
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background.as_solid() == Some(hint))
        };
        assert!(
            cx.update(|window, _| visible_hint(window)),
            "shortcut badge missing"
        );
        cx.simulate_mouse_move(target.center(), None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !cx.update(|window, _| visible_hint(window)),
            "shortcut badge still covers hover close"
        );
        assert_eq!(target, cx.debug_bounds("close-fixture").unwrap());
        cx.simulate_mouse_down(
            target.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            target.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        assert_eq!(closed.get(), 1);
        cx.simulate_mouse_move(
            gpui::point(gpui::px(500.), gpui::px(500.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.update(|window, _| visible_hint(window)),
            "hint did not return after hover"
        );
    }

}
