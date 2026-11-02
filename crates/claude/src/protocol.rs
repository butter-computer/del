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
    cost_scope: Option<String>,
    context_tokens: Option<u64>,
    context_window: Option<u64>,
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
    pub fn mark_interrupting(&mut self) -> Vec<AgentEvent> {
        self.interrupting = true;
        // The CLI abandons its permission prompts when interrupted.
        self.drop_approvals()
    }

    /// The interrupt never reached the CLI; forget it.
    pub fn unmark_interrupting(&mut self) {
        self.interrupting = false;
    }

    /// Forget every pending prompt, retiring its card.
    fn drop_approvals(&mut self) -> Vec<AgentEvent> {
        let mut ids: Vec<String> = self.approvals.drain().map(|(id, _)| id).collect();
        ids.sort();
        ids.into_iter()
            .map(|id| AgentEvent::ApprovalResolved { id })
            .collect()
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
        let mut out = self.drop_approvals();
        out.extend(
            self.turns
                .pop_front()
                .map(|turn_id| AgentEvent::TurnCompleted {
                    turn_id,
                    status: TurnStatus::Failed,
                }),
        );
        self.turns.clear();
        self.consumed_inputs = 0;
        out
    }

    fn turn(&self) -> String {
        self.turns.front().cloned().unwrap_or_default()
    }

    /// Translate one stdout line. Unparseable or unrecognised lines yield nothing.
    pub fn handle_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        self.handle_value(&v)
    }

    /// Translate one already-parsed stdout record.
    pub fn handle_value(&mut self, v: &Value) -> Vec<AgentEvent> {
        if let Some(id) = v["session_id"].as_str().or_else(|| v["sessionId"].as_str()) {
            self.session_id = Some(id.to_owned());
        }
        if let Some(event) = crate::goals::record(v) {
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
            Some("user") => self.user(v),
            Some("control_request") => self.control_request(v),
            Some("control_cancel_request") => {
                // The CLI withdrew a permission prompt; an answer would be stale.
                match v["request_id"].as_str() {
                    Some(id) if self.approvals.remove(id).is_some() => {
                        vec![AgentEvent::ApprovalResolved { id: id.to_owned() }]
                    }
                    _ => vec![],
                }
            }
            Some("result") => self.result(v),
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
        if let Some(input) = msg["usage"]["input_tokens"].as_u64() {
            let usage = &msg["usage"];
            let tokens = input
                .saturating_add(usage["cache_read_input_tokens"].as_u64().unwrap_or(0))
                .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0))
                .saturating_add(usage["output_tokens"].as_u64().unwrap_or(0));
            self.context_tokens = Some(tokens);
            self.context_window = msg["model"]
                .as_str()
                .and_then(del_agent::pricing::context_window);
            let estimated = msg["model"].as_str().and_then(|model| {
                del_agent::pricing::estimate(
                    model,
                    input,
                    usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
                    usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                    usage["output_tokens"].as_u64().unwrap_or(0),
                )
            });
            let scope = msg["id"].as_str().map(|id| {
                del_agent::pricing::cost_scope(
                    self.session_id.as_deref(),
                    format!("estimate:claude:{id}"),
                )
            });
            out.push(AgentEvent::UsageSnapshot {
                context_tokens: Some(tokens),
                context_window: self.context_window,
                cost_scope: scope.clone(),
                cost_usd: scope.and(estimated),
            });
        }
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
        // total_cost_usd is cumulative for this CLI process. Its first result UUID
        // gives replays the same scope; a restarted process gets a different scope.
        let cost = v["total_cost_usd"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.);
        if cost.is_some() && self.cost_scope.is_none() {
            self.cost_scope = Some(format!(
                "claude:{}",
                v["uuid"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            ));
        }
        let windows: Vec<_> = v["modelUsage"]
            .as_object()
            .into_iter()
            .flat_map(|models| models.values())
            .filter_map(|model| model["contextWindow"].as_u64().filter(|n| *n > 0))
            .collect();
        // A multi-model result cannot identify which capacity belongs to the last reply.
        let window = windows
            .first()
            .copied()
            .filter(|first| windows.iter().all(|n| n == first))
            .or(self.context_window);
        if cost.is_some() || self.context_tokens.is_some() || window.is_some() {
            out.push(AgentEvent::UsageSnapshot {
                context_tokens: self.context_tokens,
                context_window: window,
                cost_scope: self
                    .cost_scope
                    .clone()
                    .map(|scope| del_agent::pricing::cost_scope(self.session_id.as_deref(), scope)),
                cost_usd: cost,
            });
        }
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
        // Prompts belong to the turn that is ending.
        out.extend(self.drop_approvals());
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

#[cfg(test)]
mod approval_tests {
    use super::*;

    fn ask(t: &mut Translator, id: &str) {
        t.handle_value(&json!({"type":"control_request","request_id":id,
            "request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}));
    }

    #[test]
    fn withdrawn_finished_and_interrupted_prompts_cannot_be_answered() {
        let mut t = Translator::new();
        t.enqueue_turn("t".into());
        let resolved = |id: &str| AgentEvent::ApprovalResolved { id: id.into() };
        ask(&mut t, "a");
        let cancel = json!({"type":"control_cancel_request","request_id":"a"});
        assert_eq!(t.handle_value(&cancel), vec![resolved("a")]);
        assert!(t.approval_reply("a", ApprovalResponse::Approve).is_none());
        // A repeated or unknown cancel retires nothing.
        assert!(t.handle_value(&cancel).is_empty());

        ask(&mut t, "b");
        let evs = t.handle_value(&json!({"type":"result","usage":{}}));
        let at = evs.iter().position(|e| *e == resolved("b")).unwrap();
        assert!(matches!(evs[at + 1], AgentEvent::TurnCompleted { .. }));
        assert!(t.approval_reply("b", ApprovalResponse::Approve).is_none());

        t.enqueue_turn("u".into());
        ask(&mut t, "c");
        assert_eq!(t.mark_interrupting(), vec![resolved("c")]);
        assert!(t.approval_reply("c", ApprovalResponse::Deny).is_none());

        ask(&mut t, "d");
        assert_eq!(t.abort()[0], resolved("d"));
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;
    #[test]
    fn native_session_identity_namespaces_live_and_replayed_costs() {
        let mut translator = Translator::new();
        let events = translator.handle_value(&json!({"sessionId":"native-a","type":"assistant","message":{"id":"message","model":"claude-sonnet-5-5","usage":{"input_tokens":100},"content":[]}}));
        let expected =
            del_agent::pricing::cost_scope(Some("native-a"), "estimate:claude:message".into());
        assert!(events.iter().any(|event| matches!(event, AgentEvent::UsageSnapshot {cost_scope:Some(scope), ..} if scope == &expected)));
        let result = translator.handle_value(
            &json!({"type":"result","session_id":"native-a","uuid":"result","total_cost_usd":0.1}),
        );
        let expected = del_agent::pricing::cost_scope(Some("native-a"), "claude:result".into());
        assert!(result.iter().any(|event| matches!(event, AgentEvent::UsageSnapshot {cost_scope:Some(scope), ..} if scope == &expected)));
    }
    #[test]
    fn assistant_usage_restores_known_capacity_and_estimate_by_message_id() {
        let mut translator = Translator::new();
        let message = json!({"type":"assistant","message":{"id":"message-1","model":"claude-haiku-5-5","usage":{"input_tokens":1000,"cache_read_input_tokens":2000,"output_tokens":100},"content":[]}});
        let events = translator.handle_value(&message);
        assert!(events.iter().any(|event| matches!(event, AgentEvent::UsageSnapshot {context_tokens:Some(3100),context_window:Some(1000000),cost_scope:Some(scope),cost_usd:Some(cost)} if scope == "estimate:claude:message-1" && (*cost - 0.00017).abs() < 1e-9)));
    }
    #[test]
    fn latest_context_and_cumulative_cost_have_independent_scopes() {
        let mut translator = Translator::new();
        let assistant = json!({"type":"assistant", "message":{"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":5},"content":[]}});
        assert!(matches!(
            translator
                .handle_value(&assistant)
                .iter()
                .find(|event| matches!(event, AgentEvent::UsageSnapshot { .. }))
                .unwrap(),
            AgentEvent::UsageSnapshot {
                context_tokens: Some(135),
                ..
            }
        ));
        let result = |uuid, cost| json!({"type":"result","uuid":uuid,"total_cost_usd":cost,"usage":{"input_tokens":99999},"modelUsage":{"model":{"contextWindow":200000}}});
        let first = translator.handle_value(&result("first", 0.2));
        let next = translator.handle_value(&result("second", 0.4));
        for events in [&first, &next] {
            assert!(
                matches!(&events[0], AgentEvent::UsageSnapshot {context_tokens:Some(135), context_window:Some(200000), cost_scope:Some(scope), ..} if scope == "claude:first")
            );
        }
        let restarted = Translator::new().handle_value(&result("restart", 0.1));
        assert!(
            matches!(&restarted[0], AgentEvent::UsageSnapshot {cost_scope:Some(scope), context_tokens:None, ..} if scope == "claude:restart")
        );
    }
}
