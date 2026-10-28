//! Small, bounded title requests shared by provider adapters.
use crate::AgentEvent;

// OpenCode PROMPT_TITLE (MIT), pinned to 7b5bd8fdde59c1de865b04e41899342f7a9c6a49.
// https://github.com/anomalyco/opencode/blob/7b5bd8fdde59c1de865b04e41899342f7a9c6a49/packages/core/src/plugin/agent.ts
// License: title_prompt.LICENSE
pub const INSTRUCTIONS: &str = include_str!("title_prompt.txt");

pub fn context(events: &[AgentEvent]) -> (u32, String) {
    let count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::UserMessage { .. }))
        .count() as u32;
    let mut rows: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::UserMessage { text } => Some(format!(
                "User: {}",
                text.chars().take(1200).collect::<String>()
            )),
            AgentEvent::AssistantMessage { text, .. } => Some(format!(
                "Assistant: {}",
                text.chars().take(1200).collect::<String>()
            )),
            _ => None,
        })
        .collect();
    if rows.len() > 6 {
        rows.drain(1..rows.len() - 5);
    }
    (count, rows.join("\n\n"))
}

/// Longest title kept, in characters.
pub const MAX_CHARS: usize = 80;

/// Directional formatting characters that can visually reorder a title.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Remove control and bidi-override characters; tabs become spaces.
fn strip_controls(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() || is_bidi_control(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Validate a generated title: one line, at most [`MAX_CHARS`], no control characters.
pub fn clean(text: &str) -> Option<String> {
    if text.trim().contains(['\n', '\r']) {
        return None;
    }
    let stripped = strip_controls(text);
    let title = stripped.trim().trim_matches(['"', '\'', '`']).trim();
    if title.is_empty() || title.chars().count() > MAX_CHARS {
        return None;
    }
    Some(title.to_owned())
}

/// Sanitize a provider-owned title: first line only, truncated rather than rejected.
pub fn clean_provider(text: &str) -> Option<String> {
    let line = text.trim().lines().next().unwrap_or_default();
    let stripped = strip_controls(line);
    let truncated: String = stripped.trim().chars().take(MAX_CHARS).collect();
    clean(&truncated)
}

pub fn due(previous: u32, current: u32) -> bool {
    current > 0 && (previous == 0 || current >= previous.saturating_add(6))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_context_is_bounded_and_omits_tools_and_reasoning() {
        let mut events = vec![AgentEvent::UserMessage {
            text: "Original topic".into(),
        }];
        for i in 0..30 {
            events.push(AgentEvent::UserMessage {
                text: format!("{i} {}", "x".repeat(3000)),
            });
        }
        let (count, text) = context(&events);
        assert_eq!(count, 31);
        assert!(text.starts_with("User: Original topic"));
        assert!(text.contains("User: 29"));
        assert!(text.chars().count() < 7300);
        assert!(due(0, 1));
        assert!(!due(1, 6));
        assert!(due(1, 7));
        assert!(!due(0, 0));
        assert_eq!(
            clean("\"Build a chess game\""),
            Some("Build a chess game".into())
        );
        assert!(clean("Title\nExplanation").is_none());
    }

    #[test]
    fn titles_lose_control_and_bidi_characters() {
        assert_eq!(
            clean("\u{1b}[31mRed\u{202E}txt\tTab\u{2066}").as_deref(),
            Some("[31mRedtxt Tab")
        );
        assert!(clean("\u{202E}\u{200F}").is_none());
        assert_eq!(
            clean_provider(&"y".repeat(200)).map(|t| t.chars().count()),
            Some(MAX_CHARS)
        );
        assert_eq!(
            clean_provider("Renamed\u{202E} in Codex\nsecond").as_deref(),
            Some("Renamed in Codex")
        );
        assert!(clean_provider("  ").is_none());
    }
}
