//! Rules for what the chat composer is allowed to send.

/// Turn the raw composer text into a message to send.
///
/// Returns `None` when there is nothing to send (empty or whitespace only);
/// otherwise the text with surrounding whitespace removed.
pub fn submission(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_text_is_not_a_submission() {
        assert_eq!(submission(""), None);
        assert_eq!(submission(" \n\t "), None);
    }

    #[test]
    fn text_is_trimmed_but_inner_newlines_survive() {
        assert_eq!(submission("  hi\nthere \n"), Some("hi\nthere".to_string()));
    }
}
