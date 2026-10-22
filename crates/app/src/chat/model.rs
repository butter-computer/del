//! The chat's UI model and the reducer that folds `AgentEvent`s into it.
//!
//! Plain data, no GPUI: the view renders `ChatModel::blocks` one row per block
//! and calls `apply` for every event from the session stream. Each event
//! touches at most one block, which `apply` reports as a `Change` so the view
//! can keep its virtual list in step without diffing.

use del_agent::{
    AgentEvent, ApprovalKind, ApprovalResponse, FileChangeKind, ToolStatus, TurnStatus,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Error,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: String,
    pub output: String,
    pub state: ToolState,
    pub expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Removed,
    Context,
    Hunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeBlock {
    pub path: PathBuf,
    pub kind: FileChangeKind,
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
    pub expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    Pending,
    Approved,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalBlock {
    pub id: String,
    pub kind: ApprovalKind,
    pub detail: String,
    pub state: ApprovalState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    User {
        text: String,
        steer: bool,
    },
    /// `open` while its deltas are still arriving.
    Assistant {
        turn_id: String,
        text: String,
        open: bool,
    },
    Reasoning {
        text: String,
        open: bool,
        expanded: bool,
    },
    Tool(ToolCall),
    FileChange(FileChangeBlock),
    Approval(ApprovalBlock),
    Error {
        message: String,
        recoverable: bool,
    },
    /// A turn that did not complete normally.
    TurnEnd {
        status: TurnStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

impl Usage {
    fn add(self, other: Usage) -> Usage {
        Usage {
            input: self.input + other.input,
            output: self.output + other.output,
            cached: self.cached + other.cached,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Running,
}

/// Which row an event changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Pushed(usize),
    Updated(usize),
}

#[derive(Debug, Clone, Default)]
pub struct ChatModel {
    pub blocks: Vec<Block>,
    pub phase: Phase,
    /// Status of the most recently finished turn.
    pub last_status: Option<TurnStatus>,
    /// Usage of finished turns.
    settled_usage: Usage,
    /// Latest usage report of the turn in flight. Backends report a running
    /// total for the turn, so the newest report replaces the previous one.
    turn_usage: Usage,
    current_turn: Option<String>,
}

impl ChatModel {
    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    pub fn usage(&self) -> Usage {
        self.settled_usage.add(self.turn_usage)
    }

    pub fn has_pending_approval(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b, Block::Approval(a) if a.state == ApprovalState::Pending))
    }

    fn push(&mut self, block: Block) -> Change {
        self.blocks.push(block);
        Change::Pushed(self.blocks.len() - 1)
    }

    /// Mark the user's own message and optimistically enter the running phase.
    pub fn push_user(&mut self, text: String, steer: bool) -> Change {
        self.seal();
        if !steer {
            self.phase = Phase::Running;
        }
        self.push(Block::User { text, steer })
    }

    pub fn push_error(&mut self, message: String, recoverable: bool) -> Change {
        self.seal();
        if !recoverable {
            self.phase = Phase::Idle;
        }
        self.push(Block::Error {
            message,
            recoverable,
        })
    }

    /// Stop streaming the open assistant/reasoning block, if any.
    fn seal(&mut self) {
        match self.blocks.last_mut() {
            Some(Block::Assistant { open, .. }) | Some(Block::Reasoning { open, .. }) => {
                *open = false
            }
            _ => {}
        }
    }

    /// Record the user's answer to an approval card. Returns the row to redraw.
    pub fn resolve_approval(&mut self, id: &str, response: ApprovalResponse) -> Option<Change> {
        let ix = self.blocks.iter().position(
            |b| matches!(b, Block::Approval(a) if a.id == id && a.state == ApprovalState::Pending),
        )?;
        if let Block::Approval(a) = &mut self.blocks[ix] {
            a.state = match response {
                ApprovalResponse::Approve => ApprovalState::Approved,
                ApprovalResponse::Deny => ApprovalState::Denied,
            };
        }
        Some(Change::Updated(ix))
    }

    /// Flip a collapsible block (reasoning, tool output, diff).
    pub fn toggle(&mut self, ix: usize) -> Option<Change> {
        match self.blocks.get_mut(ix)? {
            Block::Reasoning { expanded, .. } => *expanded = !*expanded,
            Block::Tool(t) => t.expanded = !t.expanded,
            Block::FileChange(f) => f.expanded = !f.expanded,
            _ => return None,
        }
        Some(Change::Updated(ix))
    }

    fn tool_mut(&mut self, id: &str) -> Option<(usize, &mut ToolCall)> {
        self.blocks
            .iter_mut()
            .enumerate()
            .rev()
            .find_map(|(ix, b)| match b {
                Block::Tool(t) if t.id == id => Some((ix, t)),
                _ => None,
            })
    }

    pub fn apply(&mut self, event: &AgentEvent) -> Option<Change> {
        match event {
            AgentEvent::SessionStarted { .. } => None,
            AgentEvent::TurnStarted { turn_id } => {
                self.phase = Phase::Running;
                self.current_turn = Some(turn_id.clone());
                self.turn_usage = Usage::default();
                None
            }
            AgentEvent::AssistantTextDelta { turn_id, text } => {
                if let Some(Block::Assistant {
                    turn_id: t,
                    text: buf,
                    open: true,
                }) = self.blocks.last_mut()
                    && t == turn_id
                {
                    buf.push_str(text);
                    return Some(Change::Updated(self.blocks.len() - 1));
                }
                self.seal();
                Some(self.push(Block::Assistant {
                    turn_id: turn_id.clone(),
                    text: text.clone(),
                    open: true,
                }))
            }
            AgentEvent::AssistantMessage { turn_id, text } => {
                if let Some(Block::Assistant {
                    turn_id: t,
                    text: buf,
                    open,
                }) = self.blocks.last_mut()
                    && t == turn_id
                    && *open
                {
                    *buf = text.clone();
                    *open = false;
                    return Some(Change::Updated(self.blocks.len() - 1));
                }
                self.seal();
                Some(self.push(Block::Assistant {
                    turn_id: turn_id.clone(),
                    text: text.clone(),
                    open: false,
                }))
            }
            AgentEvent::ReasoningDelta { text, .. } => {
                if let Some(Block::Reasoning {
                    text: buf,
                    open: true,
                    ..
                }) = self.blocks.last_mut()
                {
                    buf.push_str(text);
                    return Some(Change::Updated(self.blocks.len() - 1));
                }
                self.seal();
                Some(self.push(Block::Reasoning {
                    text: text.clone(),
                    open: true,
                    expanded: false,
                }))
            }
            AgentEvent::ToolCallStarted {
                id,
                name,
                input_summary,
            } => {
                self.seal();
                Some(self.push(Block::Tool(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input_summary.clone(),
                    output: String::new(),
                    state: ToolState::Running,
                    expanded: false,
                })))
            }
            AgentEvent::ToolCallUpdated { id, output_summary } => {
                let (ix, tool) = self.tool_mut(id)?;
                tool.output = output_summary.clone();
                Some(Change::Updated(ix))
            }
            AgentEvent::ToolCallFinished {
                id,
                status,
                output_summary,
            } => {
                let state = match status {
                    ToolStatus::Completed => ToolState::Ok,
                    ToolStatus::Failed => ToolState::Error,
                    ToolStatus::Declined => ToolState::Declined,
                };
                if let Some((ix, tool)) = self.tool_mut(id) {
                    tool.state = state;
                    if !output_summary.is_empty() {
                        tool.output = output_summary.clone();
                    }
                    return Some(Change::Updated(ix));
                }
                // Finished without a start event: show it anyway.
                self.seal();
                Some(self.push(Block::Tool(ToolCall {
                    id: id.clone(),
                    name: "tool".into(),
                    input: String::new(),
                    output: output_summary.clone(),
                    state,
                    expanded: false,
                })))
            }
            AgentEvent::FileChange { path, kind, diff } => {
                self.seal();
                let lines = diff.as_deref().map(parse_diff).unwrap_or_default();
                let added = lines.iter().filter(|l| l.kind == DiffKind::Added).count();
                let removed = lines.iter().filter(|l| l.kind == DiffKind::Removed).count();
                Some(self.push(Block::FileChange(FileChangeBlock {
                    path: path.clone(),
                    kind: *kind,
                    added,
                    removed,
                    expanded: !lines.is_empty() && lines.len() <= AUTO_EXPAND_DIFF_LINES,
                    lines,
                })))
            }
            AgentEvent::ApprovalRequested { id, kind, detail } => {
                self.seal();
                Some(self.push(Block::Approval(ApprovalBlock {
                    id: id.clone(),
                    kind: *kind,
                    detail: detail.clone(),
                    state: ApprovalState::Pending,
                })))
            }
            AgentEvent::TokenUsage {
                input,
                output,
                cached,
            } => {
                self.turn_usage = Usage {
                    input: *input,
                    output: *output,
                    cached: *cached,
                };
                None
            }
            AgentEvent::TurnCompleted { status, .. } => {
                self.seal();
                self.phase = Phase::Idle;
                self.current_turn = None;
                self.last_status = Some(*status);
                self.settled_usage = self.settled_usage.add(self.turn_usage);
                self.turn_usage = Usage::default();
                (*status != TurnStatus::Completed)
                    .then(|| self.push(Block::TurnEnd { status: *status }))
            }
            AgentEvent::Error {
                message,
                recoverable,
            } => Some(self.push_error(message.clone(), *recoverable)),
        }
    }
}

/// Diffs up to this many lines start expanded.
const AUTO_EXPAND_DIFF_LINES: usize = 12;

/// Parse a unified diff into display lines, dropping the file headers.
pub fn parse_diff(diff: &str) -> Vec<DiffLine> {
    diff.lines()
        .filter(|l| !l.starts_with("+++") && !l.starts_with("---") && !l.starts_with("diff "))
        .filter(|l| !l.starts_with("index "))
        .map(|l| {
            let (kind, text) = if let Some(rest) = l.strip_prefix('+') {
                (DiffKind::Added, rest)
            } else if let Some(rest) = l.strip_prefix('-') {
                (DiffKind::Removed, rest)
            } else if l.starts_with("@@") {
                (DiffKind::Hunk, l)
            } else {
                (DiffKind::Context, l.strip_prefix(' ').unwrap_or(l))
            };
            DiffLine {
                kind,
                text: text.to_string(),
            }
        })
        .collect()
}

/// "1.2k"-style count for the usage line.
pub fn compact_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> AgentEvent {
        AgentEvent::AssistantTextDelta {
            turn_id: "t1".into(),
            text: text.into(),
        }
    }

    fn tool_start(id: &str) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            id: id.into(),
            name: "bash".into(),
            input_summary: "ls".into(),
        }
    }

    #[test]
    fn deltas_stream_into_one_block() {
        let mut m = ChatModel::default();
        assert_eq!(m.apply(&delta("Hel")), Some(Change::Pushed(0)));
        assert_eq!(m.apply(&delta("lo")), Some(Change::Updated(0)));
        assert_eq!(
            m.blocks,
            vec![Block::Assistant {
                turn_id: "t1".into(),
                text: "Hello".into(),
                open: true
            }]
        );
    }

    #[test]
    fn final_message_replaces_streamed_text_and_closes() {
        let mut m = ChatModel::default();
        m.apply(&delta("Hel"));
        let ev = AgentEvent::AssistantMessage {
            turn_id: "t1".into(),
            text: "Hello!".into(),
        };
        assert_eq!(m.apply(&ev), Some(Change::Updated(0)));
        assert_eq!(
            m.blocks[0],
            Block::Assistant {
                turn_id: "t1".into(),
                text: "Hello!".into(),
                open: false
            }
        );
        // A later delta starts a new message instead of reopening the old one.
        assert_eq!(m.apply(&delta("again")), Some(Change::Pushed(1)));
    }

    #[test]
    fn message_without_deltas_is_pushed_closed() {
        let mut m = ChatModel::default();
        let ev = AgentEvent::AssistantMessage {
            turn_id: "t1".into(),
            text: "hi".into(),
        };
        assert_eq!(m.apply(&ev), Some(Change::Pushed(0)));
    }

    #[test]
    fn tool_event_seals_streaming_text() {
        let mut m = ChatModel::default();
        m.apply(&delta("let me look"));
        m.apply(&tool_start("c1"));
        assert!(matches!(m.blocks[0], Block::Assistant { open: false, .. }));
        assert_eq!(m.apply(&delta("done")), Some(Change::Pushed(2)));
    }

    #[test]
    fn reasoning_accumulates_and_closes() {
        let mut m = ChatModel::default();
        let r = |t: &str| AgentEvent::ReasoningDelta {
            turn_id: "t1".into(),
            text: t.into(),
        };
        m.apply(&r("a"));
        m.apply(&r("b"));
        m.apply(&delta("x"));
        assert_eq!(
            m.blocks[0],
            Block::Reasoning {
                text: "ab".into(),
                open: false,
                expanded: false
            }
        );
        assert_eq!(m.blocks.len(), 2);
    }

    #[test]
    fn tool_lifecycle() {
        let mut m = ChatModel::default();
        m.apply(&tool_start("c1"));
        m.apply(&AgentEvent::ToolCallUpdated {
            id: "c1".into(),
            output_summary: "partial".into(),
        });
        let done = AgentEvent::ToolCallFinished {
            id: "c1".into(),
            status: ToolStatus::Failed,
            output_summary: "boom".into(),
        };
        assert_eq!(m.apply(&done), Some(Change::Updated(0)));
        let Block::Tool(t) = &m.blocks[0] else {
            panic!()
        };
        assert_eq!((t.state, t.output.as_str()), (ToolState::Error, "boom"));
    }

    #[test]
    fn finish_keeps_streamed_output_when_summary_is_empty() {
        let mut m = ChatModel::default();
        m.apply(&tool_start("c1"));
        m.apply(&AgentEvent::ToolCallUpdated {
            id: "c1".into(),
            output_summary: "out".into(),
        });
        m.apply(&AgentEvent::ToolCallFinished {
            id: "c1".into(),
            status: ToolStatus::Completed,
            output_summary: String::new(),
        });
        let Block::Tool(t) = &m.blocks[0] else {
            panic!()
        };
        assert_eq!((t.state, t.output.as_str()), (ToolState::Ok, "out"));
    }

    #[test]
    fn update_for_unknown_tool_is_ignored_but_finish_is_shown() {
        let mut m = ChatModel::default();
        let up = AgentEvent::ToolCallUpdated {
            id: "x".into(),
            output_summary: "o".into(),
        };
        assert_eq!(m.apply(&up), None);
        let fin = AgentEvent::ToolCallFinished {
            id: "x".into(),
            status: ToolStatus::Declined,
            output_summary: "no".into(),
        };
        assert_eq!(m.apply(&fin), Some(Change::Pushed(0)));
    }

    #[test]
    fn file_change_counts_diff_lines() {
        let mut m = ChatModel::default();
        let diff = "--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n+extra";
        m.apply(&AgentEvent::FileChange {
            path: "f".into(),
            kind: FileChangeKind::Modified,
            diff: Some(diff.into()),
        });
        let Block::FileChange(f) = &m.blocks[0] else {
            panic!()
        };
        assert_eq!((f.added, f.removed), (2, 1));
        assert!(f.expanded);
        assert_eq!(f.lines[0].kind, DiffKind::Hunk);
        assert_eq!(
            f.lines[1],
            DiffLine {
                kind: DiffKind::Context,
                text: "keep".into()
            }
        );
    }

    #[test]
    fn file_change_without_diff_has_no_lines() {
        let mut m = ChatModel::default();
        m.apply(&AgentEvent::FileChange {
            path: "f".into(),
            kind: FileChangeKind::Deleted,
            diff: None,
        });
        let Block::FileChange(f) = &m.blocks[0] else {
            panic!()
        };
        assert!(f.lines.is_empty());
    }

    #[test]
    fn approval_resolves_once() {
        let mut m = ChatModel::default();
        m.apply(&AgentEvent::ApprovalRequested {
            id: "a1".into(),
            kind: ApprovalKind::Command,
            detail: "rm -rf x".into(),
        });
        assert!(m.has_pending_approval());
        assert_eq!(
            m.resolve_approval("a1", ApprovalResponse::Deny),
            Some(Change::Updated(0))
        );
        assert!(!m.has_pending_approval());
        assert_eq!(m.resolve_approval("a1", ApprovalResponse::Approve), None);
        assert_eq!(m.resolve_approval("nope", ApprovalResponse::Approve), None);
        let Block::Approval(a) = &m.blocks[0] else {
            panic!()
        };
        assert_eq!(a.state, ApprovalState::Denied);
    }

    #[test]
    fn turn_phase_and_status() {
        let mut m = ChatModel::default();
        m.apply(&AgentEvent::TurnStarted {
            turn_id: "t1".into(),
        });
        assert!(m.is_running());
        let done = AgentEvent::TurnCompleted {
            turn_id: "t1".into(),
            status: TurnStatus::Completed,
        };
        assert_eq!(m.apply(&done), None);
        assert!(!m.is_running());
        assert_eq!(m.last_status, Some(TurnStatus::Completed));
    }

    #[test]
    fn interrupted_turn_leaves_a_marker() {
        let mut m = ChatModel::default();
        m.apply(&delta("partial"));
        let done = AgentEvent::TurnCompleted {
            turn_id: "t1".into(),
            status: TurnStatus::Interrupted,
        };
        assert_eq!(m.apply(&done), Some(Change::Pushed(1)));
        assert!(matches!(m.blocks[0], Block::Assistant { open: false, .. }));
        assert_eq!(
            m.blocks[1],
            Block::TurnEnd {
                status: TurnStatus::Interrupted
            }
        );
    }

    #[test]
    fn usage_replaces_within_a_turn_and_sums_across_turns() {
        let mut m = ChatModel::default();
        let u = |i, o| AgentEvent::TokenUsage {
            input: i,
            output: o,
            cached: 0,
        };
        let end = |s| AgentEvent::TurnCompleted {
            turn_id: "t".into(),
            status: s,
        };
        m.apply(&AgentEvent::TurnStarted {
            turn_id: "t".into(),
        });
        m.apply(&u(10, 1));
        m.apply(&u(10, 5));
        assert_eq!(
            m.usage(),
            Usage {
                input: 10,
                output: 5,
                cached: 0
            }
        );
        m.apply(&end(TurnStatus::Completed));
        m.apply(&AgentEvent::TurnStarted {
            turn_id: "t2".into(),
        });
        m.apply(&u(20, 2));
        assert_eq!(
            m.usage(),
            Usage {
                input: 30,
                output: 7,
                cached: 0
            }
        );
    }

    #[test]
    fn user_message_starts_running_but_steer_does_not_change_phase() {
        let mut m = ChatModel::default();
        m.push_user("hi".into(), false);
        assert!(m.is_running());
        m.push_user("also this".into(), true);
        assert!(m.is_running());
        assert_eq!(
            m.blocks[1],
            Block::User {
                text: "also this".into(),
                steer: true
            }
        );
    }

    #[test]
    fn fatal_error_stops_running_recoverable_does_not() {
        let mut m = ChatModel::default();
        m.push_user("hi".into(), false);
        m.apply(&AgentEvent::Error {
            message: "rate".into(),
            recoverable: true,
        });
        assert!(m.is_running());
        m.apply(&AgentEvent::Error {
            message: "dead".into(),
            recoverable: false,
        });
        assert!(!m.is_running());
        assert_eq!(m.blocks.len(), 3);
    }

    #[test]
    fn toggle_flips_collapsibles_only() {
        let mut m = ChatModel::default();
        m.push_user("hi".into(), false);
        m.apply(&tool_start("c1"));
        assert_eq!(m.toggle(0), None);
        assert_eq!(m.toggle(1), Some(Change::Updated(1)));
        let Block::Tool(t) = &m.blocks[1] else {
            panic!()
        };
        assert!(t.expanded);
        assert_eq!(m.toggle(9), None);
    }

    #[test]
    fn compact_counts() {
        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1200), "1.2k");
        assert_eq!(compact_count(2_500_000), "2.5m");
    }
}
