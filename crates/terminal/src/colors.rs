//! Palette and colour resolution: maps the emulator's colour model (named,
//! 256-indexed, truecolor) onto concrete RGB values on a dark theme.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

pub const DEFAULT_FG: Rgb = Rgb {
    r: 0xd4,
    g: 0xd4,
    b: 0xd4,
};
pub const DEFAULT_BG: Rgb = Rgb {
    r: 0x00,
    g: 0x00,
    b: 0x00,
};
pub const CURSOR: Rgb = Rgb {
    r: 0xe6,
    g: 0xe6,
    b: 0xe6,
};

const ANSI: [(u8, u8, u8); 16] = [
    (0x1c, 0x1c, 0x1c),
    (0xe0, 0x6c, 0x75),
    (0x98, 0xc3, 0x79),
    (0xe5, 0xc0, 0x7b),
    (0x61, 0xaf, 0xef),
    (0xc6, 0x78, 0xdd),
    (0x56, 0xb6, 0xc2),
    (0xc8, 0xcc, 0xd4),
    (0x5c, 0x63, 0x70),
    (0xf0, 0x8a, 0x92),
    (0xb1, 0xd8, 0x92),
    (0xf0, 0xd2, 0x95),
    (0x7e, 0xc0, 0xf5),
    (0xd7, 0x92, 0xea),
    (0x73, 0xc7, 0xd2),
    (0xf0, 0xf0, 0xf0),
];

/// The built-in colour for a palette index: 0-15 ansi, 16-231 the 6x6x6 cube,
/// 232-255 the grayscale ramp, then foreground / background / cursor.
pub fn palette(index: usize) -> Rgb {
    match index {
        0..=15 => {
            let (r, g, b) = ANSI[index];
            Rgb { r, g, b }
        }
        16..=231 => {
            let i = index - 16;
            let level = |n: usize| if n == 0 { 0 } else { (55 + 40 * n) as u8 };
            Rgb {
                r: level(i / 36),
                g: level(i / 6 % 6),
                b: level(i % 6),
            }
        }
        232..=255 => {
            let v = (8 + 10 * (index - 232)) as u8;
            Rgb { r: v, g: v, b: v }
        }
        256 | 267 => DEFAULT_FG,
        257 => DEFAULT_BG,
        258 => CURSOR,
        // Dim variants: the base colour at two thirds intensity.
        259..=266 => dim(palette(index - 259)),
        268 => dim(DEFAULT_FG),
        _ => DEFAULT_FG,
    }
}

fn dim(c: Rgb) -> Rgb {
    let d = |v: u8| (v as u16 * 2 / 3) as u8;
    Rgb {
        r: d(c.r),
        g: d(c.g),
        b: d(c.b),
    }
}

/// Resolve a cell colour, honouring runtime overrides (OSC 4/10/11).
pub fn resolve(color: Color, overrides: &Colors) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| palette(i as usize)),
        Color::Named(n) => {
            let i = n as usize;
            overrides[i].unwrap_or_else(|| palette(i))
        }
    }
}

/// Bright variant of an ansi colour, used for bold text.
pub fn bold_variant(color: Color) -> Color {
    match color {
        Color::Named(n) => Color::Named(match n {
            NamedColor::Black => NamedColor::BrightBlack,
            NamedColor::Red => NamedColor::BrightRed,
            NamedColor::Green => NamedColor::BrightGreen,
            NamedColor::Yellow => NamedColor::BrightYellow,
            NamedColor::Blue => NamedColor::BrightBlue,
            NamedColor::Magenta => NamedColor::BrightMagenta,
            NamedColor::Cyan => NamedColor::BrightCyan,
            NamedColor::White => NamedColor::BrightWhite,
            NamedColor::Foreground => NamedColor::BrightForeground,
            other => other,
        }),
        Color::Indexed(i @ 0..=7) => Color::Indexed(i + 8),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_ramp() {
        assert_eq!(palette(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            palette(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(palette(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(palette(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            palette(255),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }

    #[test]
    fn truecolor_and_override() {
        let mut colors = Colors::default();
        let spec = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(resolve(Color::Spec(spec), &colors), spec);
        assert_eq!(resolve(Color::Indexed(1), &colors), palette(1));
        colors[1] = Some(spec);
        assert_eq!(resolve(Color::Indexed(1), &colors), spec);
        assert_eq!(
            resolve(Color::Named(NamedColor::Background), &colors),
            DEFAULT_BG
        );
    }

    #[test]
    fn bold_brightens_ansi_only() {
        assert_eq!(
            bold_variant(Color::Named(NamedColor::Red)),
            Color::Named(NamedColor::BrightRed)
        );
        assert_eq!(bold_variant(Color::Indexed(2)), Color::Indexed(10));
        assert_eq!(bold_variant(Color::Indexed(100)), Color::Indexed(100));
    }
}
