//! Compact activity presentation for the native transcript.
//! See docs/codex-chat-reference.md for the public-source and visual references.
use super::model::{Block, FileChangeBlock, ToolCall, ToolState};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Search,
    Command,
    Edit,
    Tool,
}

impl Kind {
    fn verb(self, running: bool) -> &'static str {
        match (self, running) {
            (Self::Read, true) => "Reading",
            (Self::Read, false) => "Read",
            (Self::Search, true) => "Searching",
            (Self::Search, false) => "Searched",
            (Self::Command, true) => "Running",
            (Self::Command, false) => "Ran",
            (Self::Edit, true) => "Editing",
            (Self::Edit, false) => "Edited",
            (Self::Tool, true) => "Calling",
            (Self::Tool, false) => "Called",
        }
    }
}

pub fn describe(tool: &ToolCall) -> (Kind, String) {
    let name = tool.name.to_ascii_lowercase();
    let name = name.rsplit(['.', ':']).next().unwrap_or(&name);
    let mut detail = tool
        .input
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Providers sometimes include the shell launcher in their display input.
    for prefix in ["/bin/zsh -lc ", "/bin/bash -lc ", "bash -lc ", "zsh -lc "] {
        if let Some(command) = detail.strip_prefix(prefix) {
            detail = command.trim_matches(['\'', '"']).to_owned();
            break;
        }
    }
    let kind = match name {
        "read" | "read_file" | "readfile" => Kind::Read,
        "grep" | "glob" | "search" | "web_search" | "file_search" => Kind::Search,
        "edit" | "write" | "write_file" | "apply_patch" | "multiedit" => Kind::Edit,
        "shell" | "bash" | "exec_command" | "execute" | "run_command" | "write_stdin"
        | "terminal_command" => {
            // Classify only simple commands. Compound scripts stay visible as commands.
            if !tool.input.contains([';', '\n', '|', '&']) {
                let command = detail.split_whitespace().next().unwrap_or("");
                if matches!(command, "cat" | "head" | "tail") || detail.starts_with("sed -n ") {
                    if let Some(path) = detail
                        .split_whitespace()
                        .last()
                        .filter(|part| !part.starts_with('-'))
                    {
                        detail = path.trim_matches(['\'', '"']).to_owned();
                    }
                    Kind::Read
                } else if matches!(command, "rg" | "grep" | "find" | "ls") {
                    Kind::Search
                } else {
                    Kind::Command
                }
            } else {
                Kind::Command
            }
        }
        _ => Kind::Tool,
    };
    if kind == Kind::Edit {
        for prefix in ["add ", "update ", "delete "] {
            if let Some(path) = detail.strip_prefix(prefix) {
                detail = path.to_owned();
                break;
            }
        }
    }
    if matches!(kind, Kind::Read | Kind::Edit) && !detail.contains(' ') {
        detail = std::path::Path::new(&detail)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or(detail);
    }
    if kind == Kind::Tool {
        detail = if detail.is_empty() {
            tool.name.clone()
        } else {
            format!("{} {detail}", tool.name)
        };
    }
    let verb = match tool.state {
        ToolState::Error => "Failed",
        ToolState::Declined => "Declined",
        _ => kind.verb(tool.state == ToolState::Running),
    };
    (kind, format!("{verb} {detail}").trim_end().to_owned())
}

pub fn matches_file(tool: &ToolCall, file: &FileChangeBlock) -> bool {
    tool.state == ToolState::Ok
        && describe(tool).0 == Kind::Edit
        && tool.input.contains(file.path.to_string_lossy().as_ref())
}

/// Group consecutive calls immediately; messages and approvals remain boundaries.
/// Live and failed states are surfaced in the group header. One pass over the
/// transcript; look rows up with [`group_at`] instead of rescanning per row.
pub fn tool_groups(blocks: &[Block]) -> Vec<Range<usize>> {
    let mut groups = Vec::new();
    let mut start = None;
    for ix in 0..=blocks.len() {
        match (start, matches!(blocks.get(ix), Some(Block::Tool(_)))) {
            (None, true) => start = Some(ix),
            (Some(first), false) => {
                if ix - first >= 2 {
                    groups.push(first..ix);
                }
                start = None;
            }
            _ => {}
        }
    }
    groups
}

/// The group from [`tool_groups`] containing row `ix`, if any.
pub fn group_at(groups: &[Range<usize>], ix: usize) -> Option<Range<usize>> {
    let at = groups.partition_point(|group| group.end <= ix);
    groups.get(at).filter(|group| group.contains(&ix)).cloned()
}

/// A group's summary: the running action, or what the finished calls did,
/// with real counts ("Read 1 file, ran 3 commands").
pub fn group_label(blocks: &[Block]) -> String {
    let tools = || {
        blocks.iter().filter_map(|block| match block {
            Block::Tool(tool) => Some(tool),
            _ => None,
        })
    };
    if let Some(tool) = tools().rfind(|tool| tool.state == ToolState::Running) {
        return format!(
            "{} · {}",
            describe(tool).1,
            crate::ui::plural(blocks.len(), "tool", "tools")
        );
    }
    // Reads and edits count distinct files; other kinds count calls.
    let mut kinds: Vec<(Kind, Vec<String>)> = Vec::new();
    for tool in tools() {
        let (kind, label) = describe(tool);
        let at = match kinds.iter().position(|(k, _)| *k == kind) {
            Some(at) => at,
            None => {
                kinds.push((kind, Vec::new()));
                kinds.len() - 1
            }
        };
        let key = match kind {
            Kind::Read | Kind::Edit => label
                .split_once(' ')
                .map_or(label.clone(), |(_, file)| file.to_owned()),
            _ => kinds[at].1.len().to_string(),
        };
        if !kinds[at].1.contains(&key) {
            kinds[at].1.push(key);
        }
    }
    let labels: Vec<_> = kinds
        .iter()
        .map(|(kind, items)| {
            let n = items.len();
            match kind {
                Kind::Read => format!("read {}", crate::ui::plural(n, "file", "files")),
                Kind::Search => "searched".to_owned(),
                Kind::Command => format!("ran {}", crate::ui::plural(n, "command", "commands")),
                Kind::Edit => format!("edited {}", crate::ui::plural(n, "file", "files")),
                Kind::Tool => format!("called {}", crate::ui::plural(n, "tool", "tools")),
            }
        })
        .collect();
    let text = labels.join(", ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Failed and declined calls in a group, if any: "1 failed", "2 declined".
pub fn failure_note(blocks: &[Block]) -> Option<String> {
    let count = |state: ToolState| {
        blocks
            .iter()
            .filter(|block| matches!(block, Block::Tool(tool) if tool.state == state))
            .count()
    };
    let parts: Vec<String> = [
        (count(ToolState::Error), "failed"),
        (count(ToolState::Declined), "declined"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, label)| format!("{n} {label}"))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tool(name: &str, input: &str, state: ToolState) -> ToolCall {
        ToolCall {
            id: "test".into(),
            name: name.into(),
            input: input.into(),
            output: String::new(),
            state,
            expanded: false,
        }
    }
    #[test]
    fn summaries_distinguish_reads_searches_and_running_commands() {
        assert_eq!(
            describe(&tool("Read", "src/main.rs", ToolState::Ok)).1,
            "Read main.rs"
        );
        assert_eq!(
            describe(&tool("shell", "sed -n '1,30p' main.rs", ToolState::Ok)).1,
            "Read main.rs"
        );
        assert_eq!(
            describe(&tool("shell", "cargo test", ToolState::Running)).1,
            "Running cargo test"
        );
        assert_eq!(
            describe(&tool("shell", "cat a; rm b", ToolState::Ok)).0,
            Kind::Command
        );
        assert_eq!(
            describe(&tool(
                "shell",
                "/bin/zsh -lc 'ls -l index.html'",
                ToolState::Error
            ))
            .1,
            "Failed ls -l index.html"
        );
        assert_eq!(
            describe(&tool(
                "apply_patch",
                "add /Users/nihal/index.html",
                ToolState::Ok
            ))
            .1,
            "Edited index.html"
        );
        assert_eq!(
            describe(&tool("Grep", "TODO", ToolState::Ok)).0,
            Kind::Search
        );
    }
    #[test]
    fn multiline_command_summary_uses_only_first_line() {
        let call = tool("shell", "echo first\necho second", ToolState::Running);
        assert_eq!(describe(&call).1, "Running echo first");
        assert_eq!(call.input, "echo first\necho second");
        assert_eq!(
            group_label(&[Block::Tool(call)]),
            "Running echo first · 1 tool"
        );
    }

    fn completed_group(blocks: &[Block], ix: usize, _running: bool) -> Option<Range<usize>> {
        group_at(&tool_groups(blocks), ix)
    }

    #[test]
    fn groups_are_found_for_every_member_in_one_pass() {
        let call = || Block::Tool(tool("shell", "pwd", ToolState::Ok));
        let user = Block::User {
            text: "hi".into(),
            steer: false,
            goal: false,
            attachments: Default::default(),
        };
        let blocks = vec![
            call(),
            call(),
            user.clone(),
            call(),
            user,
            call(),
            call(),
            call(),
        ];
        assert_eq!(tool_groups(&blocks), [0..2, 5..8]);
        assert_eq!(group_at(&tool_groups(&blocks), 3), None);
        assert_eq!(group_at(&tool_groups(&blocks), 7), Some(5..8));
        assert_eq!(group_at(&tool_groups(&blocks), 8), None);
    }

    #[test]
    fn consecutive_calls_group_while_running() {
        let blocks = vec![
            Block::Tool(tool("shell", "pwd", ToolState::Ok)),
            Block::Tool(tool("shell", "date", ToolState::Running)),
        ];
        assert_eq!(completed_group(&blocks, 0, true), Some(0..2));
        assert_eq!(completed_group(&blocks, 1, true), Some(0..2));
    }
    #[test]
    fn groups_surface_failures_and_do_not_cross_messages() {
        let mut blocks = vec![
            Block::Tool(tool("Read", "a", ToolState::Ok)),
            Block::Tool(tool("shell", "cargo test", ToolState::Ok)),
        ];
        assert_eq!(completed_group(&blocks, 1, false), Some(0..2));
        assert_eq!(group_label(&blocks), "Read 1 file, ran 1 command");
        assert_eq!(completed_group(&blocks, 1, true), Some(0..2));
        blocks.push(Block::Assistant {
            turn_id: "t".into(),
            text: "Done".into(),
            open: true,
        });
        assert_eq!(completed_group(&blocks, 0, true), Some(0..2));
        if let Block::Tool(tool) = &mut blocks[0] {
            tool.state = ToolState::Error;
        }
        assert_eq!(completed_group(&blocks, 1, false), Some(0..2));
        assert_eq!(failure_note(&blocks[..2]).as_deref(), Some("1 failed"));
        if let Block::Tool(tool) = &mut blocks[1] {
            tool.state = ToolState::Declined;
        }
        assert_eq!(
            failure_note(&blocks[..2]).as_deref(),
            Some("1 failed, 1 declined")
        );
        assert_eq!(completed_group(&blocks, 2, false), None);
    }
}
