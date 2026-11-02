//! Back/forward history of the sessions and tabs the user has visited.
//!
//! Works like a browser: visiting something new drops the forward entries,
//! and stepping skips entries that no longer exist (closed drafts, deleted
//! sessions) without forgetting them.
use crate::state::Tab;

/// Older visits beyond this are forgotten.
const LIMIT: usize = 64;

#[derive(Default)]
pub struct NavHistory {
    entries: Vec<Tab>,
    index: usize,
}

impl NavHistory {
    /// Record that `tab` is now shown. Stepping lands on an existing entry,
    /// so it is not recorded again.
    pub fn visit(&mut self, tab: Tab) {
        if self.entries.get(self.index) == Some(&tab) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(tab);
        if self.entries.len() > LIMIT {
            self.entries.remove(0);
        }
        self.index = self.entries.len() - 1;
    }

    fn target(&self, delta: isize, valid: &impl Fn(&Tab) -> bool) -> Option<usize> {
        let mut at = self.index as isize;
        loop {
            at += delta;
            let entry = self.entries.get(usize::try_from(at).ok()?)?;
            if valid(entry) && Some(entry) != self.entries.get(self.index) {
                return Some(at as usize);
            }
        }
    }

    pub fn can_step(&self, delta: isize, valid: impl Fn(&Tab) -> bool) -> bool {
        self.target(delta, &valid).is_some()
    }

    /// Move one valid entry back (`-1`) or forward (`1`) and return it.
    pub fn step(&mut self, delta: isize, valid: impl Fn(&Tab) -> bool) -> Option<Tab> {
        let at = self.target(delta, &valid)?;
        self.index = at;
        Some(self.entries[at].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use del_agent::SessionId;

    fn tab(name: &str) -> Tab {
        Tab::Session(SessionId::new(name))
    }

    #[test]
    fn steps_skip_missing_entries_and_new_visits_drop_forward_history() {
        let mut history = NavHistory::default();
        for name in ["a", "b", "c"] {
            history.visit(tab(name));
        }
        let all = |_: &Tab| true;
        assert!(!history.can_step(1, all));
        assert_eq!(history.step(-1, |t| t != &tab("b")), Some(tab("a")));
        assert!(!history.can_step(-1, all));
        assert_eq!(history.step(1, all), Some(tab("b")));
        history.visit(tab("d"));
        assert!(!history.can_step(1, all));
        assert_eq!(history.step(-1, all), Some(tab("b")));
        assert_eq!(history.step(-1, all), Some(tab("a")));
    }
}
