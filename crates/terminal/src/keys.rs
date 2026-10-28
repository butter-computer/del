//! Keyboard → bytes. Plain printable text is deliberately *not* handled here:
//! it flows through GPUI's text-input path so IME composition works.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// CSI modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
fn modifier_param(k: &Keystroke) -> u8 {
    1 + k.modifiers.shift as u8 + 2 * k.modifiers.alt as u8 + 4 * k.modifiers.control as u8
}

fn csi_with_mod(final_byte: char, k: &Keystroke) -> String {
    match modifier_param(k) {
        1 => format!("\x1b[{final_byte}"),
        m => format!("\x1b[1;{m}{final_byte}"),
    }
}

fn tilde(code: u8, k: &Keystroke) -> String {
    match modifier_param(k) {
        1 => format!("\x1b[{code}~"),
        m => format!("\x1b[{code};{m}~"),
    }
}

fn function_key(n: u8, k: &Keystroke) -> Option<String> {
    let m = modifier_param(k);
    Some(match n {
        1..=4 => {
            let c = ['P', 'Q', 'R', 'S'][n as usize - 1];
            if m == 1 {
                format!("\x1bO{c}")
            } else {
                format!("\x1b[1;{m}{c}")
            }
        }
        5 => tilde(15, k),
        6 => tilde(17, k),
        7 => tilde(18, k),
        8 => tilde(19, k),
        9 => tilde(20, k),
        10 => tilde(21, k),
        11 => tilde(23, k),
        12 => tilde(24, k),
        13 => tilde(25, k),
        14 => tilde(26, k),
        15 => tilde(28, k),
        16 => tilde(29, k),
        17 => tilde(31, k),
        18 => tilde(32, k),
        19 => tilde(33, k),
        20 => tilde(34, k),
        _ => return None,
    })
}

/// Byte for ctrl + `c`, if there is one.
fn control_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// Encode `k` for the program, or `None` if it should go through text input
/// (or be handled by the app).
///
/// `option_as_meta`: treat Alt/Option as Meta (ESC prefix) instead of letting
/// the OS compose a character.
pub fn encode_key(k: &Keystroke, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    if m.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let plain = modifier_param(k) == 1;
    let alt_only = m.alt && !m.shift && !m.control;
    let key = k.key.as_str();

    let s: String = match key {
        "enter" => {
            if m.alt {
                "\x1b\r".into()
            } else if m.shift {
                "\n".into()
            } else {
                "\r".into()
            }
        }
        "tab" => {
            if m.shift && !m.control && !m.alt {
                "\x1b[Z".into()
            } else if m.alt {
                "\x1b\t".into()
            } else {
                "\t".into()
            }
        }
        "escape" => if m.alt { "\x1b\x1b" } else { "\x1b" }.into(),
        "backspace" => {
            if m.alt {
                "\x1b\x7f".into()
            } else if m.control {
                "\x08".into()
            } else {
                "\x7f".into()
            }
        }
        "delete" => tilde(3, k),
        "insert" => tilde(2, k),
        "pageup" => tilde(5, k),
        "pagedown" => tilde(6, k),
        "up" | "down" | "left" | "right" | "home" | "end" => {
            let c = match key {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            if alt_only && (key == "left" || key == "right") {
                // Option-arrow word motion, as macOS terminals do.
                if key == "left" { "\x1bb" } else { "\x1bf" }.into()
            } else if plain && app_cursor {
                format!("\x1bO{c}")
            } else {
                csi_with_mod(c, k)
            }
        }
        "space" if m.control => if m.alt { "\x1b\0" } else { "\0" }.into(),
        "space" if m.alt && option_as_meta => "\x1b ".into(),
        _ => {
            if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return function_key(n, k).map(String::into_bytes);
            }
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            if m.control {
                let b = control_byte(c)?;
                let mut out = Vec::with_capacity(2);
                if m.alt {
                    out.push(0x1b);
                }
                out.push(b);
                return Some(out);
            }
            if m.alt && option_as_meta {
                let ch = if m.shift {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                };
                format!("\x1b{ch}")
            } else {
                return None;
            }
        }
    };
    Some(s.into_bytes())
}

/// Bytes for pasting `text`: line breaks become CR, and in bracketed-paste
/// mode the text is wrapped and any embedded end marker is removed so pasted
/// content cannot escape the bracket. Outside bracketed paste, C0 controls
/// other than tab and CR (and the DEL byte) are stripped, as Alacritty does,
/// so a paste cannot inject key sequences.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let safe = normalized.replace("\x1b[201~", "").replace("\x1b[200~", "");
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        normalized
            .chars()
            .filter(|&c| !(c.is_control() && c != '\t' && c != '\r'))
            .collect::<String>()
            .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(s: &str) -> Option<String> {
        encode_key(&Keystroke::parse(s).unwrap(), TermMode::NONE, true)
            .map(|b| String::from_utf8(b).unwrap())
    }

    fn enc_mode(s: &str, mode: TermMode) -> Option<String> {
        encode_key(&Keystroke::parse(s).unwrap(), mode, true).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn plain_text_goes_to_ime() {
        assert_eq!(enc("a"), None);
        assert_eq!(enc("shift-a"), None);
        assert_eq!(enc("space"), None);
        assert_eq!(enc("cmd-c"), None);
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc("enter").unwrap(), "\r");
        assert_eq!(enc("alt-enter").unwrap(), "\x1b\r");
        assert_eq!(enc("tab").unwrap(), "\t");
        assert_eq!(enc("shift-tab").unwrap(), "\x1b[Z");
        assert_eq!(enc("escape").unwrap(), "\x1b");
        assert_eq!(enc("backspace").unwrap(), "\x7f");
        assert_eq!(enc("alt-backspace").unwrap(), "\x1b\x7f");
        assert_eq!(enc("delete").unwrap(), "\x1b[3~");
        assert_eq!(enc("pageup").unwrap(), "\x1b[5~");
        assert_eq!(enc("pagedown").unwrap(), "\x1b[6~");
    }

    #[test]
    fn arrows_respect_application_cursor_mode() {
        assert_eq!(enc("up").unwrap(), "\x1b[A");
        assert_eq!(enc_mode("up", TermMode::APP_CURSOR).unwrap(), "\x1bOA");
        assert_eq!(enc_mode("home", TermMode::APP_CURSOR).unwrap(), "\x1bOH");
        // Modified arrows always use the CSI form.
        assert_eq!(
            enc_mode("ctrl-right", TermMode::APP_CURSOR).unwrap(),
            "\x1b[1;5C"
        );
        assert_eq!(enc("shift-left").unwrap(), "\x1b[1;2D");
        assert_eq!(enc("alt-left").unwrap(), "\x1bb");
        assert_eq!(enc("alt-right").unwrap(), "\x1bf");
    }

    #[test]
    fn control_combos() {
        assert_eq!(enc("ctrl-c").unwrap(), "\x03");
        assert_eq!(enc("ctrl-a").unwrap(), "\x01");
        assert_eq!(enc("ctrl-z").unwrap(), "\x1a");
        assert_eq!(enc("ctrl-shift-a").unwrap(), "\x01");
        assert_eq!(enc("ctrl-space").unwrap(), "\0");
        assert_eq!(enc("ctrl-[").unwrap(), "\x1b");
        assert_eq!(enc("ctrl-\\").unwrap(), "\x1c");
        assert_eq!(enc("ctrl-alt-a").unwrap(), "\x1b\x01");
        assert_eq!(enc("ctrl-1"), None);
    }

    #[test]
    fn alt_is_meta_when_enabled() {
        assert_eq!(enc("alt-b").unwrap(), "\x1bb");
        assert_eq!(enc("alt-space").unwrap(), "\x1b ");
        assert_eq!(enc("ctrl-alt-space").unwrap(), "\x1b\0");
        assert_eq!(enc("alt-shift-b").unwrap(), "\x1bB");
        let k = Keystroke::parse("alt-b").unwrap();
        assert_eq!(encode_key(&k, TermMode::NONE, false), None);
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc("f1").unwrap(), "\x1bOP");
        assert_eq!(enc("f4").unwrap(), "\x1bOS");
        assert_eq!(enc("f5").unwrap(), "\x1b[15~");
        assert_eq!(enc("f12").unwrap(), "\x1b[24~");
        assert_eq!(enc("shift-f5").unwrap(), "\x1b[15;2~");
        assert_eq!(enc("ctrl-f1").unwrap(), "\x1b[1;5P");
    }

    #[test]
    fn plain_paste_strips_control_bytes_except_tab_and_cr() {
        assert_eq!(
            encode_paste("a\x1b[Ab\x03\tc\x7f\u{85}", TermMode::NONE),
            b"a[Ab\tc"
        );
        assert_eq!(
            encode_paste("x\x1by", TermMode::BRACKETED_PASTE),
            b"\x1b[200~x\x1by\x1b[201~"
        );
    }

    #[test]
    fn paste_plain_and_bracketed() {
        assert_eq!(encode_paste("a\nb\r\nc", TermMode::NONE), b"a\rb\rc");
        assert_eq!(
            encode_paste("hi", TermMode::BRACKETED_PASTE),
            b"\x1b[200~hi\x1b[201~"
        );
        assert_eq!(
            encode_paste("x\x1b[201~rm", TermMode::BRACKETED_PASTE),
            b"\x1b[200~xrm\x1b[201~"
        );
    }
}
