//! Compact display metadata; attachment payloads still go to the provider intact.
use del_agent::UserInput;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachmentSummary {
    images: usize,
    files: usize,
}

impl AttachmentSummary {
    pub fn from_input(input: &UserInput) -> Self {
        let images = input
            .attachments
            .iter()
            .filter(|a| {
                a.mime
                    .as_deref()
                    .is_some_and(|mime| mime.starts_with("image/"))
            })
            .count();
        Self {
            images,
            files: input.attachments.len() - images,
        }
    }

    pub fn label(&self) -> Option<String> {
        let count = self.images + self.files;
        (count > 0).then(|| format!("{count} attachment{}", if count == 1 { "" } else { "s" }))
    }

    /// Adapters represent saved attachment blocks with these trailing markers.
    /// Only consume complete marker lines, never inline prose or filenames.
    pub fn from_history(text: &str) -> (String, Self) {
        let mut remaining = text;
        let mut summary = Self::default();
        loop {
            let trimmed = remaining.trim_end_matches(['\n', '\r']);
            let (prefix, line) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
            let line = line.trim_end_matches('\r');
            if line == "[Image]"
                || line == "[attached image]"
                || (line.starts_with("[attached image: ") && line.ends_with(']'))
            {
                summary.images += 1;
            } else if line == "[Document]"
                || ((line.starts_with("[attached file: ") || line.starts_with("[Attached file: "))
                    && line.ends_with(']'))
            {
                summary.files += 1;
            } else {
                break;
            }
            remaining = prefix;
        }
        (remaining.to_owned(), summary)
    }
}

#[cfg(test)]
mod tests {
    use super::AttachmentSummary;
    use del_agent::{Attachment, UserInput};

    #[test]
    fn summaries_count_types_without_exposing_names_or_changing_payloads() {
        let input = UserInput {
            text: "what are these?".into(),
            attachments: (0..4)
                .map(|i| Attachment {
                    path: format!("/private/screenshot-{i}.png").into(),
                    mime: Some("image/png".into()),
                })
                .collect(),
        };
        let original = input.clone();
        assert_eq!(
            AttachmentSummary::from_input(&input).label().as_deref(),
            Some("4 attachments")
        );
        assert_eq!(input, original);
        assert_eq!(
            AttachmentSummary {
                images: 1,
                files: 2
            }
            .label()
            .as_deref(),
            Some("3 attachments")
        );
        assert_eq!(
            AttachmentSummary {
                images: 0,
                files: 1
            }
            .label()
            .as_deref(),
            Some("1 attachment")
        );
        assert_eq!(AttachmentSummary::default().label(), None);
    }

    #[test]
    fn saved_codex_and_claude_markers_become_counts() {
        for text in [
            "look\n[attached image: /tmp/a.png]\n[attached image]",
            "look\n[Image]\n[Image]",
        ] {
            let (body, summary) = AttachmentSummary::from_history(text);
            assert_eq!(body, "look");
            assert_eq!(summary.label().as_deref(), Some("2 attachments"));
        }
        let (body, summary) =
            AttachmentSummary::from_history("[Image]\n[Attached file: /tmp/report.pdf]");
        assert!(body.is_empty());
        assert_eq!(summary.label().as_deref(), Some("2 attachments"));
    }

    #[test]
    fn ordinary_prose_and_bracketed_filenames_stay_intact() {
        for text in [
            "read [notes.pdf]",
            "prompt\n[photo.png]",
            "[Image]\nexplain this notation",
            "line one\nline two\n",
        ] {
            let (body, summary) = AttachmentSummary::from_history(text);
            assert_eq!(body, text);
            assert_eq!(summary.label(), None);
        }
    }
}
