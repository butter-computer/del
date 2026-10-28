//! Persisted appearance shared by the custom chrome and component theme.
use crate::zoom::px as zpx;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Rgba};
use std::sync::atomic::{AtomicBool, Ordering};
static LIGHT: AtomicBool = AtomicBool::new(false);

// Shared neutral dark tones used by both custom chrome and component defaults.
const DARK_SURFACE: u32 = 0x101010;
const DARK_HOVER: u32 = 0x191919;
const DARK_ACTIVE: u32 = 0x222222;
const DARK_MUTED: u32 = 0x737373;
const DARK_SECONDARY_TEXT: u32 = 0xa1a1a1;
const DARK_PRIMARY_TEXT: u32 = 0xfafafa;

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
            (Self::Accent, false) => 0x79b4ff,
            (Self::Accent, true) => 0x175db5,
            (Self::Success, false) => 0x72c997,
            (Self::Success, true) => 0x236940,
            (Self::Warning, false) => 0xe6b56c,
            (Self::Warning, true) => 0x885210,
        };
        gpui_kit::rgb(value)
    }
    pub fn surface(self, hovered: bool) -> Rgba {
        let mut color = self.foreground();
        color.a = if hovered { 0.20 } else { 0.11 };
        color
    }
}

pub fn sidebar_surface() -> Rgba {
    gpui_kit::rgb(if light() { 0xfcfcfc } else { 0x060606 })
}

pub fn separator() -> Rgba {
    gpui_kit::rgba(if light() { 0x00000014 } else { 0xffffff14 })
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
pub fn composer_surface() -> Rgba {
    gpui_kit::rgba(if light() { 0xf9f9f9ff } else { 0x191919ff })
}

/// Shared raised surface for menus and dialogs in both themes.
pub fn popup_surface() -> Rgba {
    if light() {
        composer_surface()
    } else {
        gpui_kit::rgb(0x161616)
    }
}

pub fn shell_surface() -> Rgba {
    gpui_kit::rgba(if light() {
        0xfcfcfcd9
    } else {
        del_core::SHELL_SURFACE_RGBA
    })
}

pub fn page_surface() -> Rgba {
    gpui_kit::rgba(if light() {
        0xffffffff
    } else {
        del_core::PAGE_SURFACE_RGBA
    })
}

fn path() -> std::path::PathBuf {
    del_agent::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("appearance.json")
}
pub fn load(cx: &mut App) {
    let preferences = std::fs::read(path())
        .ok()
        .and_then(|bytes| {
            // Older preferences stored just the light/dark boolean.
            if let Ok(light) = serde_json::from_slice::<bool>(&bytes) {
                Some(Preferences { light })
            } else {
                serde_json::from_slice::<Preferences>(&bytes).ok()
            }
        })
        .unwrap_or_default();
    apply(preferences.light, cx);
}
fn save() {
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&Preferences { light: light() }) {
        let temporary = path.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
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
            let white = gpui_kit::rgb(0xffffff).into();
            let black = gpui_kit::rgb(0x000000).into();
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
            c.primary = gpui_kit::rgb(0x286bd1).into();
            c.primary_hover = gpui_kit::rgb(0x347be5).into();
            c.primary_active = gpui_kit::rgb(0x205bb8).into();
            c.primary_foreground = white;
            c.button_primary = c.primary;
            c.button_primary_hover = c.primary_hover;
            c.button_primary_active = c.primary_active;
            c.button_primary_foreground = white;
            c.secondary = surface;
            c.secondary_hover = hover;
            c.secondary_active = active;
            c.secondary_foreground = text;
            c.button_secondary = surface;
            c.button_secondary_hover = hover;
            c.button_secondary_active = active;
            c.button_secondary_foreground = text;
            c.ring = ActionTone::Accent.foreground().into();
            c.selection = gpui_kit::rgba(0x579fff40).into();
            c.link = ActionTone::Accent.foreground().into();
            c.link_hover = gpui_kit::rgb(0xa8cfff).into();
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
        }
    });
}
// Normalize legacy charcoal/purple-tinted neutral literals into the shared
// dark scale. Preserve alpha and meaningful provider/status colors. Light mode
// keeps its existing neutral inversion rather than inheriting dark overrides.
fn palette(mut color: Rgba) -> Rgba {
    let neutral = color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b) < 0.06;
    if neutral {
        if light() {
            color.r = 0.98 - color.r * 0.88;
            color.g = 0.98 - color.g * 0.88;
            color.b = 0.98 - color.b * 0.88;
        } else {
            let value = (color.r + color.g + color.b) / 3.;
            let tone = if value < 0.08 {
                0x000000
            } else if value < 0.18 {
                DARK_SURFACE
            } else if value < 0.26 {
                DARK_HOVER
            } else if value < 0.36 {
                DARK_ACTIVE
            } else if value < 0.52 {
                DARK_MUTED
            } else if value < 0.75 {
                DARK_SECONDARY_TEXT
            } else if value < 0.94 {
                DARK_PRIMARY_TEXT
            } else {
                0xffffff
            };
            let gray = (tone & 0xff) as f32 / 255.;
            color.r = gray;
            color.g = gray;
            color.b = gray;
        }
    }
    color
}
pub fn rgb(value: u32) -> Rgba {
    palette(gpui_kit::rgb(value))
}
pub fn rgba(value: u32) -> Rgba {
    palette(gpui_kit::rgba(value))
}
