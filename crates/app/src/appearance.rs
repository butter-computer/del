//! Persisted appearance shared by the custom chrome and component theme.
use crate::zoom::px as zpx;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Rgba};
use std::sync::atomic::{AtomicBool, Ordering};
static LIGHT: AtomicBool = AtomicBool::new(false);

// The calm palette (docs/neverfuckingchange.md, "Colors and surfaces"): graphite
// dark and warm off-white light, surfaces a few steps apart, quiet borders.
// Shared neutral dark tones used by both custom chrome and component defaults.
const DARK_PAGE: u32 = 0x212225;
const DARK_SURFACE: u32 = 0x27282c;
const DARK_HOVER: u32 = 0x2b2c30;
const DARK_ACTIVE: u32 = 0x34353a;
const DARK_FAINT_TEXT: u32 = 0x83837f;
const DARK_MUTED: u32 = DARK_FAINT_TEXT;
const DARK_SECONDARY_TEXT: u32 = 0xa3a39f;
const DARK_TEXT: u32 = 0xdcdcd8;
const DARK_PRIMARY_TEXT: u32 = 0xececea;
/// Terminals stay dark in both themes: graphite rather than pure black.
pub const TERMINAL_BACKGROUND: u32 = 0x141517;

/// Semantic action colors; neutrals stay neutral and color signals intent.
#[derive(Clone, Copy)]
pub enum ActionTone {
    Accent,
    Success,
    Warning,
}
impl ActionTone {
    pub fn foreground(self) -> Rgba {
        let value = match (self, light()) {
            (Self::Accent, false) => 0x8fb4e6,
            (Self::Accent, true) => 0x2f5f9e,
            (Self::Success, false) => 0x86b596,
            (Self::Success, true) => 0x3f7a54,
            (Self::Warning, false) => 0xd3b07a,
            (Self::Warning, true) => 0x87621f,
        };
        gpui_kit::rgb(value)
    }
    pub fn surface(self, hovered: bool) -> Rgba {
        let mut color = self.foreground();
        color.a = if hovered { 0.20 } else { 0.11 };
        color
    }
}

/// The sidebar shares the title bar's chrome surface, so there is no seam.
pub fn sidebar_surface() -> Rgba {
    shell_surface()
}

pub fn separator() -> Rgba {
    gpui_kit::rgba(if light() { 0x3c321e14 } else { 0xffffff0f })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[derive(Default)]
struct Preferences {
    light: bool,
}

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// Stable input surface, distinct from the black workspace.
/// Light keeps the composer darker than the page.
pub fn composer_surface() -> Rgba {
    gpui_kit::rgb(if light() { 0xf1efeb } else { 0x2a2b2f })
}

/// Shared raised surface for menus and dialogs in both themes.
pub fn popup_surface() -> Rgba {
    gpui_kit::rgb(if light() { 0xf6f5f2 } else { 0x28292d })
}

/// One chrome surface for the title bar, sidebar, gutters and status bar.
pub fn shell_surface() -> Rgba {
    gpui_kit::rgba(if light() {
        0xf7f6f3ff
    } else {
        del_core::SHELL_SURFACE_RGBA
    })
}

/// Opaque neutral backing keeps a dragged tab readable over either pane.
pub fn dragged_tab_surface() -> Rgba {
    gpui_kit::rgb(if light() { 0xe9e6e0 } else { 0x2b2c30 })
}

pub fn page_surface() -> Rgba {
    gpui_kit::rgba(if light() {
        0xfbfaf8ff
    } else {
        del_core::PAGE_SURFACE_RGBA
    })
}

fn path() -> std::path::PathBuf {
    del_agent::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("appearance.json")
}
/// Older preferences stored just the light/dark boolean; removed fields such
/// as an opacity value are ignored. Unreadable files fall back to dark.
fn parse(bytes: &[u8]) -> Preferences {
    if let Ok(light) = serde_json::from_slice::<bool>(bytes) {
        Preferences { light }
    } else {
        serde_json::from_slice::<Preferences>(bytes).unwrap_or_default()
    }
}
pub fn load(cx: &mut App) {
    let preferences = std::fs::read(path())
        .map(|bytes| parse(&bytes))
        .unwrap_or_default();
    apply(preferences.light, cx);
}
fn save() {
    if let Ok(bytes) = serde_json::to_vec(&Preferences { light: light() }) {
        let _ = crate::sessions::write_atomic(&path(), &bytes, 0o644);
    }
}
pub fn set(light: bool, cx: &mut App) {
    apply(light, cx);
    save();
    cx.refresh_windows();
}
fn apply(light: bool, cx: &mut App) {
    LIGHT.store(light, Ordering::Relaxed);
    Theme::change(
        if light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        None,
        cx,
    );
    Theme::update(cx, |theme| {
        theme.colors.background = gpui_kit::transparent_black();
        theme.font_size = zpx(del_core::typography::TEXT_MD);
        theme.colors.popover = popup_surface().into();
        theme.colors.popover_foreground = rgb(0xf0f0f0).into();
        theme.colors.border = rgba(0xffffff14).into();
        theme.colors.accent = rgba(0xffffff14).into();
        theme.colors.accent_foreground = rgb(0xf5f5f5).into();
        if !light {
            let c = &mut theme.colors;
            let surface = gpui_kit::rgb(DARK_SURFACE).into();
            let hover = gpui_kit::rgb(DARK_HOVER).into();
            let active = gpui_kit::rgb(DARK_ACTIVE).into();
            let text = gpui_kit::rgb(DARK_PRIMARY_TEXT).into();
            let secondary = gpui_kit::rgb(DARK_SECONDARY_TEXT).into();
            let white = gpui_kit::rgb(DARK_PRIMARY_TEXT).into();
            let black = shell_surface().into();
            let border = separator().into();
            c.foreground = text;
            c.popover = popup_surface().into();
            c.popover_foreground = text;
            c.border = border;
            c.input = border;
            c.accent = hover;
            c.accent_foreground = text;
            c.muted = hover;
            c.muted_foreground = secondary;
            c.button = surface;
            c.button_hover = hover;
            c.button_active = active;
            c.button_foreground = text;
            c.primary = gpui_kit::rgb(0x3b6db3).into();
            c.primary_hover = gpui_kit::rgb(0x4579bf).into();
            c.primary_active = gpui_kit::rgb(0x335f9c).into();
            c.primary_foreground = gpui_kit::rgb(0xffffff).into();
            c.button_primary = c.primary;
            c.button_primary_hover = c.primary_hover;
            c.button_primary_active = c.primary_active;
            c.button_primary_foreground = c.primary_foreground;
            c.secondary = surface;
            c.secondary_hover = hover;
            c.secondary_active = active;
            c.secondary_foreground = text;
            c.button_secondary = surface;
            c.button_secondary_hover = hover;
            c.button_secondary_active = active;
            c.button_secondary_foreground = text;
            c.ring = ActionTone::Accent.foreground().into();
            c.selection = gpui_kit::rgba(0x8fb4e640).into();
            c.link = ActionTone::Accent.foreground().into();
            c.link_hover = gpui_kit::rgb(0xb3cdef).into();
            c.link_active = c.link;
            c.list = surface;
            c.list_head = surface;
            c.list_even = surface;
            c.list_hover = hover;
            c.list_active = active;
            c.list_active_border = border;
            c.sidebar = black;
            c.sidebar_foreground = text;
            c.sidebar_accent = hover;
            c.sidebar_accent_foreground = text;
            c.sidebar_border = border;
            c.sidebar_primary = white;
            c.sidebar_primary_foreground = black;
            c.skeleton = hover;
            c.slider_bar = secondary;
            c.slider_thumb = white;
            c.switch = active;
            c.switch_thumb = white;
            c.scrollbar = gpui_kit::transparent_black();
            c.scrollbar_thumb = active;
            c.scrollbar_thumb_hover = gpui_kit::rgb(DARK_MUTED).into();
            c.tab = surface;
            c.tab_bar = black;
            c.tab_bar_segmented = surface;
            c.tab_foreground = secondary;
            c.tab_active = hover;
            c.tab_active_foreground = text;
            c.title_bar = black;
            c.title_bar_border = border;
            c.status_bar = black;
            c.status_bar_border = border;
            c.drag_border = secondary;
            c.drop_target = gpui_kit::rgba(0xffffff14).into();
        } else {
            let c = &mut theme.colors;
            c.popover = popup_surface().into();
            c.border = separator().into();
            c.input = separator().into();
            c.sidebar = shell_surface().into();
            c.title_bar = shell_surface().into();
            c.status_bar = shell_surface().into();
            c.tab_bar = shell_surface().into();
            c.foreground = gpui_kit::rgb(0x2a2926).into();
            c.muted_foreground = gpui_kit::rgb(0x6e6c66).into();
            c.ring = ActionTone::Accent.foreground().into();
            c.link = ActionTone::Accent.foreground().into();
            c.primary = ActionTone::Accent.foreground().into();
            c.button_primary = c.primary;
        }
    });
}
// Normalize neutral literals onto the calm palette, preserving alpha. Dark
// snaps them to the graphite scale; light inverts them onto warm ink and
// off-whites. Provider marks and status colours keep their hue; the few
// saturated status literals are softened by `status`.
fn palette(mut color: Rgba) -> Rgba {
    let neutral = color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b) < 0.06;
    if neutral {
        let value = (color.r + color.g + color.b) / 3.;
        let (r, g, b) = if light() {
            // White text becomes #2a2926 ink, black becomes a warm off-white.
            let t = 0.975 - value * 0.81;
            (t, (t - 0.005).max(0.), (t - 0.016).max(0.))
        } else {
            let tone = if value < 0.08 {
                DARK_PAGE
            } else if value < 0.18 {
                DARK_SURFACE
            } else if value < 0.26 {
                DARK_HOVER
            } else if value < 0.36 {
                DARK_ACTIVE
            } else if value < 0.52 {
                DARK_FAINT_TEXT
            } else if value < 0.75 {
                DARK_SECONDARY_TEXT
            } else if value < 0.94 {
                DARK_TEXT
            } else {
                DARK_PRIMARY_TEXT
            };
            let channel = |shift: u32| ((tone >> shift) & 0xff) as f32 / 255.;
            (channel(16), channel(8), channel(0))
        };
        color.r = r;
        color.g = g;
        color.b = b;
    }
    color
}

/// Desaturated replacements for the saturated status literals.
fn status(rgb: u32) -> u32 {
    match (rgb, light()) {
        // Danger red.
        (0xff646b | 0xff4545, false) => 0xe08e8c,
        (0xff646b | 0xff4545, true) => 0xa94848,
        // Unread dot: Del's softened blue.
        (0x4a9eff, false) => 0x8fb4e6,
        (0x4a9eff, true) => 0x2f5f9e,
        // Claude's mark.
        (0xd97757, false) => 0xcf8b70,
        (0xd97757, true) => 0xb8714f,
        // Keep-awake gold and the approval amber.
        (0xf5a524 | 0xe8b04a, false) => 0xd3b07a,
        (0xf5a524 | 0xe8b04a, true) => 0x87621f,
        (other, _) => other,
    }
}

pub fn rgb(value: u32) -> Rgba {
    palette(gpui_kit::rgb(status(value)))
}
pub fn rgba(value: u32) -> Rgba {
    palette(gpui_kit::rgba(status(value >> 8) << 8 | (value & 0xff)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_theme_tolerates_legacy_unknown_and_corrupt_files() {
        assert!(parse(b"true").light);
        assert!(!parse(b"false").light);
        assert!(parse(br#"{"light":true}"#).light);
        // Removed opacity settings and future fields never break loading.
        assert!(parse(br#"{"light":true,"opacity":0.4,"future":[1]}"#).light);
        assert!(!parse(br#"{}"#).light);
        assert!(!parse(b"{not json").light);
        let bytes = serde_json::to_vec(&Preferences { light: true }).unwrap();
        assert!(parse(&bytes).light);
    }
}
