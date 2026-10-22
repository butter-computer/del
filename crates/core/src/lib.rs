//! UI-agnostic domain state for the `del` app.
//!
//! This crate has no dependency on GPUI or any other UI toolkit, so it can be
//! unit-tested and reused from any front end.

pub mod composer;
pub mod shell;
pub mod tabs;
pub mod typography;
pub mod wallpaper;

pub use shell::{PAGE_SURFACE_RGBA, SHELL_SURFACE_RGBA, ShellMetrics};

/// A simple counter with a greeting, used as placeholder domain logic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counter {
    count: u64,
}

impl Counter {
    /// Create a counter starting at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current value.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Increase the counter by one.
    pub fn increment(&mut self) {
        self.count = self.count.saturating_add(1);
    }

    /// Reset the counter to zero.
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// A human-readable greeting that reflects the current count.
    pub fn greeting(&self) -> String {
        match self.count {
            0 => "Hello from del. Click the button to start counting.".to_string(),
            1 => "Clicked once.".to_string(),
            n => format!("Clicked {n} times."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        assert_eq!(Counter::new().count(), 0);
    }

    #[test]
    fn increments_and_resets() {
        let mut c = Counter::new();
        c.increment();
        c.increment();
        assert_eq!(c.count(), 2);
        c.reset();
        assert_eq!(c.count(), 0);
    }

    #[test]
    fn greeting_tracks_count() {
        let mut c = Counter::new();
        assert!(c.greeting().starts_with("Hello"));
        c.increment();
        assert_eq!(c.greeting(), "Clicked once.");
        c.increment();
        assert_eq!(c.greeting(), "Clicked 2 times.");
    }
}
