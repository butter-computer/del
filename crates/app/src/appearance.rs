//! Persisted appearance shared by the custom chrome and component theme.
use crate::zoom::px as zpx;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Rgba};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
mod background;
mod themes;
pub use background::BackgroundAnimation;
static BACKGROUND: AtomicU8 = AtomicU8::new(0);
static MOUSE_REACTIVE: AtomicBool = AtomicBool::new(false);
pub use themes::ThemePreset;
static PRESET: AtomicU8 = AtomicU8::new(0);
static LIGHT: AtomicBool = AtomicBool::new(false);

/// Semantic action colors; neutrals stay neutral and color signals intent.
#[derive(Clone, Copy)]
pub enum ActionTone {
    Accent,
    Success,
    Danger,
    Warning,
}
impl ActionTone {
    pub fn foreground(self) -> Rgba {
        let value = match (self, light()) {
            (Self::Accent, _) => colors().accent,
            (Self::Danger, false) => 0xe58b8b,
            (Self::Danger, true) => 0xb83c3c,
            (Self::Success, false) => 0x86b596,
            (Self::Success, true) => 0x366f49,
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

/// The fill behind a secondary action. A literal neutral, so `rgba` maps it to
/// warm ink in light mode like every other neutral.
pub const QUIET_FILL: u32 = 0xffffff0f;

pub fn separator() -> Rgba {
    gpui_kit::rgba(if light() {
        0x3c321e14
    } else if preset() == ThemePreset::Vercel {
        0xffffff24
    } else {
        0xffffff0f
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[derive(Default)]
struct Preferences {
    light: bool,
    theme: ThemePreset,
    /// None when nothing valid is saved, including the removed "theme_default".
    #[serde(deserialize_with = "saved_background")]
    background: Option<BackgroundAnimation>,
    mouse_reactive: bool,
}
impl Preferences {
    fn starting_background(&self) -> BackgroundAnimation {
        self.background
            .unwrap_or_else(|| BackgroundAnimation::starting_for(self.theme))
    }
}
/// One unrecognised animation name must not cost someone their other preferences.
fn saved_background<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BackgroundAnimation>, D::Error> {
    use serde::Deserialize;
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok()))
}

pub fn light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

pub fn preset() -> ThemePreset {
    ThemePreset::ALL
        .get(PRESET.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or_default()
}
pub fn background() -> BackgroundAnimation {
    BackgroundAnimation::from_index(BACKGROUND.load(Ordering::Relaxed))
        .unwrap_or(BackgroundAnimation::Life)
}
pub fn set_background(animation: BackgroundAnimation, cx: &mut App) {
    BACKGROUND.store(animation as u8, Ordering::Relaxed);
    save();
    cx.refresh_windows();
}
pub fn mouse_reactive() -> bool {
    MOUSE_REACTIVE.load(Ordering::Relaxed)
}
pub fn set_mouse_reactive(enabled: bool, cx: &mut App) {
    MOUSE_REACTIVE.store(enabled, Ordering::Relaxed);
    save();
    cx.refresh_windows();
}

/// Theme accent in dark mode, darker ink-like accent in light mode. Alpha stays
/// deliberately low: the animated layer never competes with chat content.
pub fn background_tint() -> Rgba {
    let mut color = ActionTone::Accent.foreground();
    color.a = if light() { 0.16 } else { 0.14 };
    color
}

fn colors() -> themes::Palette {
    preset().palette(light())
}

pub fn secondary_text() -> Rgba {
    gpui_kit::rgb(colors().neutral[5])
}
/// Quiet metadata: times, section labels, disclosure arrows. Stays readable on
/// the selected row in every theme, at about #83837F in Graphite dark.
pub fn faint_text() -> Rgba {
    let mut color = secondary_text();
    color.a = if light() { 0.88 } else { 0.78 };
    color
}
pub fn composer_surface() -> Rgba {
    gpui_kit::rgb(colors().composer)
}
/// Queued follow-ups: the composer's own tint, one step darker, so the row and the
/// composer read as one object in every theme.
pub fn queue_surface() -> Rgba {
    let composer = composer_surface();
    let dim = if light() { 0.96 } else { 0.9 };
    Rgba {
        r: composer.r * dim,
        g: composer.g * dim,
        b: composer.b * dim,
        a: 1.,
    }
}
pub fn popup_surface() -> Rgba {
    gpui_kit::rgb(colors().popup)
}
pub fn shell_surface() -> Rgba {
    gpui_kit::rgb(colors().shell)
}
pub fn dragged_tab_surface() -> Rgba {
    gpui_kit::rgb(colors().neutral[2])
}
/// Code, diffs and tool output: one step below the page in every theme, so text
/// stays the brightest part and the animated background still reads through.
pub fn code_surface() -> Rgba {
    let mut surface = shell_surface();
    surface.a = 0.82;
    surface
}
pub fn page_surface() -> Rgba {
    gpui_kit::rgb(colors().neutral[0])
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
        Preferences {
            light,
            ..Preferences::default()
        }
    } else {
        serde_json::from_slice::<Preferences>(bytes).unwrap_or_default()
    }
}
pub fn load(cx: &mut App) {
    let preferences = std::fs::read(path())
        .map(|bytes| parse(&bytes))
        .unwrap_or_default();
    BACKGROUND.store(preferences.starting_background() as u8, Ordering::Relaxed);
    MOUSE_REACTIVE.store(preferences.mouse_reactive, Ordering::Relaxed);
    apply(preferences.light, preferences.theme, cx);
}
fn save() {
    if let Ok(bytes) = serde_json::to_vec(&Preferences {
        light: light(),
        theme: preset(),
        background: Some(background()),
        mouse_reactive: mouse_reactive(),
    }) {
        let _ = crate::sessions::write_atomic(&path(), &bytes, 0o644);
    }
}
pub fn set(light: bool, cx: &mut App) {
    apply(light, preset(), cx);
    save();
    cx.refresh_windows();
}
pub fn set_preset(theme: ThemePreset, cx: &mut App) {
    apply(light(), theme, cx);
    save();
    cx.refresh_windows();
}
fn apply(light: bool, preset: ThemePreset, cx: &mut App) {
    PRESET.store(preset as u8, Ordering::Relaxed);
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
        let palette = colors();
        let c = &mut theme.colors;
        let surface = gpui_kit::rgb(palette.neutral[1]).into();
        let hover = gpui_kit::rgb(palette.neutral[2]).into();
        let active = gpui_kit::rgb(palette.neutral[3]).into();
        let text = gpui_kit::rgb(palette.neutral[7]).into();
        let secondary = gpui_kit::rgb(palette.neutral[5]).into();
        let white = gpui_kit::rgb(palette.neutral[7]).into();
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
        c.primary = gpui_kit::rgb(palette.primary).into();
        c.primary_hover = mix(
            gpui_kit::rgb(palette.primary),
            gpui_kit::rgb(0xffffff),
            0.10,
        )
        .into();
        c.primary_active = mix(
            gpui_kit::rgb(palette.primary),
            gpui_kit::rgb(0x000000),
            0.10,
        )
        .into();
        c.primary_foreground = gpui_kit::rgb(palette.primary_foreground).into();
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
        c.selection = ActionTone::Accent.foreground().opacity(0.25).into();
        c.link = ActionTone::Accent.foreground().into();
        c.link_hover = mix(
            ActionTone::Accent.foreground(),
            gpui_kit::rgb(palette.neutral[7]),
            0.2,
        )
        .into();
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
        // The agreed muted green, for toasts and anything else the theme tints.
        c.success = gpui_kit::rgb(if light { 0x366f49 } else { 0x86b596 }).into();
        // A switch thumb is always the light part of the control, as on macOS.
        // Ink on a pale track reads as "on" when the switch is off.
        c.switch = if light {
            gpui_kit::rgba(palette.neutral[7] << 8 | 0x33).into()
        } else {
            gpui_kit::rgba(0xffffff24).into()
        };
        c.switch_thumb = if light {
            gpui_kit::rgb(0xffffff).into()
        } else {
            white
        };
        c.scrollbar = gpui_kit::transparent_black();
        c.scrollbar_thumb = active;
        c.scrollbar_thumb_hover = gpui_kit::rgb(palette.neutral[4]).into();
        // Markdown tables sit on the page, with a header on the code surface.
        c.table = gpui_kit::transparent_black();
        c.table_even = gpui_kit::transparent_black();
        c.table_head = code_surface().into();
        c.table_head_foreground = secondary;
        c.table_row_border = border;
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
        c.drop_target = ActionTone::Accent.foreground().opacity(0.08).into();
    });
}
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

// Map legacy neutral literals to the selected palette; preserve alpha and
// provider/status hues. Both custom chrome and component controls share it.
fn palette(color: Rgba) -> Rgba {
    map_neutral(color, colors())
}
fn map_neutral(mut color: Rgba, palette: themes::Palette) -> Rgba {
    let neutral = color.r.max(color.g).max(color.b) - color.r.min(color.g).min(color.b) < 0.06;
    if neutral {
        let value = (color.r + color.g + color.b) / 3.;
        let index = [0.08, 0.18, 0.26, 0.36, 0.52, 0.75, 0.94]
            .into_iter()
            .position(|threshold| value < threshold)
            .unwrap_or(7);
        let mapped = gpui_kit::rgb(palette.neutral[index]);
        color.r = mapped.r;
        color.g = mapped.g;
        color.b = mapped.b;
    }
    color
}

/// Desaturated replacements for the saturated status literals.
fn status(rgb: u32) -> u32 {
    status_for_mode(rgb, light())
}
fn status_for_mode(rgb: u32, light: bool) -> u32 {
    match (rgb, light) {
        // Diffs, account status, and success labels need dark ink in light mode.
        (0x5fc27e | 0x4cc38a, false) => 0x86b596,
        (0x5fc27e | 0x4cc38a, true) => 0x366f49,
        (0xf0716b, false) => 0xe58b8b,
        (0xf0716b, true) => 0xb83c3c,
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

    fn blend(top: u32, alpha: f64, under: u32) -> u32 {
        let channel = |shift: u32| {
            let (t, u) = (
                f64::from((top >> shift) & 0xff),
                f64::from((under >> shift) & 0xff),
            );
            ((t * alpha + u * (1. - alpha)).round() as u32) << shift
        };
        channel(16) | channel(8) | channel(0)
    }

    /// Every preset in both modes: body text, secondary text, faint metadata and the
    /// semantic and focus colours stay readable on every surface they sit on.
    #[test]
    fn every_theme_keeps_readable_text_on_every_surface() {
        let mut failures = Vec::new();
        for theme in ThemePreset::ALL {
            for light in [false, true] {
                let p = theme.palette(light);
                let surfaces = [
                    ("page", p.neutral[0]),
                    ("shell", p.shell),
                    ("composer", p.composer),
                    ("popup", p.popup),
                ];
                let (danger, success, warning): (u32, u32, u32) = if light {
                    (0xb83c3c, 0x366f49, 0x87621f)
                } else {
                    (0xe58b8b, 0x86b596, 0xd3b07a)
                };
                for (surface, under) in surfaces {
                    let faint = blend(p.neutral[5], if light { 0.88 } else { 0.78 }, under);
                    for (role, color, minimum) in [
                        ("text", p.neutral[7], 7.),
                        ("secondary", p.neutral[5], 4.5),
                        ("faint", faint, 3.),
                        ("danger", danger, 3.),
                        ("success", success, 3.),
                        ("warning", warning, 3.),
                        ("focus ring", p.accent, 3.),
                    ] {
                        let ratio = contrast(color, under);
                        if ratio < minimum {
                            failures.push(format!(
                                "{} {} {role} on {surface}: {ratio:.2} < {minimum}",
                                theme.name(),
                                if light { "light" } else { "dark" },
                            ));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn saved_theme_tolerates_legacy_unknown_and_corrupt_files() {
        assert!(parse(b"true").light);
        assert!(!parse(b"false").light);
        assert!(parse(br#"{"light":true}"#).light);
        // Removed opacity settings and future fields never break loading.
        assert!(parse(br#"{"light":true,"opacity":0.4,"future":[1]}"#).light);
        assert!(!parse(br#"{}"#).light);
        assert!(!parse(b"{not json").light);
        let bytes = serde_json::to_vec(&Preferences {
            light: true,
            ..Preferences::default()
        })
        .unwrap();
        assert!(parse(&bytes).light);
    }
    #[test]
    fn every_preset_round_trips_and_unknown_names_preserve_color_mode() {
        for theme in ThemePreset::ALL {
            for light in [false, true] {
                let bytes = serde_json::to_vec(&Preferences {
                    light,
                    theme,
                    ..Preferences::default()
                })
                .unwrap();
                let restored = parse(&bytes);
                assert_eq!(restored.theme, theme);
                assert_eq!(restored.light, light);
            }
            assert_eq!(ThemePreset::ALL[theme as usize], theme);
        }
        let future = parse(br#"{"light":true,"theme":"future-theme"}"#);
        assert!(future.light);
        assert_eq!(future.theme, ThemePreset::Graphite);
        assert_eq!(parse(b"true").theme, ThemePreset::Graphite);
    }

    fn luminance(value: u32) -> f32 {
        let color = gpui_kit::rgb(value);
        let linear = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }
    fn contrast(a: u32, b: u32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn diff_and_success_ink_remain_readable_in_both_graphite_modes() {
        for light in [false, true] {
            let p = ThemePreset::Graphite.palette(light);
            for background in [p.shell, p.composer, p.popup, p.neutral[0]] {
                for literal in [0x5fc27e, 0x4cc38a, 0xf0716b] {
                    assert!(contrast(status_for_mode(literal, light), background) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn all_palettes_keep_readable_text_accents_and_surface_hierarchy() {
        for theme in ThemePreset::ALL {
            for light in [false, true] {
                let p = theme.palette(light);
                for background in [
                    p.shell,
                    p.composer,
                    p.popup,
                    p.neutral[0],
                    p.neutral[1],
                    p.neutral[2],
                    p.neutral[3],
                ] {
                    for text in [p.neutral[5], p.neutral[6], p.neutral[7], p.accent] {
                        assert!(
                            contrast(text, background) >= 4.5,
                            "{theme:?} light={light}: {text:06x} on {background:06x}: {}",
                            contrast(text, background)
                        );
                    }
                }
                assert!(
                    contrast(p.primary_foreground, p.primary) >= 4.5,
                    "{theme:?}"
                );
                assert_eq!(luminance(p.composer) < luminance(p.neutral[0]), light);
                assert!(luminance(p.shell) <= luminance(p.neutral[0]));
            }
        }
    }

    #[test]
    fn neutral_mapping_preserves_transparency_and_provider_colors() {
        for theme in ThemePreset::ALL {
            for light in [false, true] {
                let p = theme.palette(light);
                let neutral = gpui_kit::rgba(0xffffff80);
                let mapped = map_neutral(neutral, p);
                assert_eq!(mapped.a, neutral.a);
                assert_eq!(mapped.r, gpui_kit::rgb(p.neutral[7]).r);
                let provider = gpui_kit::rgb(0xd97757);
                assert_eq!(map_neutral(provider, p), provider);
            }
        }
    }
    #[test]
    fn background_preferences_migrate_and_round_trip_independently() {
        let legacy = parse(br#"{"light":false,"theme":"vercel"}"#);
        assert_eq!(legacy.starting_background(), BackgroundAnimation::Off);
        // The removed Theme default option becomes what it was showing, for that theme.
        let vercel = parse(br#"{"theme":"vercel","background":"theme_default"}"#);
        assert_eq!(vercel.starting_background(), BackgroundAnimation::Off);
        let tide = parse(br#"{"theme":"tide","background":"theme_default","mouse_reactive":true}"#);
        assert_eq!(tide.starting_background(), BackgroundAnimation::Life);
        assert!(tide.mouse_reactive);
        for background in BackgroundAnimation::ALL {
            let bytes = serde_json::to_vec(&Preferences {
                light: true,
                theme: ThemePreset::Tide,
                background: Some(background),
                ..Preferences::default()
            })
            .unwrap();
            let restored = parse(&bytes);
            assert_eq!(restored.background, Some(background));
            assert_eq!(restored.theme, ThemePreset::Tide);
            assert!(restored.light);
        }
        let future = parse(br#"{"light":true,"theme":"tide","background":"future"}"#);
        assert_eq!(future.starting_background(), BackgroundAnimation::Life);
        assert_eq!(future.theme, ThemePreset::Tide);
        assert!(future.light);
    }
    #[test]
    fn mouse_reactivity_migrates_and_persists_with_other_preferences() {
        let legacy = parse(br#"{"light":true,"theme":"tide","background":"liquid"}"#);
        assert!(!legacy.mouse_reactive);
        // Files saved before rain haptics were removed still load, keeping everything else.
        assert_eq!(
            parse(br#"{"background":"rain","rain_haptics":true}"#).background,
            Some(BackgroundAnimation::Rain)
        );
        for mouse_reactive in [false, true] {
            let preferences = Preferences {
                mouse_reactive,
                ..legacy
            };
            let restored = parse(&serde_json::to_vec(&preferences).unwrap());
            assert_eq!(restored.mouse_reactive, mouse_reactive);
            assert!(restored.light);
            assert_eq!(restored.theme, ThemePreset::Tide);
            assert_eq!(restored.background, Some(BackgroundAnimation::Liquid));
        }
    }
}
