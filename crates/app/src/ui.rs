//! Shared UI tokens and small building blocks.
//!
//! Every hand-built surface takes its spacing, radii, row heights, list fills,
//! text roles and motion timings from here, so panels stay on one grid. All
//! lengths are logical pixels for `zoom::px`; never use gpui's rem utilities
//! (`px_2`, `gap_3`, `rounded_md`, ...) in this crate: at Del's 13 px base they
//! land on fractional sizes such as 6.5 and 9.75.
#![allow(dead_code)] // Removed once every helper has callers.
use crate::appearance::{self, ActionTone};
use crate::zoom::px as zpx;
use del_core::typography::{TEXT_SM, TEXT_XS};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

// Spacing scale.
pub const SPACE_2XS: f32 = 2.;
pub const SPACE_XS: f32 = 4.;
pub const SPACE_SM: f32 = 6.;
pub const SPACE_MD: f32 = 8.;
pub const SPACE_LG: f32 = 12.;
pub const SPACE_XL: f32 = 16.;
pub const SPACE_2XL: f32 = 24.;

// Corner radii by role.
/// Kbd badges and other tiny chips.
pub const RADIUS_XS: f32 = 4.;
/// List rows, small buttons, tool tabs.
pub const RADIUS_ROW: f32 = 6.;
/// Cards inside a page: transcript cards, code, trays, settings groups.
pub const RADIUS_CARD: f32 = 8.;
/// Floating menus, pickers, popovers and hover cards.
pub const RADIUS_POPOVER: f32 = 10.;
/// Modal dialogs, Settings and Cmd-K.
pub const RADIUS_MODAL: f32 = 12.;

// Row heights by role.
/// Tree rows.
pub const ROW_COMPACT: f32 = 26.;
/// Menus, pickers.
pub const ROW_MENU: f32 = 28.;
/// Sidebar rows and settings navigation.
pub const ROW_SIDEBAR: f32 = 30.;
/// Two-line rows: commands, search results, models.
pub const ROW_TWO_LINE: f32 = 44.;

// Motion.
/// Background fill change on hover and press.
pub const FILL_MS: u64 = 120;
/// Popovers and floating panels: the same timing as dropdown menus.
pub const POPOVER_ENTER: Duration = Duration::from_millis(180);
pub const POPOVER_EXIT: Duration = Duration::from_millis(120);
/// Vertical lift for a floating surface's entrance.
pub const POPOVER_LIFT: f32 = 4.;
/// Height and opacity reveal for rows that join a list or tray.
pub const REVEAL: Duration = Duration::from_millis(180);
/// Show nothing for loads that finish faster than this, so they never flash.
pub const LOADING_DELAY: Duration = Duration::from_millis(150);
/// One period for every text shimmer, so bands on screen together stay in step.
pub const SHIMMER_PERIOD: Duration = Duration::from_millis(1600);
/// One full spinner turn, linear.
pub const SPINNER_PERIOD: Duration = Duration::from_millis(800);
/// Hold Command this long before tab-number hints appear.
pub const SHORTCUT_HINT_DELAY: Duration = Duration::from_millis(600);

/// Ease-out cubic, the default curve for UI chrome.
pub fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

// List fills: the sidebar and tab values, used by every list.
pub fn row_hover() -> Rgba {
    appearance::rgba(0xffffff0a)
}
pub fn row_selected() -> Rgba {
    appearance::rgba(0xffffff14)
}
pub fn row_pressed() -> Rgba {
    appearance::rgba(0xffffff1a)
}

// Text roles: primary, secondary, faint. Nothing in between.
pub fn primary_text() -> Rgba {
    appearance::rgb(0xf5f5f5)
}
pub use appearance::{faint_text, secondary_text};

/// One keyboard badge everywhere: menus, Cmd-K, launchers, Keybinds.
pub fn kbd(label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(zpx(5.))
        .min_w(zpx(18.))
        .h(zpx(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(zpx(RADIUS_XS))
        .bg(appearance::rgba(0xffffff0d))
        .text_size(zpx(TEXT_XS))
        .text_color(secondary_text())
        .child(label.into())
}

/// Section label inside a list or popover ("Actions", "Recent commits").
pub fn section_label(label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .text_size(zpx(TEXT_SM))
        .text_color(secondary_text())
        .child(label.into())
}

/// A quiet one-line note for empty or loading lists.
pub fn empty_note(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(zpx(ROW_MENU))
        .px(zpx(SPACE_MD))
        .text_size(zpx(TEXT_SM))
        .text_color(secondary_text())
        .child(text.into())
}

/// An inline error: danger ink, small, wraps, never a raw technical blob.
pub fn inline_error(message: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap(zpx(SPACE_SM))
        .text_size(zpx(TEXT_SM))
        .text_color(ActionTone::Danger.foreground())
        .child(message.into())
}

/// Abbreviates the home directory to `~` for display.
pub fn display_path(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_owned();
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_owned();
    }
    if path == home {
        "~".to_owned()
    } else if let Some(rest) = path.strip_prefix(home).filter(|rest| rest.starts_with('/')) {
        format!("~{rest}")
    } else {
        path.to_owned()
    }
}

/// Thousands separators: 184523 → "184,523".
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Compact counts: 950, 1.2K, 184K, 1.5M.
pub fn compact_count(value: u64) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..10_000 => trim_decimal(value as f64 / 1_000., "K"),
        10_000..1_000_000 => format!("{}K", value / 1_000),
        1_000_000..10_000_000 => trim_decimal(value as f64 / 1_000_000., "M"),
        _ => format!("{}M", value / 1_000_000),
    }
}
fn trim_decimal(value: f64, suffix: &str) -> String {
    let text = format!("{value:.1}");
    format!("{}{suffix}", text.trim_end_matches(".0"))
}

/// Memory in MB or GB: 512 MB, 2.0 GB.
pub fn megabytes(mb: f64) -> String {
    if mb >= 1024. {
        format!("{:.1} GB", mb / 1024.)
    } else {
        format!("{} MB", mb.round() as u64)
    }
}

/// "1 file", "2 files".
pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::{compact_count, display_path, ease_out, group_digits, megabytes, plural};

    #[test]
    fn formats_numbers_for_people() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(184523), "184,523");
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(compact_count(950), "950");
        assert_eq!(compact_count(1000), "1K");
        assert_eq!(compact_count(1250), "1.2K");
        assert_eq!(compact_count(184523), "184K");
        assert_eq!(compact_count(1_500_000), "1.5M");
        assert_eq!(megabytes(512.4), "512 MB");
        assert_eq!(megabytes(2048.), "2.0 GB");
        assert_eq!(plural(1, "file", "files"), "1 file");
        assert_eq!(plural(3, "file", "files"), "3 files");
    }

    #[test]
    fn abbreviates_home() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(&format!("{home}/code/del")), "~/code/del");
        assert_eq!(display_path("/tmp/x"), "/tmp/x");
        assert_eq!(display_path(&format!("{home}other")), format!("{home}other"));
    }

    #[test]
    fn ease_out_is_bounded() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert!(ease_out(0.5) > 0.5);
    }
}
