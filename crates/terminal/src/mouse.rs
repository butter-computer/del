//! Mouse reporting: encodes clicks, drags, motion and wheel for programs that
//! turned on a mouse mode (xterm 1000/1002/1003, with SGR 1006 or legacy).

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    /// Motion; `button` says which button is held, if any.
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Whether any mouse reporting mode is on.
pub fn reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// Encode a mouse event at 0-based `col`/`row`, or `None` if the active mode
/// does not report it.
pub fn encode_mouse(
    action: MouseAction,
    button: MouseButton,
    col: usize,
    row: usize,
    mods: MouseMods,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let is_wheel = matches!(button, MouseButton::WheelUp | MouseButton::WheelDown);
    match action {
        MouseAction::Motion => {
            let any = mode.contains(TermMode::MOUSE_MOTION);
            let drag = mode.contains(TermMode::MOUSE_DRAG) && button != MouseButton::None;
            if !(any || drag) {
                return None;
            }
        }
        _ => {
            if !reporting(mode) {
                return None;
            }
            if is_wheel && action == MouseAction::Release {
                return None;
            }
        }
    }

    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    // Legacy encoding has a single "release" code and cannot say which button.
    if action == MouseAction::Release && !sgr {
        code = 3;
    }
    if action == MouseAction::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;

    if sgr {
        let end = if action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes())
    } else {
        // Legacy: one byte per value, offset by 32; unrepresentable past 222.
        if col > 222 || row > 222 {
            return None;
        }
        Some(vec![
            0x1b,
            b'[',
            b'M',
            32 + code as u8,
            33 + col as u8,
            33 + row as u8,
        ])
    }
}

/// Alternate-scroll: in the alternate screen with no mouse mode, the wheel
/// becomes arrow keys. Positive `lines` scrolls up.
pub fn wheel_as_arrows(lines: i32, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) || reporting(mode) {
        return None;
    }
    let up = lines > 0;
    let seq: &[u8] = match (up, mode.contains(TermMode::APP_CURSOR)) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    Some(seq.repeat(lines.unsigned_abs() as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO: MouseMods = MouseMods {
        shift: false,
        alt: false,
        ctrl: false,
    };

    fn sgr() -> TermMode {
        TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE
    }

    #[test]
    fn off_when_no_mode() {
        let r = encode_mouse(
            MouseAction::Press,
            MouseButton::Left,
            0,
            0,
            NO,
            TermMode::NONE,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn sgr_press_release() {
        let p = encode_mouse(MouseAction::Press, MouseButton::Left, 4, 9, NO, sgr()).unwrap();
        assert_eq!(p, b"\x1b[<0;5;10M");
        let r = encode_mouse(MouseAction::Release, MouseButton::Right, 4, 9, NO, sgr()).unwrap();
        assert_eq!(r, b"\x1b[<2;5;10m");
    }

    #[test]
    fn modifiers_and_wheel() {
        let mods = MouseMods {
            shift: true,
            alt: false,
            ctrl: true,
        };
        let r = encode_mouse(MouseAction::Press, MouseButton::Left, 0, 0, mods, sgr()).unwrap();
        assert_eq!(r, b"\x1b[<20;1;1M");
        let w = encode_mouse(MouseAction::Press, MouseButton::WheelDown, 1, 1, NO, sgr()).unwrap();
        assert_eq!(w, b"\x1b[<65;2;2M");
    }

    #[test]
    fn legacy_encoding() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let p = encode_mouse(MouseAction::Press, MouseButton::Left, 0, 0, NO, mode).unwrap();
        assert_eq!(p, [0x1b, b'[', b'M', 32, 33, 33]);
        let r = encode_mouse(MouseAction::Release, MouseButton::Left, 0, 0, NO, mode).unwrap();
        assert_eq!(r[3], 35);
        assert_eq!(
            encode_mouse(MouseAction::Press, MouseButton::Left, 300, 0, NO, mode),
            None
        );
    }

    #[test]
    fn motion_filtering() {
        let click = sgr();
        assert_eq!(
            encode_mouse(MouseAction::Motion, MouseButton::Left, 0, 0, NO, click),
            None
        );
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert!(encode_mouse(MouseAction::Motion, MouseButton::Left, 0, 0, NO, drag).is_some());
        assert_eq!(
            encode_mouse(MouseAction::Motion, MouseButton::None, 0, 0, NO, drag),
            None
        );
        let any = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        let m = encode_mouse(MouseAction::Motion, MouseButton::None, 0, 0, NO, any).unwrap();
        assert_eq!(m, b"\x1b[<35;1;1M");
    }

    #[test]
    fn alternate_scroll() {
        let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL;
        assert_eq!(wheel_as_arrows(2, mode).unwrap(), b"\x1b[A\x1b[A");
        assert_eq!(
            wheel_as_arrows(-1, mode | TermMode::APP_CURSOR).unwrap(),
            b"\x1bOB"
        );
        assert_eq!(wheel_as_arrows(1, TermMode::ALTERNATE_SCROLL), None);
        assert_eq!(
            wheel_as_arrows(1, mode | TermMode::MOUSE_REPORT_CLICK),
            None
        );
    }
}
