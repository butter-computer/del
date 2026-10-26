//! Pure translation between the `claude` CLI's stream-json protocol and
//! [`AgentEvent`]. No I/O lives here so it can be tested against recorded
//! transcripts (`tests/fixtures/*.jsonl`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use del_agent::{
    AgentEvent, ApprovalKind, ApprovalResponse, FileChangeKind, ToolStatus, TurnStatus,
};
use serde_json::{Value, json};

const SUMMARY_LIMIT: usize = 500;

struct ToolInfo {
    name: String,
    input: Value,
}

/// Stateful line-by-line translator for one `claude` process.
#[derive(Default)]
pub struct Translator {
    /// Turn ids in flight; the front one is the turn the CLI is currently running.
    turns: VecDeque<String>,
    /// Stdin messages acknowledged as consumed since the previous result.
    /// Steering can be absorbed into the current response, not a separate turn.
    consumed_inputs: usize,
    tools: HashMap<String, ToolInfo>,
    /// `can_use_tool` request id -> original tool input (echoed back on approve).
    approvals: HashMap<String, Value>,
    interrupting: bool,
    session_id: Option<String>,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The CLI-reported session id, once `system/init` has been seen.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Register a user message sent to the CLI. The `TurnStarted` event is only
    /// returned when the turn becomes current; messages queued behind a running
    /// turn start when the previous result arrives.
    pub fn enqueue_turn(&mut self, turn_id: String) -> Vec<AgentEvent> {
        let starts_now = self.turns.is_empty();
        self.turns.push_back(turn_id.clone());
        if starts_now {
            vec![AgentEvent::TurnStarted { turn_id }]
        } else {
            vec![]
        }
    }

    /// Note that an interrupt was requested so the aborted result maps to `Interrupted`.
    pub fn mark_interrupting(&mut self) {
        self.interrupting = true;
    }

    /// Build the `control_response` line answering a pending `can_use_tool` request.
    pub fn approval_reply(&mut self, id: &str, response: ApprovalResponse) -> Option<String> {
        let input = self.approvals.remove(id)?;
        let body = match response {
            ApprovalResponse::Approve => json!({"behavior": "allow", "updatedInput": input}),
            ApprovalResponse::Deny => {
                json!({"behavior": "deny", "message": "Denied by the user in del"})
            }
        };
        Some(
            json!({"type": "control_response", "response": {
                "subtype": "success", "request_id": id, "response": body}})
            .to_string(),
        )
    }

    /// Whether a turn is currently running (or queued).
    pub fn is_busy(&self) -> bool {
        !self.turns.is_empty()
    }

    /// The process died: fail the running turn so consumers do not hang.
    pub fn abort(&mut self) -> Vec<AgentEvent> {
        let out = self
            .turns
            .pop_front()
            .map(|turn_id| AgentEvent::TurnCompleted {
                turn_id,
                status: TurnStatus::Failed,
            });
        self.turns.clear();
        self.consumed_inputs = 0;
        self.approvals.clear();
        out.into_iter().collect()
    }

    fn turn(&self) -> String {
        self.turns.front().cloned().unwrap_or_default()
    }

    /// Translate one stdout line. Unparseable or unrecognised lines yield nothing.
    pub fn handle_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        if let Some(event) = crate::goals::record(&v) {
            return vec![event];
        }
        // Acknowledgements are native local-command records, never inferred
        // from assistant prose (which can quote arbitrary goal instructions).
        if v["local_command_run"]["command"] == "goal" {
            let text = v["message"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default();
            if let Some(objective) = text.strip_prefix("Goal set: ") {
                return vec![AgentEvent::GoalUpdated {
                    goal: Some(del_agent::Goal {
                        objective: objective.into(),
                        status: del_agent::GoalStatus::Active,
                        time_used_seconds: 0,
                        tokens_used: 0,
                        can_pause: false,
                    }),
                }];
            }
            if text.starts_with("Goal cleared:") || text.starts_with("No goal set") {
                return vec![AgentEvent::GoalUpdated { goal: None }];
            }
        }
        // Native goals can start another turn without a user message. Give
        // those turns their own lifecycle so Stop, queues and streaming work.
        let starts_native_turn = self.turns.is_empty()
            && (v["type"] == "assistant"
                || (v["type"] == "stream_event" && v["event"]["type"] == "message_start"));
        let mut events = if starts_native_turn {
            self.enqueue_turn(format!("claude-{}", uuid::Uuid::new_v4()))
        } else {
            vec![]
        };
        events.extend(match v["type"].as_str() {
            Some("system") => {
                if v["subtype"] == "init" {
                    self.session_id = v["session_id"].as_str().map(str::to_owned);
                }
                vec![]
            }
            Some("stream_event") => self.stream_event(&v["event"]),
            Some("assistant") => self.assistant(&v["message"]),
            Some("user") => self.user(&v),
            Some("control_request") => self.control_request(&v),
            Some("result") => self.result(&v),
            _ => vec![],
        });
        events
    }

    fn stream_event(&self, ev: &Value) -> Vec<AgentEvent> {
        if ev["type"] != "content_block_delta" {
            return vec![];
        }
        let d = &ev["delta"];
        let turn_id = self.turn();
        match d["type"].as_str() {
            Some("text_delta") => {
                text(&d["text"]).map(|text| AgentEvent::AssistantTextDelta { turn_id, text })
            }
            // With thinking_display "updates" the text is blank; only forward real text.
            Some("thinking_delta") => {
                text(&d["thinking"]).map(|text| AgentEvent::ReasoningDelta { turn_id, text })
            }
            _ => None,
        }
        .into_iter()
        .collect()
    }

    fn assistant(&mut self, msg: &Value) -> Vec<AgentEvent> {
        let mut out = vec![];
        for block in msg["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = text(&block["text"]) {
                        out.push(AgentEvent::AssistantMessage {
                            turn_id: self.turn(),
                            text,
                        });
                    }
                }
                Some("tool_use") => {
                    let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                    else {
                        continue;
                    };
                    let input = block["input"].clone();
                    out.push(AgentEvent::ToolCallStarted {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        input_summary: summarize_input(name, &input),
                    });
                    self.tools.insert(
                        id.to_owned(),
                        ToolInfo {
                            name: name.to_owned(),
                            input,
                        },
                    );
                }
                _ => {}
            }
        }
        out
    }

    fn user(&mut self, v: &Value) -> Vec<AgentEvent> {
        if v["isReplay"] == true && v["parent_tool_use_id"].is_null() {
            self.consumed_inputs = (self.consumed_inputs + 1).min(self.turns.len());
        }
        let mut out = vec![];
        // Replayed plain-text user messages (`--replay-user-messages`) have string content.
        for block in v["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_result" {
                continue;
            }
            let Some(id) = block["tool_use_id"].as_str() else {
                continue;
            };
            let is_error = block["is_error"].as_bool().unwrap_or(false);
            let declined = v["tool_result_meta"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|m| m["id"] == id && m["permission_decision"]["decision"] == "reject");
            let status = match (declined, is_error) {
                (true, _) => ToolStatus::Declined,
                (false, true) => ToolStatus::Failed,
                (false, false) => ToolStatus::Completed,
            };
            out.push(AgentEvent::ToolCallFinished {
                id: id.to_owned(),
                status,
                output_summary: truncate(&result_text(&block["content"])),
            });
            if let (ToolStatus::Completed, Some(info)) = (status, self.tools.get(id)) {
                out.extend(file_change(info, &v["tool_use_result"]));
            }
        }
        out
    }

    fn control_request(&mut self, v: &Value) -> Vec<AgentEvent> {
        let r = &v["request"];
        let (Some(id), "can_use_tool") = (
            v["request_id"].as_str(),
            r["subtype"].as_str().unwrap_or(""),
        ) else {
            return vec![];
        };
        let name = r["tool_name"].as_str().unwrap_or("tool");
        let kind = match name {
            "Bash" | "PowerShell" => ApprovalKind::Command,
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => ApprovalKind::FileChange,
            _ => ApprovalKind::Other,
        };
        let detail = match kind {
            ApprovalKind::Other => format!("{name}: {}", summarize_input(name, &r["input"])),
            _ => summarize_input(name, &r["input"]),
        };
        self.approvals.insert(id.to_owned(), r["input"].clone());
        vec![AgentEvent::ApprovalRequested {
            id: id.to_owned(),
            kind,
            detail,
        }]
    }

    fn result(&mut self, v: &Value) -> Vec<AgentEvent> {
        let mut out = vec![];
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        out.push(AgentEvent::TokenUsage {
            input: n("input_tokens") + n("cache_creation_input_tokens"),
            output: n("output_tokens"),
            cached: n("cache_read_input_tokens"),
        });
        let aborted = v["terminal_reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("aborted"));
        let is_error = v["is_error"].as_bool().unwrap_or(false);
        let status = if self.interrupting || aborted {
            TurnStatus::Interrupted
        } else if is_error {
            TurnStatus::Failed
        } else {
            TurnStatus::Completed
        };
        if status == TurnStatus::Failed {
            let message = v["result"].as_str().map(str::to_owned).unwrap_or_else(|| {
                v["errors"]
                    .as_array()
                    .map(|e| {
                        e.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default()
            });
            out.push(AgentEvent::Error {
                message,
                recoverable: true,
            });
        }
        out.push(AgentEvent::TurnCompleted {
            turn_id: self.turn(),
            status,
        });
        // --replay-user-messages acknowledges inputs when the CLI consumes them.
        // One result may cover both the original prompt and steering messages.
        let consumed = self.consumed_inputs.max(1).min(self.turns.len());
        self.turns.drain(..consumed);
        self.consumed_inputs = 0;
        self.interrupting = false;
        self.tools.clear();
        if let Some(next) = self.turns.front() {
            out.push(AgentEvent::TurnStarted {
                turn_id: next.clone(),
            });
        }
        out
    }
}

/// `user` message line for stdin.
pub fn user_message_line(text: &str) -> String {
    json!({"type": "user", "message": {"role": "user", "content": text}}).to_string()
}

/// `interrupt` control request line for stdin.
pub fn interrupt_line(request_id: &str) -> String {
    json!({"type": "control_request", "request_id": request_id, "request": {"subtype": "interrupt"}}).to_string()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(SUMMARY_LIMIT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn summarize_input(name: &str, input: &Value) -> String {
    let field = match name {
        "Bash" | "PowerShell" => "command",
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "NotebookEdit" => "notebook_path",
        "Grep" | "Glob" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        _ => "",
    };
    match input[field].as_str() {
        Some(s) if !field.is_empty() => truncate(s),
        _ => truncate(&input.to_string()),
    }
}

/// Derive a `FileChange` from a successful Write/Edit/MultiEdit/NotebookEdit result.
fn file_change(info: &ToolInfo, result: &Value) -> Option<AgentEvent> {
    let (path_key, default_kind) = match info.name.as_str() {
        "Write" => ("file_path", FileChangeKind::Modified),
        "Edit" | "MultiEdit" => ("file_path", FileChangeKind::Modified),
        "NotebookEdit" => ("notebook_path", FileChangeKind::Modified),
        _ => return None,
    };
    let path = info.input[path_key].as_str()?;
    let kind = if result["type"] == "create" {
        FileChangeKind::Created
    } else {
        default_kind
    };
    let diff = patch_text(&result["structuredPatch"]).or_else(|| match info.name.as_str() {
        "Write" if kind == FileChangeKind::Created => info.input["content"]
            .as_str()
            .map(|c| c.lines().map(|l| format!("+{l}\n")).collect()),
        "Edit" => match (
            info.input["old_string"].as_str(),
            info.input["new_string"].as_str(),
        ) {
            (Some(o), Some(n)) => Some(format!("-{o}\n+{n}\n")),
            _ => None,
        },
        _ => None,
    });
    Some(AgentEvent::FileChange {
        path: PathBuf::from(path),
        kind,
        diff,
    })
}

/// Render the CLI's `structuredPatch` hunks as a unified diff body.
fn patch_text(patch: &Value) -> Option<String> {
    let hunks = patch.as_array().filter(|h| !h.is_empty())?;
    let mut s = String::new();
    for h in hunks {
        s += &format!(
            "@@ -{},{} +{},{} @@\n",
            h["oldStart"], h["oldLines"], h["newStart"], h["newLines"]
        );
        for l in h["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            s += l;
            s.push('\n');
        }
    }
    Some(s)
}
