//! Recognize an opt-in local command without sending its Enter to the child.
//! Unknown cursor edits invalidate tracking until the next clear or submission.
#[derive(Default)]
pub(crate) struct ChatCommand {
    line: String,
    uncertain: bool,
}

impl ChatCommand {
    pub fn input(&mut self, bytes: &[u8]) -> bool {
        if bytes == b"\r" {
            let command = !self.uncertain && self.line.trim() == "/chat";
            self.clear();
            return command;
        }
        if matches!(bytes, b"\x03" | b"\x15") {
            self.clear();
        } else if matches!(bytes, b"\x7f" | b"\x08") {
            self.line.pop();
        } else {
            let text = bytes
                .strip_prefix(b"\x1b[200~")
                .and_then(|text| text.strip_suffix(b"\x1b[201~"))
                .unwrap_or(bytes);
            match std::str::from_utf8(text) {
                Ok(text)
                    if !text.chars().any(char::is_control)
                        && self.line.len() + text.len() <= 4096 =>
                {
                    self.line.push_str(text)
                }
                _ => {
                    self.uncertain = true;
                    self.line.clear();
                }
            }
        }
        false
    }

    pub fn invalidate(&mut self) {
        if !self.line.is_empty() {
            self.uncertain = true;
            self.line.clear();
        }
    }

    fn clear(&mut self) {
        self.line.clear();
        self.uncertain = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_edited_and_pasted_chat_are_local_only_on_enter() {
        let mut command = ChatCommand::default();
        for byte in b"/chax" {
            assert!(!command.input(&[*byte]));
        }
        command.input(b"\x7f");
        command.input(b"t");
        assert!(command.input(b"\r"));
        assert!(!command.input(b"\r"));
        command.input(b"\x1b[200~/chat\x1b[201~");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn ordinary_input_arguments_multiline_and_unknown_edits_are_not_commands() {
        for text in [
            b"explain /chat".as_slice(),
            b"/chat more",
            b"/chat\n",
            b"/chat\rhello",
        ] {
            let mut command = ChatCommand::default();
            assert!(!command.input(text));
            assert!(!command.input(b"\r"));
        }
        let mut command = ChatCommand::default();
        command.input(b"\x1b[A");
        command.input(b"/chat");
        assert!(!command.input(b"\r"));
        command.input(b"draft");
        command.input(b"\x15");
        command.input(b"/chat");
        assert!(command.input(b"\r"));
    }

    fn typed(command: &mut ChatCommand, text: &str) {
        for ch in text.chars() {
            assert!(!command.input(ch.to_string().as_bytes()));
        }
    }

    #[test]
    fn only_the_exact_lowercase_command_matches() {
        for text in [
            "/CHAT", "/Chat", "/cha", "/chats", "chat", "//chat", "/chat/",
        ] {
            let mut command = ChatCommand::default();
            typed(&mut command, text);
            assert!(!command.input(b"\r"), "{text}");
        }
        // Surrounding spaces are not arguments.
        let mut command = ChatCommand::default();
        typed(&mut command, "  /chat  ");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn cursor_history_completion_and_newline_keys_make_the_line_uncertain() {
        for edit in [
            b"\x1b[A".as_slice(), // history
            b"\x1bOA\x1bOA",      // wheel as arrows (alternate scroll)
            b"\x1b[D",            // cursor left
            b"\x1bOH",            // home (application cursor)
            b"\t",                // completion
            b"\x1b",              // escape
            b"\x17",              // ctrl-w
            b"\x0b",              // ctrl-k
            b"\n",                // shift-enter newline
            b"\x1b\r",            // alt-enter newline
        ] {
            let mut command = ChatCommand::default();
            typed(&mut command, "/ch");
            command.input(edit);
            typed(&mut command, "at");
            assert!(!command.input(b"\r"), "{edit:?}");
            // Submitting clears the uncertainty for the next, empty prompt.
            typed(&mut command, "/chat");
            assert!(command.input(b"\r"), "{edit:?}");
        }
    }

    #[test]
    fn uncertainty_survives_backspace_and_typing_until_a_clear_or_submit() {
        let mut command = ChatCommand::default();
        command.input(b"\x1b[A");
        command.input(b"\x7f");
        command.input(b"\x08");
        typed(&mut command, "/chat");
        assert!(!command.input(b"\r"));
        // Ctrl-C clears the editor, so tracking is certain again.
        command.input(b"\x1b[A");
        command.input(b"\x03");
        typed(&mut command, "/chat");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn backspace_on_an_empty_line_and_ctrl_h_edit_the_tracked_line() {
        let mut command = ChatCommand::default();
        command.input(b"\x7f");
        command.input(b"\x7f");
        typed(&mut command, "/chatx");
        command.input(b"\x08");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn pointer_invalidation_only_affects_a_non_empty_line() {
        let mut command = ChatCommand::default();
        command.invalidate();
        typed(&mut command, "/chat");
        assert!(command.input(b"\r"));
        typed(&mut command, "/chat");
        command.invalidate();
        assert!(!command.input(b"\r"));
        typed(&mut command, "/chat");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn chunked_pastes_invalid_utf8_and_oversized_lines_are_not_commands() {
        for chunk in [
            b"/chat\r".as_slice(),
            b"\x1b[200~/chat\r\x1b[201~",
            b"\x1b[200~/chat\n\x1b[201~",
            b"\x1b[200~/chat",
            b"\xff/chat",
        ] {
            let mut command = ChatCommand::default();
            assert!(!command.input(chunk));
            assert!(!command.input(b"\r"), "{chunk:?}");
        }
        let mut command = ChatCommand::default();
        command.input("x".repeat(4096).as_bytes());
        command.input(b"/chat");
        assert!(!command.input(b"\r"));
        // Unicode text is tracked as text.
        typed(&mut command, "界");
        command.input(b"\x7f");
        typed(&mut command, "/chat");
        assert!(command.input(b"\r"));
    }

    #[test]
    fn a_pasted_command_after_typed_text_is_ordinary_input() {
        let mut command = ChatCommand::default();
        typed(&mut command, "see ");
        command.input(b"\x1b[200~/chat\x1b[201~");
        assert!(!command.input(b"\r"));
    }
}
