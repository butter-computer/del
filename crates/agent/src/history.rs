//! Presentation cleanup for complete, restored provider records. Native files stay intact.
use crate::AgentEvent;

/// Remove actual terminal controls, not printable escape examples or XML/code.
/// Only use on complete history records: live deltas can split an escape sequence.
pub fn plain_text(text: &mut String) {
    if !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return;
    }
    struct Text(String);
    impl vte::Perform for Text {
        fn print(&mut self, c: char) {
            self.0.push(c);
        }
        fn execute(&mut self, byte: u8) {
            if matches!(byte, b'\n' | b'\r' | b'\t') {
                self.0.push(byte as char);
            }
        }
    }
    let mut output = Text(String::with_capacity(text.len()));
    vte::Parser::new().advance(&mut output, text.as_bytes());
    *text = output.0;
}

pub fn clean_events(events: &mut [AgentEvent]) {
    for event in events {
        match event {
            AgentEvent::UserMessage { text }
            | AgentEvent::AssistantMessage { text, .. }
            | AgentEvent::AssistantTextDelta { text, .. }
            | AgentEvent::ReasoningDelta { text, .. } => plain_text(text),
            AgentEvent::ToolCallStarted { input_summary, .. } => plain_text(input_summary),
            AgentEvent::ToolCallUpdated { output_summary, .. }
            | AgentEvent::ToolCallFinished { output_summary, .. } => plain_text(output_summary),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_controls_disappear_but_text_links_unicode_and_code_survive() {
        let mut text = "\x1b[2mNote\x1b[22m\n\t雪 🐄 \x1b]8;;https://example.com\x1b\\link\x1b]8;;\x07\x1b[0m\x1b[2K\x1b]0;window title\x07".to_owned();
        plain_text(&mut text);
        assert_eq!(text, "Note\n\t雪 🐄 link");
        let literal = r#"```xml
<command-name>/terminal-setup</command-name>
```
Examples: \x1b[2m, \\033[22m and [2m stay literal."#;
        let mut text = literal.to_owned();
        plain_text(&mut text);
        assert_eq!(text, literal);
    }
}
