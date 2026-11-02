//! App-wide scale shared by the shell and embedded terminal.
use std::cell::Cell;

pub const DEFAULT: u16 = 100;
pub const MIN: u16 = 70;
pub const MAX: u16 = 200;
pub const STEP: i16 = 10;
// All layout and terminal painting runs on the UI thread.
thread_local! { static PERCENT: Cell<u16> = const { Cell::new(DEFAULT) }; }

pub fn clamp(percent: i16) -> u16 {
    percent.clamp(MIN as i16, MAX as i16) as u16
}
pub fn percent() -> u16 {
    PERCENT.get()
}
pub fn set(percent: i16) {
    PERCENT.set(clamp(percent));
}
pub fn factor() -> f32 {
    percent() as f32 / 100.
}
/// Scale a design measurement; never apply to coordinates measured by GPUI.
pub fn scaled(value: f32) -> f32 {
    value * factor()
}
/// Convert a measured pointer/viewport distance back to design units.
pub fn unscaled(value: f32) -> f32 {
    value / factor()
}

#[cfg(test)]
mod tests {
    #[test]
    fn limits_include_default_and_clamp_repeated_shortcuts() {
        assert_eq!(super::clamp(-100), super::MIN);
        assert_eq!(super::clamp(300), super::MAX);
        assert_eq!(super::clamp(100), super::DEFAULT);
        let mut value = super::DEFAULT;
        for _ in 0..100 {
            value = super::clamp(value as i16 + super::STEP);
        }
        assert_eq!(value, super::MAX);
        for _ in 0..100 {
            value = super::clamp(value as i16 - super::STEP);
        }
        assert_eq!(value, super::MIN);
    }

    #[test]
    fn set_clamps_and_scaling_round_trips() {
        // Thread-local: other tests on other threads are unaffected.
        assert_eq!(super::percent(), super::DEFAULT);
        assert_eq!(super::factor(), 1.0);
        super::set(i16::MAX);
        assert_eq!(super::percent(), super::MAX);
        assert_eq!(super::scaled(13.), 26.);
        assert_eq!(super::unscaled(26.), 13.);
        super::set(i16::MIN);
        assert_eq!(super::percent(), super::MIN);
        assert!((super::unscaled(super::scaled(19.)) - 19.).abs() < 1e-4);
        super::set(155);
        assert_eq!(super::percent(), 155);
        super::set(super::DEFAULT as i16);
        assert_eq!(super::scaled(8.), 8.);
    }
}
