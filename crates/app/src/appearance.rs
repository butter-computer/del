//! Persisted appearance shared by the custom chrome and component theme.
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Rgba, px};
use std::sync::atomic::{AtomicBool, Ordering};
static LIGHT: AtomicBool = AtomicBool::new(false);

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}
fn path() -> std::path::PathBuf {
    del_agent::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("appearance.json")
}
pub fn load(cx: &mut App) {
    let light = std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
        .unwrap_or(false);
    apply(light, cx);
}
pub fn set(light: bool, cx: &mut App) {
    apply(light, cx);
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&light) {
        let _ = std::fs::write(path, bytes);
    }
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
        theme.font_size = px(del_core::typography::TEXT_MD);
        theme.colors.popover = rgb(0x2b2b2e).into();
        theme.colors.popover_foreground = rgb(0xf0f0f0).into();
        theme.colors.border = rgba(0xffffff14).into();
        theme.colors.accent = rgba(0xffffff14).into();
        theme.colors.accent_foreground = rgb(0xf5f5f5).into();
    });
}
// The custom chrome uses neutral dark palette values. Map those neutrals to the
// light palette, preserving semantic/provider colors and all alpha values.
fn palette(mut color: Rgba) -> Rgba {
    if light() && color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b) < 0.06 {
        color.r = 0.98 - color.r * 0.88;
        color.g = 0.98 - color.g * 0.88;
        color.b = 0.98 - color.b * 0.88;
    }
    color
}
pub fn rgb(value: u32) -> Rgba {
    palette(gpui_kit::rgb(value))
}
pub fn rgba(value: u32) -> Rgba {
    palette(gpui_kit::rgba(value))
}
