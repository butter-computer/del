//! Messages for the user, queued so one cannot overwrite another, with a
//! severity that decides whether the toast stays until dismissed.
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Something the user asked for failed: stays until dismissed.
    Error,
    /// Background housekeeping that failed; nothing for the user to do now.
    Warning,
    /// Informational; nothing failed.
    Info,
}

impl Severity {
    /// Errors stay until dismissed; housekeeping and information hide.
    pub fn sticky(self) -> bool {
        self == Severity::Error
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub severity: Severity,
    /// A short specific title ("Could not start the session"), if the message has one.
    pub title: Option<String>,
    pub message: String,
    /// Shown only while the session the notice concerns is still selected.
    pub undo: Option<Undo>,
}

/// A cheap reversal offered beside a notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    Unarchive(del_agent::SessionId),
}

impl Notice {
    pub fn new(severity: Severity, raw: &str) -> Self {
        let (title, message) = present(raw);
        Self {
            severity,
            title,
            message,
            undo: None,
        }
    }

    /// The same text for the same severity coalesces into one toast.
    pub fn key(&self) -> String {
        format!(
            "{:?}:{}:{}",
            self.severity,
            self.title.as_deref().unwrap_or_default(),
            self.message
        )
    }
}

/// Pending notices, oldest first. Repeats of one still waiting are dropped.
#[derive(Default)]
pub struct Notices {
    queue: VecDeque<Notice>,
}

impl Notices {
    pub fn push(&mut self, notice: Notice) {
        if notice.message.is_empty() {
            return;
        }
        if let Some(existing) = self.queue.iter_mut().find(|n| n.key() == notice.key()) {
            // Keep the newest Undo, if any.
            if notice.undo.is_some() {
                existing.undo = notice.undo;
            }
            return;
        }
        self.queue.push_back(notice);
    }

    pub fn drain(&mut self) -> Vec<Notice> {
        self.queue.drain(..).collect()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// Severity for messages set through the legacy `AppState::error` field:
/// failures of saves the user never asked for are housekeeping, and messages
/// that only say something will happen later are information.
pub fn classify(raw: &str) -> Severity {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("will retry") || lower.contains("is pending") {
        Severity::Info
    } else if lower.starts_with("could not save sidebar")
        || lower.starts_with("could not save terminal history")
        || lower.starts_with("could not save tab")
    {
        Severity::Warning
    } else {
        Severity::Error
    }
}

/// Splits "Could not X: detail" into a short title and a readable message, and
/// tidies raw provider text: backticks dropped, first letter capitalised and a
/// closing full stop added.
pub fn present(raw: &str) -> (Option<String>, String) {
    let text = raw.trim().replace('`', "");
    let split = text
        .strip_prefix("Could not ")
        .and_then(|rest| rest.find(": ").map(|at| at + "Could not ".len()))
        .filter(|at| *at <= 60);
    match split {
        Some(at) => {
            let title = text[..at].trim().to_owned();
            let detail = sentence(&text[at + 2..]);
            if detail.is_empty() {
                (None, sentence(&title))
            } else {
                (Some(title), detail)
            }
        }
        None => (None, sentence(&text)),
    }
}

fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?', '…', ')']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_could_not_into_title_and_capitalised_detail() {
        let (title, message) = present(
            "Could not start the session: the `codex` CLI was not found. Install it (`npm i -g @openai/codex`) and run `codex login`. Your message is saved; use Retry message.",
        );
        assert_eq!(title.as_deref(), Some("Could not start the session"));
        assert_eq!(
            message,
            "The codex CLI was not found. Install it (npm i -g @openai/codex) and run codex login. Your message is saved; use Retry message."
        );
        assert_eq!(
            present("the `codex` CLI was not found"),
            (None, "The codex CLI was not found.".into())
        );
        assert_eq!(
            present("Wait for this project's session to finish starting."),
            (
                None,
                "Wait for this project's session to finish starting.".into()
            )
        );
    }

    #[test]
    fn housekeeping_and_information_are_not_errors() {
        assert_eq!(
            classify("Browser data cleanup is pending; Del will retry at next launch."),
            Severity::Info
        );
        assert_eq!(
            classify("Could not save sidebar widths: disk full"),
            Severity::Warning
        );
        assert_eq!(
            classify("Could not start terminal: no such file"),
            Severity::Error
        );
        assert!(Severity::Error.sticky());
        assert!(!Severity::Warning.sticky() && !Severity::Info.sticky());
    }

    #[test]
    fn queue_keeps_order_and_coalesces_repeats() {
        let mut notices = Notices::default();
        notices.push(Notice::new(Severity::Error, "Could not save a: x"));
        notices.push(Notice::new(Severity::Error, "Could not save b: y"));
        notices.push(Notice::new(Severity::Error, "Could not save a: x"));
        notices.push(Notice::new(Severity::Info, ""));
        let drained = notices.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].title.as_deref(), Some("Could not save a"));
        assert_eq!(drained[1].message, "Y.");
        assert!(notices.is_empty());
    }
}
