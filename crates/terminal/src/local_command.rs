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
}
