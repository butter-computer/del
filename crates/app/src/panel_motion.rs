//! One interruptible reveal shared by the sidebar header and its content.
use std::time::{Duration, Instant};

const DURATION: Duration = Duration::from_millis(220);

#[derive(Default)]
pub struct Reveal {
    from: f32,
    to: f32,
    started: Option<Instant>,
}

impl Reveal {
    pub fn settle(&mut self, open: bool) {
        self.to = if open { 1. } else { 0. };
        self.from = self.to;
        self.started = None;
    }

    pub fn set(&mut self, open: bool, reduced_motion: bool, now: Instant) {
        if reduced_motion {
            self.settle(open);
        } else {
            self.from = self.sample(now).0;
            self.to = if open { 1. } else { 0. };
            self.started = Some(now);
        }
    }

    pub fn sample(&self, now: Instant) -> (f32, bool) {
        let Some(started) = self.started else {
            return (self.to, false);
        };
        let t =
            (now.saturating_duration_since(started).as_secs_f32() / DURATION.as_secs_f32()).min(1.);
        let eased = 1. - (1. - t).powi(5);
        (
            self.from + (self.to - self.from) * eased,
            t < 1. && self.from != self.to,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_starts_at_the_current_position() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        reveal.set(true, false, now);
        let halfway = now + Duration::from_millis(60);
        let before = reveal.sample(halfway).0;
        assert!(before > 0. && before < 1.);
        reveal.set(false, false, halfway);
        assert_eq!(reveal.sample(halfway).0, before);
        assert!(reveal.sample(halfway + Duration::from_millis(30)).0 < before);
        assert_eq!(reveal.sample(halfway + DURATION), (0., false));
    }

    #[test]
    fn restored_and_reduced_motion_panels_settle_immediately() {
        let mut reveal = Reveal::default();
        reveal.settle(true);
        assert_eq!(reveal.sample(Instant::now()), (1., false));
        reveal.set(false, true, Instant::now());
        assert_eq!(reveal.sample(Instant::now()), (0., false));
    }
}
