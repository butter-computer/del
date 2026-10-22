//! ACP `session/update` → normalized [`AgentEvent`]s.
//!
//! Pure state machine, no I/O. It tracks the turn id, buffers the assistant
//! message text so a `AssistantMessage` with the full text follows its deltas,
//! and remembers tool calls so partial `tool_call_update`s can be completed.

use std::collections::HashMap;
use std::path::PathBuf;

use agent_client_protocol_schema::v1::{
    ContentBlock, Diff, Plan, PlanEntryStatus, SessionUpdate, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolKind,
};
use del_agent::{AgentEvent, ApprovalKind, FileChangeKind, ToolStatus, TurnStatus};

const SUMMARY_MAX: usize = 400;

#[derive(Default)]
struct ToolInfo {
    kind: ToolKind,
    /// Last output summary sent, to skip no-op updates.
    last_output: String,
    finished: bool,
}

#[derive(Default)]
pub struct Translator {
    turn: Option<String>,
    text: String,
    tools: HashMap<String, ToolInfo>,
    plan_seq: u32,
    plan_open: bool,
    plan_text: String,
}

fn truncate(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= SUMMARY_MAX {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(SUMMARY_MAX).collect();
    out.push('…');
    out
}

pub fn tool_name(kind: ToolKind, name: Option<&str>) -> String {
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        return n.to_owned();
    }
    match kind {
        ToolKind::Read => "Read",
        ToolKind::Edit => "Edit",
        ToolKind::Delete => "Delete",
        ToolKind::Move => "Move",
        ToolKind::Search => "Search",
        ToolKind::Execute => "Execute",
        ToolKind::Think => "Think",
        ToolKind::Fetch => "Fetch",
        ToolKind::SwitchMode => "Switch mode",
        _ => "Tool",
    }
    .to_owned()
}

pub fn approval_kind(kind: ToolKind) -> ApprovalKind {
    match kind {
        ToolKind::Execute => ApprovalKind::Command,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => ApprovalKind::FileChange,
        _ => ApprovalKind::Other,
    }
}

/// The most informative one-line description of a tool call's input.
pub fn input_summary(title: &str, raw_input: Option<&serde_json::Value>) -> String {
    if let Some(v) = raw_input {
        for key in [
            "command",
            "cmd",
            "file_path",
            "path",
            "pattern",
            "url",
            "query",
        ] {
            if let Some(s) = v.get(key).and_then(|c| {
                c.as_str().map(str::to_owned).or_else(|| {
                    c.as_array().map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                })
            }) && !s.is_empty()
            {
                return truncate(&s);
            }
        }
    }
    truncate(title)
}

fn content_text(c: &ToolCallContent) -> Option<String> {
    match c {
        ToolCallContent::Content(c) => match &c.content {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        },
        ToolCallContent::Terminal(_) => Some("(terminal output)".into()),
        _ => None,
    }
}

fn output_summary(content: &[ToolCallContent], raw_output: Option<&serde_json::Value>) -> String {
    let text: Vec<String> = content.iter().filter_map(content_text).collect();
    if !text.is_empty() {
        return truncate(&text.join("\n"));
    }
    match raw_output {
        Some(serde_json::Value::String(s)) => truncate(s),
        Some(v) if !v.is_null() => truncate(&v.to_string()),
        _ => String::new(),
    }
}

/// A compact unified diff of `old` → `new`: common head and tail are trimmed
/// to a few context lines, the middle is shown as removed/added.
pub fn unified_diff(old: Option<&str>, new: &str) -> String {
    const CTX: usize = 3;
    let old_lines: Vec<&str> = old.map(|o| o.lines().collect()).unwrap_or_default();
    let new_lines: Vec<&str> = new.lines().collect();
    let head = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let tail = old_lines[head..]
        .iter()
        .rev()
        .zip(new_lines[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (o_mid, n_mid) = (
        &old_lines[head..old_lines.len() - tail],
        &new_lines[head..new_lines.len() - tail],
    );
    let ctx_start = head.saturating_sub(CTX);
    let ctx_end = (old_lines.len() - tail + CTX).min(old_lines.len());
    let mut out = format!(
        "@@ -{},{} +{},{} @@\n",
        ctx_start + 1,
        ctx_end - ctx_start,
        ctx_start + 1,
        (head - ctx_start) + n_mid.len() + (ctx_end - (old_lines.len() - tail))
    );
    for l in &old_lines[ctx_start..head] {
        out.push_str(&format!(" {l}\n"));
    }
    for l in o_mid {
        out.push_str(&format!("-{l}\n"));
    }
    for l in n_mid {
        out.push_str(&format!("+{l}\n"));
    }
    for l in &old_lines[old_lines.len() - tail..ctx_end] {
        out.push_str(&format!(" {l}\n"));
    }
    out
}

fn file_change(d: &Diff, kind: ToolKind) -> AgentEvent {
    let change = if kind == ToolKind::Delete {
        FileChangeKind::Deleted
    } else if d.old_text.is_none() {
        FileChangeKind::Created
    } else {
        FileChangeKind::Modified
    };
    AgentEvent::FileChange {
        path: PathBuf::from(&d.path),
        kind: change,
        diff: Some(unified_diff(d.old_text.as_deref(), &d.new_text)),
    }
}

fn tool_status(s: ToolCallStatus) -> Option<ToolStatus> {
    match s {
        ToolCallStatus::Completed => Some(ToolStatus::Completed),
        ToolCallStatus::Failed => Some(ToolStatus::Failed),
        _ => None,
    }
}

impl Translator {
    pub fn turn_id(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    pub fn begin_turn(&mut self, turn_id: String) -> Vec<AgentEvent> {
        self.turn = Some(turn_id.clone());
        self.text.clear();
        self.plan_open = false;
        self.plan_text.clear();
        vec![AgentEvent::TurnStarted { turn_id }]
    }

    /// Close the open assistant message, if any.
    fn flush_text(&mut self, out: &mut Vec<AgentEvent>) {
        if self.text.is_empty() {
            return;
        }
        if let Some(turn_id) = self.turn.clone() {
            out.push(AgentEvent::AssistantMessage {
                turn_id,
                text: std::mem::take(&mut self.text),
            });
        }
        self.text.clear();
    }

    /// End the turn: flush text, close the plan, fail tool calls the agent
    /// never completed (so nothing spins forever), emit `TurnCompleted`.
    pub fn end_turn(&mut self, status: TurnStatus) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn.clone() else {
            return out;
        };
        self.flush_text(&mut out);
        if self.plan_open {
            out.push(AgentEvent::ToolCallFinished {
                id: self.plan_id(),
                status: ToolStatus::Completed,
                output_summary: self.plan_text.clone(),
            });
            self.plan_open = false;
        }
        let mut open: Vec<_> = self
            .tools
            .iter()
            .filter(|(_, t)| !t.finished)
            .map(|(id, _)| id.clone())
            .collect();
        open.sort();
        for id in open {
            out.push(AgentEvent::ToolCallFinished {
                id,
                status: ToolStatus::Failed,
                output_summary: "Agent ended the turn before reporting tool completion".into(),
            });
        }
        self.tools.clear();
        self.turn = None;
        out.push(AgentEvent::TurnCompleted { turn_id, status });
        out
    }

    fn plan_id(&self) -> String {
        format!("plan-{}", self.plan_seq)
    }

    fn plan(&mut self, p: &Plan, out: &mut Vec<AgentEvent>) {
        let text = p
            .entries
            .iter()
            .map(|e| {
                let mark = match e.status {
                    PlanEntryStatus::Completed => "[x]",
                    PlanEntryStatus::InProgress => "[~]",
                    _ => "[ ]",
                };
                format!("{mark} {}", e.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !self.plan_open {
            self.plan_seq += 1;
            self.plan_open = true;
            out.push(AgentEvent::ToolCallStarted {
                id: self.plan_id(),
                name: "Plan".into(),
                input_summary: format!("{} steps", p.entries.len()),
            });
        }
        if text != self.plan_text {
            out.push(AgentEvent::ToolCallUpdated {
                id: self.plan_id(),
                output_summary: text.clone(),
            });
            self.plan_text = text;
        }
    }

    fn tool_call(&mut self, tc: &ToolCall, out: &mut Vec<AgentEvent>) {
        self.flush_text(out);
        let id = tc.tool_call_id.to_string();
        out.push(AgentEvent::ToolCallStarted {
            id: id.clone(),
            name: tool_name(tc.kind, tc.name.as_deref()),
            input_summary: input_summary(&tc.title, tc.raw_input.as_ref()),
        });
        let mut info = ToolInfo {
            kind: tc.kind,
            ..Default::default()
        };
        self.apply_content(&id, &mut info, &tc.content, tc.raw_output.as_ref(), out);
        if let Some(status) = tool_status(tc.status) {
            info.finished = true;
            out.push(AgentEvent::ToolCallFinished {
                id: id.clone(),
                status,
                output_summary: info.last_output.clone(),
            });
        }
        self.tools.insert(id, info);
    }

    fn apply_content(
        &mut self,
        id: &str,
        info: &mut ToolInfo,
        content: &[ToolCallContent],
        raw_output: Option<&serde_json::Value>,
        out: &mut Vec<AgentEvent>,
    ) {
        for c in content {
            if let ToolCallContent::Diff(d) = c {
                out.push(file_change(d, info.kind));
            }
        }
        let summary = output_summary(content, raw_output);
        if !summary.is_empty() && summary != info.last_output {
            out.push(AgentEvent::ToolCallUpdated {
                id: id.to_owned(),
                output_summary: summary.clone(),
            });
            info.last_output = summary;
        }
    }

    fn tool_update(&mut self, u: &ToolCallUpdate, out: &mut Vec<AgentEvent>) {
        let id = u.tool_call_id.to_string();
        let mut info = self.tools.remove(&id).unwrap_or_else(|| {
            // Update for a call we never saw start: announce it first.
            out.push(AgentEvent::ToolCallStarted {
                id: id.clone(),
                name: tool_name(u.fields.kind.unwrap_or_default(), u.fields.name.as_deref()),
                input_summary: input_summary(
                    u.fields.title.as_deref().unwrap_or(""),
                    u.fields.raw_input.as_ref(),
                ),
            });
            ToolInfo {
                kind: u.fields.kind.unwrap_or_default(),
                ..Default::default()
            }
        });
        if let Some(k) = u.fields.kind {
            info.kind = k;
        }
        let content = u.fields.content.clone().unwrap_or_default();
        self.apply_content(&id, &mut info, &content, u.fields.raw_output.as_ref(), out);
        if let Some(status) = u.fields.status.and_then(tool_status)
            && !info.finished
        {
            info.finished = true;
            out.push(AgentEvent::ToolCallFinished {
                id: id.clone(),
                status,
                output_summary: info.last_output.clone(),
            });
        }
        self.tools.insert(id, info);
    }

    /// Translate one update. Updates outside a turn (late chunks, replay) yield nothing.
    pub fn update(&mut self, u: &SessionUpdate) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Some(turn_id) = self.turn.clone() else {
            return out;
        };
        match u {
            SessionUpdate::AgentMessageChunk(c) => {
                if let ContentBlock::Text(t) = &c.content {
                    self.text.push_str(&t.text);
                    out.push(AgentEvent::AssistantTextDelta {
                        turn_id,
                        text: t.text.clone(),
                    });
                }
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if let ContentBlock::Text(t) = &c.content {
                    self.flush_text(&mut out);
                    out.push(AgentEvent::ReasoningDelta {
                        turn_id,
                        text: t.text.clone(),
                    });
                }
            }
            SessionUpdate::ToolCall(tc) => self.tool_call(tc, &mut out),
            SessionUpdate::ToolCallUpdate(tu) => self.tool_update(tu, &mut out),
            SessionUpdate::Plan(p) => {
                self.flush_text(&mut out);
                self.plan(p, &mut out)
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upd(v: serde_json::Value) -> SessionUpdate {
        serde_json::from_value(v).expect("valid session update")
    }

    fn started() -> Translator {
        let mut t = Translator::default();
        t.begin_turn("t1".into());
        t
    }

    #[test]
    fn message_chunks_stream_then_close_with_full_text() {
        let mut t = started();
        let mut evs = vec![];
        for s in ["Hel", "lo"] {
            evs.extend(t.update(&upd(json!({"sessionUpdate":"agent_message_chunk",
                "content":{"type":"text","text":s}}))));
        }
        evs.extend(t.end_turn(TurnStatus::Completed));
        assert_eq!(
            evs,
            vec![
                AgentEvent::AssistantTextDelta {
                    turn_id: "t1".into(),
                    text: "Hel".into()
                },
                AgentEvent::AssistantTextDelta {
                    turn_id: "t1".into(),
                    text: "lo".into()
                },
                AgentEvent::AssistantMessage {
                    turn_id: "t1".into(),
                    text: "Hello".into()
                },
                AgentEvent::TurnCompleted {
                    turn_id: "t1".into(),
                    status: TurnStatus::Completed
                },
            ]
        );
    }

    #[test]
    fn thought_closes_the_message_and_maps_to_reasoning() {
        let mut t = started();
        t.update(&upd(
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"a"}}),
        ));
        let evs = t.update(&upd(
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"hmm"}}),
        ));
        assert!(matches!(evs[0], AgentEvent::AssistantMessage { .. }));
        assert!(matches!(&evs[1], AgentEvent::ReasoningDelta { text, .. } if text == "hmm"));
    }

    #[test]
    fn tool_call_lifecycle_with_partial_updates() {
        let mut t = started();
        let evs = t.update(&upd(json!({"sessionUpdate":"tool_call","toolCallId":"c1",
            "title":"Run ls","kind":"execute","status":"pending","rawInput":{"command":"ls -la"}})));
        assert_eq!(
            evs,
            vec![AgentEvent::ToolCallStarted {
                id: "c1".into(),
                name: "Execute".into(),
                input_summary: "ls -la".into()
            }]
        );
        let evs = t.update(&upd(json!({"sessionUpdate":"tool_call_update","toolCallId":"c1",
            "status":"in_progress","content":[{"type":"content","content":{"type":"text","text":"a.txt"}}]})));
        assert_eq!(
            evs,
            vec![AgentEvent::ToolCallUpdated {
                id: "c1".into(),
                output_summary: "a.txt".into()
            }]
        );
        let evs = t.update(&upd(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"c1","status":"completed"}),
        ));
        assert_eq!(
            evs,
            vec![AgentEvent::ToolCallFinished {
                id: "c1".into(),
                status: ToolStatus::Completed,
                output_summary: "a.txt".into()
            }]
        );
        // Nothing left open at the end of the turn.
        let end = t.end_turn(TurnStatus::Completed);
        assert_eq!(end.len(), 1);
    }

    #[test]
    fn diff_content_becomes_file_change() {
        let mut t = started();
        let evs = t.update(&upd(json!({"sessionUpdate":"tool_call","toolCallId":"e1","title":"Edit a.rs",
            "kind":"edit","status":"completed",
            "content":[{"type":"diff","path":"/w/a.rs","oldText":"one\ntwo\nthree\n","newText":"one\n2\nthree\n"}]})));
        let change = evs.iter().find_map(|e| match e {
            AgentEvent::FileChange { path, kind, diff } => {
                Some((path, *kind, diff.clone().unwrap()))
            }
            _ => None,
        });
        let (path, kind, diff) = change.expect("file change");
        assert_eq!(path, &PathBuf::from("/w/a.rs"));
        assert_eq!(kind, FileChangeKind::Modified);
        assert!(diff.contains("-two\n+2\n"), "{diff}");
        assert!(diff.starts_with("@@ "));
        assert!(matches!(
            evs.last(),
            Some(AgentEvent::ToolCallFinished {
                status: ToolStatus::Completed,
                ..
            })
        ));
    }

    #[test]
    fn new_file_is_created_and_plan_updates_one_row() {
        let mut t = started();
        let evs = t.update(&upd(
            json!({"sessionUpdate":"tool_call","toolCallId":"e1","title":"Write",
            "kind":"edit","content":[{"type":"diff","path":"/w/n.txt","newText":"hi\n"}]}),
        ));
        assert!(evs.iter().any(|e| matches!(
            e,
            AgentEvent::FileChange {
                kind: FileChangeKind::Created,
                ..
            }
        )));

        let plan = |s1: &str| {
            upd(json!({"sessionUpdate":"plan","entries":[
            {"content":"first","priority":"high","status":s1},
            {"content":"second","priority":"low","status":"pending"}]}))
        };
        let a = t.update(&plan("pending"));
        assert!(matches!(&a[0], AgentEvent::ToolCallStarted { name, .. } if name == "Plan"));
        let b = t.update(&plan("completed"));
        assert_eq!(b.len(), 1);
        assert!(
            matches!(&b[0], AgentEvent::ToolCallUpdated { output_summary, .. } if output_summary.starts_with("[x] first"))
        );
        let end = t.end_turn(TurnStatus::Completed);
        assert!(
            end.iter()
                .any(|e| matches!(e, AgentEvent::ToolCallFinished { id, .. } if id == "plan-1"))
        );
    }

    #[test]
    fn interrupted_turn_fails_open_tools_and_drops_late_updates() {
        let mut t = started();
        t.update(&upd(
            json!({"sessionUpdate":"tool_call","toolCallId":"c1","title":"x","kind":"execute"}),
        ));
        let end = t.end_turn(TurnStatus::Interrupted);
        assert!(matches!(
            &end[0],
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Failed,
                ..
            }
        ));
        assert!(matches!(
            end.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            })
        ));
        // Late chunk after the turn closed.
        let late = t.update(&upd(
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}),
        ));
        assert!(late.is_empty());
    }

    #[test]
    fn update_without_start_announces_the_tool() {
        let mut t = started();
        let evs = t.update(&upd(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"z","title":"Fetch it",
            "kind":"fetch","status":"failed"}),
        ));
        assert!(matches!(evs[0], AgentEvent::ToolCallStarted { .. }));
        assert!(matches!(
            evs[1],
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Failed,
                ..
            }
        ));
    }

    #[test]
    fn unified_diff_trims_context() {
        let old = (1..=20).map(|i| format!("l{i}\n")).collect::<String>();
        let new = old.replace("l10\n", "ten\n");
        let d = unified_diff(Some(&old), &new);
        assert!(d.contains("-l10\n+ten\n"));
        assert!(!d.contains(" l1\n"));
        assert!(d.contains(" l9\n") && d.contains(" l11\n"));
    }
}
