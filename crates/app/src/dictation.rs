//! Microphone lifetime is tied to the active composer, never a provider session.
use std::time::{Duration, Instant};
const REVEAL_DURATION: Duration = Duration::from_millis(80);
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Listening,
    Finishing,
}
pub enum Event {
    Listening,
    Text(String),
    Finishing,
    Done,
    Error(String),
}

/// Replace cumulative partial transcripts, preserving the draft that preceded them.
pub struct Draft {
    prefix: String,
    pub displayed: String,
    target: String,
    reveal_from: usize,
    reveal_started: Instant,
}
impl Draft {
    pub fn new(text: String) -> Self {
        let prefix = if !text.is_empty() && !text.ends_with(char::is_whitespace) {
            format!("{text} ")
        } else {
            text.clone()
        };
        Self {
            prefix,
            target: text.clone(),
            displayed: text,
            reveal_from: 0,
            reveal_started: Instant::now(),
        }
    }
    pub fn queue(&mut self, current: &str, transcript: &str, now: Instant) -> bool {
        if current != self.displayed {
            return false;
        }
        if transcript.is_empty() {
            return true;
        }
        let target = format!("{}{transcript}", self.prefix);
        if target != self.target {
            // Only reveal the changed suffix; settled text never replays.
            self.reveal_from = self
                .displayed
                .chars()
                .zip(target.chars())
                .take_while(|(left, right)| left == right)
                .map(|(ch, _)| ch.len_utf8())
                .sum();
            self.target = target;
            self.reveal_started = now;
        }
        true
    }
    pub fn advance(&mut self, current: &str, now: Instant, immediate: bool) -> Option<String> {
        if current != self.displayed {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.reveal_started);
        if immediate || elapsed >= REVEAL_DURATION {
            self.displayed = self.target.clone();
        } else {
            let t = elapsed.as_secs_f32() / REVEAL_DURATION.as_secs_f32();
            let progress = 1.0 - (1.0 - t).powi(3);
            let suffix = &self.target[self.reveal_from..];
            let count = (suffix.chars().count() as f32 * progress).ceil() as usize;
            let end = self.reveal_from
                + suffix
                    .chars()
                    .take(count)
                    .map(char::len_utf8)
                    .sum::<usize>();
            // Don't erase the old suffix while a correction is still appearing.
            if end == self.target.len() || end >= self.displayed.len() {
                self.displayed = self.target[..end].to_owned();
            }
        }
        Some(self.displayed.clone())
    }
    pub fn animating(&self) -> bool {
        self.displayed != self.target
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::Event;
    use std::{
        ffi::{CStr, c_char, c_void},
        marker::PhantomData,
        rc::Rc,
    };
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
    unsafe extern "C" {
        fn del_dictation_start(
            callback: extern "C" fn(*mut c_void, *const c_char, *const c_char),
            context: *mut c_void,
        ) -> *mut c_void;
        fn del_dictation_finish(pointer: *mut c_void);
        fn del_dictation_release(pointer: *mut c_void);
    }
    pub struct Recorder {
        pointer: *mut c_void,
        _sender: Box<UnboundedSender<Event>>,
        _main_thread: PhantomData<Rc<()>>,
    }
    impl Recorder {
        pub fn start() -> (Self, UnboundedReceiver<Event>) {
            let (tx, rx) = unbounded_channel();
            let mut sender = Box::new(tx);
            let pointer = unsafe {
                del_dictation_start(
                    receive,
                    (&mut *sender as *mut UnboundedSender<Event>).cast(),
                )
            };
            if pointer.is_null() {
                let _ = sender.send(Event::Error("Voice input could not start.".into()));
            }
            (
                Self {
                    pointer,
                    _sender: sender,
                    _main_thread: PhantomData,
                },
                rx,
            )
        }
        pub fn finish(&self) {
            unsafe { del_dictation_finish(self.pointer) }
        }
    }
    impl Drop for Recorder {
        fn drop(&mut self) {
            // Clear the native callback before the boxed sender can be freed.
            unsafe { del_dictation_release(self.pointer) };
        }
    }
    extern "C" fn receive(context: *mut c_void, kind: *const c_char, text: *const c_char) {
        if context.is_null() || kind.is_null() || text.is_null() {
            return;
        }
        let sender = unsafe { &*context.cast::<UnboundedSender<Event>>() };
        let kind = unsafe { CStr::from_ptr(kind) }.to_bytes();
        let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        let event = match kind {
            b"listening" => Event::Listening,
            b"text" => Event::Text(text.into_owned()),
            b"finishing" => Event::Finishing,
            b"done" => Event::Done,
            b"error" => Event::Error(text.into_owned()),
            _ => return,
        };
        let _ = sender.send(event);
    }
}
#[cfg(target_os = "macos")]
pub use native::Recorder;
#[cfg(not(target_os = "macos"))]
pub struct Recorder;
#[cfg(not(target_os = "macos"))]
impl Recorder {
    pub fn start() -> (Self, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = tx.send(Event::Error("Voice input requires macOS.".into()));
        (Self, rx)
    }
    pub fn finish(&self) {}
}

pub struct Dictation {
    pub recorder: Recorder,
    pub owner: u64,
    pub generation: u64,
    pub phase: Phase,
    pub draft: Draft,
    pub animation_running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    impl Draft {
        fn update(&mut self, current: &str, text: &str) -> Option<String> {
            let now = Instant::now();
            self.queue(current, text, now).then_some(())?;
            self.advance(current, now, true)
        }
    }
    #[test]
    fn fast_reveal_preserves_prefix_and_flushes_before_send() {
        let now = Instant::now();
        let mut draft = Draft::new("Before.".into());
        assert!(draft.queue("Before.", "one two three four five six", now));
        let middle = draft
            .advance("Before.", now + Duration::from_millis(20), false)
            .unwrap();
        assert!(middle.starts_with("Before. ") && draft.animating());
        assert_eq!(
            draft.advance(&middle, now, true).unwrap(),
            "Before. one two three four five six"
        );
        assert!(!draft.animating());
    }
    #[test]
    fn revisions_unicode_and_user_edits_are_safe() {
        let now = Instant::now();
        let mut draft = Draft::new("पहले ".into());
        assert!(draft.queue("पहले ", "नमस्ते 🌍", now));
        let middle = draft
            .advance("पहले ", now + Duration::from_millis(16), false)
            .unwrap();
        assert!(draft.queue(&middle, "नमस्ते 🌍!", now));
        let result = draft
            .advance(&middle, now + REVEAL_DURATION, false)
            .unwrap();
        assert_eq!(result, "पहले नमस्ते 🌍!");
        assert!(draft.advance("Manual edit", now, true).is_none());
    }
    #[test]
    fn partial_results_replace_instead_of_duplicating_and_preserve_existing_text() {
        let mut draft = Draft::new("Existing text".into());
        assert_eq!(
            draft.update("Existing text", "hello").as_deref(),
            Some("Existing text hello")
        );
        assert_eq!(
            draft
                .update("Existing text hello", "Hello world.")
                .as_deref(),
            Some("Existing text Hello world.")
        );
        assert!(draft.update("User edited this", "Later result").is_none());
        let mut empty = Draft::new(String::new());
        assert_eq!(empty.update("", "Hello").as_deref(), Some("Hello"));
        let mut newline = Draft::new("Notes:\n".into());
        assert_eq!(
            newline.update("Notes:\n", "hello").as_deref(),
            Some("Notes:\nhello")
        );
    }
}
