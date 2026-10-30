//! Pure translation from Codex notifications / server requests to normalized
//! [`AgentEvent`]s. No I/O, so it is exercised directly by transcript replay.

use crate::protocol::*;
use crate::rpc::RequestId;
use del_agent::{AgentEvent, ApprovalKind, ApprovalResponse, FileChangeKind, ToolStatus};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

/// Cap on summaries copied into events; full output stays in Codex's rollout.
const SUMMARY_LIMIT: usize = 4096;

/// How a pending approval must be answered on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalMethod {
    Command,
    FileChange,
    /// Carries the permission profile the agent asked for.
    Permissions(Value),
    /// A confirmation-only MCP form; accepting it grants only this call.
    McpConfirmation,
}

impl ApprovalMethod {
    /// JSON-RPC `result` for the user's answer.
    pub fn result(&self, response: ApprovalResponse) -> Value {
        let approve = response == ApprovalResponse::Approve;
        match self {
            ApprovalMethod::Command | ApprovalMethod::FileChange => {
                json!({ "decision": if approve { "accept" } else { "decline" } })
            }
            ApprovalMethod::McpConfirmation => json!({
                "action": if approve { "accept" } else { "decline" },
                "content": if approve { json!({}) } else { Value::Null },
            }),
            ApprovalMethod::Permissions(requested) => {
                let granted = if approve {
                    requested.clone()
                } else {
                    json!({})
                };
                json!({ "permissions": granted, "scope": "turn" })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestAction {
    /// Surface to the user; answer later via [`ApprovalMethod::result`].
    Approval {
        event: AgentEvent,
        method: ApprovalMethod,
    },
    /// We cannot service this request; reply with a JSON-RPC error.
    Reject {
        message: String,
        event: Option<AgentEvent>,
    },
}

/// Per-thread translation state.
#[derive(Debug, Default)]
pub struct Mapper {
    turn_id: Option<String>,
    last_completed: Option<String>,
    /// Replaying persisted turns: errors are not re-raised.
    history_mode: bool,
    /// itemId -> one-line description, so approvals (which only carry an
    /// itemId for file changes) can show what is being approved.
    item_detail: HashMap<String, String>,
}

impl Mapper {
    /// Rebuild the same events used by the live chat from persisted turn items.
    /// User/reasoning items are snapshot-only here: the live composer already
    /// inserts its user messages and reasoning arrives as deltas.
    pub fn history(&mut self, turns: &[Value]) -> Vec<AgentEvent> {
        self.history_mode = true;
        let mut events = Vec::new();
        for turn in turns {
            let Some(id) = turn["id"].as_str() else {
                continue;
            };
            events.extend(self.notification("turn/started", &json!({"turn": turn})));
            for item in turn["items"].as_array().into_iter().flatten() {
                match item["type"].as_str() {
                    Some("userMessage") => {
                        let text = item["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|content| match content["type"].as_str() {
                                Some("text") => content["text"].as_str().map(str::to_owned),
                                Some("localImage") => content["path"]
                                    .as_str()
                                    .map(|path| format!("[attached image: {path}]")),
                                Some("image") => Some("[attached image]".into()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        events.push(AgentEvent::UserMessage { text });
                    }
                    Some("reasoning") => {
                        let text = ["summary", "content"]
                            .into_iter()
                            .flat_map(|field| {
                                item[field]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !text.is_empty() {
                            events.push(AgentEvent::ReasoningDelta {
                                turn_id: id.into(),
                                text,
                            });
                        }
                    }
                    _ => {
                        let params = json!({"turnId": id, "item": item});
                        events.extend(self.notification("item/started", &params));
                        events.extend(self.notification("item/completed", &params));
                    }
                }
            }
            // A persisted turn still marked `inProgress` was cut off (restart,
            // crash); close it as interrupted so the session is not Busy.
            let mut turn = turn.clone();
            if turn["status"].as_str() == Some("inProgress") {
                turn["status"] = json!("interrupted");
            }
            events.extend(self.notification("turn/completed", &json!({"turn": turn})));
        }
        self.history_mode = false;
        events
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    /// Record a turn id learned from a `turn/start` response, unless that turn
    /// already completed (its notifications can overtake the response).
    pub fn note_turn(&mut self, id: &str) {
        if self.last_completed.as_deref() != Some(id) {
            self.turn_id = Some(id.to_owned());
        }
    }

    pub fn notification(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match method {
            "thread/goal/updated" => {
                if let Some(goal) = crate::commands::goal(&params["goal"]) {
                    out.push(AgentEvent::GoalUpdated { goal: Some(goal) });
                }
            }
            "thread/goal/cleared" => out.push(AgentEvent::GoalUpdated { goal: None }),
            "turn/started" => {
                if let Some(n) = decode::<TurnNotification>(params) {
                    self.turn_id = Some(n.turn.id.clone());
                    out.push(AgentEvent::TurnStarted { turn_id: n.turn.id });
                }
            }
            "turn/completed" => {
                if let Some(n) = decode::<TurnNotification>(params) {
                    let status = match n.turn.status {
                        Some(TurnStatus::Interrupted) => del_agent::TurnStatus::Interrupted,
                        Some(TurnStatus::Failed) => del_agent::TurnStatus::Failed,
                        _ => del_agent::TurnStatus::Completed,
                    };
                    if status == del_agent::TurnStatus::Failed
                        && !self.history_mode
                        && let Some(e) = n.turn.error
                    {
                        out.push(AgentEvent::Error {
                            message: e.message,
                            recoverable: false,
                        });
                    }
                    self.turn_id = None;
                    self.last_completed = Some(n.turn.id.clone());
                    self.item_detail.clear();
                    out.push(AgentEvent::TurnCompleted {
                        turn_id: n.turn.id,
                        status,
                    });
                }
            }
            "item/agentMessage/delta" => {
                if let Some(d) = decode::<DeltaParams>(params) {
                    out.push(AgentEvent::AssistantTextDelta {
                        turn_id: d.turn_id,
                        text: d.delta,
                    });
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                if let Some(d) = decode::<DeltaParams>(params) {
                    out.push(AgentEvent::ReasoningDelta {
                        turn_id: d.turn_id,
                        text: d.delta,
                    });
                }
            }
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                if let Some(d) = decode::<DeltaParams>(params) {
                    out.push(AgentEvent::ToolCallUpdated {
                        id: d.item_id,
                        output_summary: truncate(&d.delta),
                    });
                }
            }
            "item/mcpToolCall/progress" => {
                if let (Some(id), Some(msg)) =
                    (params["itemId"].as_str(), params["message"].as_str())
                {
                    out.push(AgentEvent::ToolCallUpdated {
                        id: id.to_owned(),
                        output_summary: truncate(msg),
                    });
                }
            }
            "item/started" => {
                if let Some(p) = decode::<ItemParams>(params) {
                    self.item_started(p.item, &mut out);
                }
            }
            "item/completed" => {
                if let Some(p) = decode::<ItemParams>(params) {
                    self.item_completed(p.turn_id, p.item, &mut out);
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(u) = decode::<TokenUsageParams>(params) {
                    // Thread-cumulative totals, so the UI can show a running figure.
                    out.push(AgentEvent::TokenUsage {
                        input: u.token_usage.total.input_tokens,
                        output: u.token_usage.total.output_tokens,
                        cached: u.token_usage.total.cached_input_tokens,
                    });
                }
            }
            "error" => {
                if let Some(e) = decode::<ErrorParams>(params) {
                    out.push(AgentEvent::Error {
                        message: e.error.message,
                        recoverable: e.will_retry,
                    });
                }
            }
            // serverRequest/resolved is handled by the backend (pending approvals);
            // everything else (status, rate limits, MCP startup, ...) is not surfaced.
            _ => {}
        }
        out
    }

    fn item_started(&mut self, item: ThreadItem, out: &mut Vec<AgentEvent>) {
        match item {
            ThreadItem::CommandExecution { id, command, .. } => {
                self.item_detail.insert(id.clone(), command.clone());
                out.push(AgentEvent::ToolCallStarted {
                    id,
                    name: "shell".into(),
                    input_summary: truncate(&command),
                });
            }
            ThreadItem::FileChange { id, changes, .. } => {
                let summary = change_summary(&changes);
                self.item_detail.insert(id.clone(), summary.clone());
                out.push(AgentEvent::ToolCallStarted {
                    id,
                    name: "apply_patch".into(),
                    input_summary: summary,
                });
            }
            ThreadItem::McpToolCall {
                id,
                server,
                tool,
                arguments,
                ..
            } => {
                let summary = truncate(&arguments.to_string());
                self.item_detail
                    .insert(id.clone(), format!("{server}/{tool} {summary}"));
                out.push(AgentEvent::ToolCallStarted {
                    id,
                    name: format!("{server}/{tool}"),
                    input_summary: summary,
                });
            }
            ThreadItem::DynamicToolCall {
                id,
                tool,
                arguments,
                ..
            } => {
                out.push(AgentEvent::ToolCallStarted {
                    id,
                    name: tool,
                    input_summary: truncate(&arguments.to_string()),
                });
            }
            ThreadItem::WebSearch { id, query } => {
                out.push(AgentEvent::ToolCallStarted {
                    id,
                    name: "web_search".into(),
                    input_summary: truncate(&query),
                });
            }
            _ => {}
        }
    }

    fn item_completed(&mut self, turn_id: String, item: ThreadItem, out: &mut Vec<AgentEvent>) {
        match item {
            ThreadItem::AgentMessage { text, .. } => {
                out.push(AgentEvent::AssistantMessage { turn_id, text });
            }
            ThreadItem::CommandExecution {
                id,
                status,
                aggregated_output,
                exit_code,
                ..
            } => {
                let mut summary = truncate(aggregated_output.as_deref().unwrap_or(""));
                if let Some(code) = exit_code
                    && code != 0
                {
                    summary.push_str(&format!("\n[exit code {code}]"));
                }
                out.push(AgentEvent::ToolCallFinished {
                    id,
                    status: tool_status(status),
                    output_summary: summary,
                });
            }
            ThreadItem::FileChange {
                id,
                changes,
                status,
            } => {
                let tool = tool_status(status);
                if tool == ToolStatus::Completed {
                    for c in &changes {
                        let kind = match c.kind {
                            PatchChangeKind::Add => FileChangeKind::Created,
                            PatchChangeKind::Delete => FileChangeKind::Deleted,
                            _ => FileChangeKind::Modified,
                        };
                        out.push(AgentEvent::FileChange {
                            path: PathBuf::from(&c.path),
                            kind,
                            diff: (!c.diff.is_empty()).then(|| c.diff.clone()),
                        });
                    }
                }
                out.push(AgentEvent::ToolCallFinished {
                    id,
                    status: tool,
                    output_summary: change_summary(&changes),
                });
            }
            ThreadItem::McpToolCall {
                id,
                status,
                result,
                error,
                ..
            } => {
                let summary = match (error, result) {
                    (Some(e), _) => e.message,
                    (None, Some(r)) => mcp_text(&r),
                    _ => String::new(),
                };
                out.push(AgentEvent::ToolCallFinished {
                    id,
                    status: tool_status(status),
                    output_summary: truncate(&summary),
                });
            }
            ThreadItem::DynamicToolCall {
                id,
                status,
                success,
                content_items,
                ..
            } => {
                let mut st = tool_status(status);
                if success == Some(false) {
                    st = ToolStatus::Failed;
                }
                let summary = content_items.map(|c| c.to_string()).unwrap_or_default();
                out.push(AgentEvent::ToolCallFinished {
                    id,
                    status: st,
                    output_summary: truncate(&summary),
                });
            }
            ThreadItem::WebSearch { id, .. } => {
                out.push(AgentEvent::ToolCallFinished {
                    id,
                    status: ToolStatus::Completed,
                    output_summary: String::new(),
                });
            }
            ThreadItem::Other => {}
        }
    }

    /// Handle a server-initiated request.
    pub fn server_request(
        &mut self,
        id: &RequestId,
        method: &str,
        params: &Value,
    ) -> RequestAction {
        match method {
            "item/commandExecution/requestApproval" => {
                let cmd = params["command"].as_str().unwrap_or("(unknown command)");
                let detail = match params["reason"].as_str() {
                    Some(r) if !r.is_empty() => format!("{cmd}\n{r}"),
                    _ => cmd.to_owned(),
                };
                RequestAction::Approval {
                    event: AgentEvent::ApprovalRequested {
                        id: id.to_string(),
                        kind: ApprovalKind::Command,
                        detail,
                    },
                    method: ApprovalMethod::Command,
                }
            }
            "item/fileChange/requestApproval" => {
                let mut detail = params["itemId"]
                    .as_str()
                    .and_then(|i| self.item_detail.get(i).cloned())
                    .unwrap_or_else(|| "file changes".into());
                if let Some(r) = params["reason"].as_str().filter(|r| !r.is_empty()) {
                    detail.push('\n');
                    detail.push_str(r);
                }
                RequestAction::Approval {
                    event: AgentEvent::ApprovalRequested {
                        id: id.to_string(),
                        kind: ApprovalKind::FileChange,
                        detail,
                    },
                    method: ApprovalMethod::FileChange,
                }
            }
            "mcpServer/elicitation/request" if confirmation_only(params) => {
                let server = params["serverName"].as_str().unwrap_or("MCP server");
                let message = params["message"].as_str().unwrap_or("Confirm tool call");
                let mut detail = format!("{server}: {message}");
                if let Some(arguments) = params.get("_meta").and_then(|m| m.get("tool_params")) {
                    detail.push('\n');
                    detail.push_str(&arguments.to_string());
                }
                RequestAction::Approval {
                    event: AgentEvent::ApprovalRequested {
                        id: id.to_string(),
                        kind: ApprovalKind::Other,
                        detail,
                    },
                    method: ApprovalMethod::McpConfirmation,
                }
            }
            "item/permissions/requestApproval" => {
                let reason = params["reason"]
                    .as_str()
                    .unwrap_or("additional permissions");
                RequestAction::Approval {
                    event: AgentEvent::ApprovalRequested {
                        id: id.to_string(),
                        kind: ApprovalKind::Other,
                        detail: format!("{reason}\n{}", params["permissions"]),
                    },
                    method: ApprovalMethod::Permissions(params["permissions"].clone()),
                }
            }
            other => RequestAction::Reject {
                message: format!("del does not support server request `{other}`"),
                event: Some(AgentEvent::Error {
                    message: format!("Codex asked for `{other}`, which del does not support yet"),
                    recoverable: true,
                }),
            },
        }
    }
}

/// The approval UI can answer yes/no, but cannot fill forms or complete URL
/// authentication flows. Never invent field values or silently accept those.
fn confirmation_only(params: &Value) -> bool {
    let schema = &params["requestedSchema"];
    params["mode"] == "form"
        && schema["type"] == "object"
        && schema["properties"]
            .as_object()
            .is_some_and(|p| p.is_empty())
        && schema
            .get("required")
            .is_none_or(|r| r.is_null() || r.as_array().is_some_and(|a| a.is_empty()))
        && schema.as_object().is_some_and(|s| {
            s.keys()
                .all(|k| matches!(k.as_str(), "$schema" | "type" | "properties" | "required"))
        })
}

fn decode<T: serde::de::DeserializeOwned>(v: &Value) -> Option<T> {
    T::deserialize(v).ok()
}

fn tool_status(s: ItemStatus) -> ToolStatus {
    match s {
        ItemStatus::Declined => ToolStatus::Declined,
        ItemStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Completed,
    }
}

fn change_summary(changes: &[FileUpdateChange]) -> String {
    changes
        .iter()
        .map(|c| {
            let verb = match c.kind {
                PatchChangeKind::Add => "add",
                PatchChangeKind::Delete => "delete",
                _ => "update",
            };
            format!("{verb} {}", c.path)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mcp_text(result: &Value) -> String {
    let texts: Vec<&str> = result["content"]
        .as_array()
        .map(|a| a.iter().filter_map(|b| b["text"].as_str()).collect())
        .unwrap_or_default();
    if texts.is_empty() {
        result.to_string()
    } else {
        texts.join("\n")
    }
}

fn truncate(s: &str) -> String {
    if s.len() <= SUMMARY_LIMIT {
        return s.to_owned();
    }
    let mut end = SUMMARY_LIMIT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &s[..end])
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn stale_in_progress_turns_are_closed_as_interrupted_and_errors_stay_quiet() {
        let turns = vec![
            json!({"id":"t1","status":"failed","error":{"message":"boom"},"items":[]}),
            json!({"id":"t2","status":"inProgress","items":[
                {"type":"agentMessage","id":"a","text":"partial"}]}),
        ];
        let mut mapper = Mapper::default();
        let events = mapper.history(&turns);
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
        assert!(events.iter().any(|e| matches!(e,
            AgentEvent::TurnCompleted { turn_id, status: del_agent::TurnStatus::Failed } if turn_id == "t1")));
        assert!(matches!(events.last(), Some(AgentEvent::TurnCompleted {
            turn_id, status: del_agent::TurnStatus::Interrupted }) if turn_id == "t2"));
        assert_eq!(mapper.turn_id(), None);
        // Live notifications after history are not muted.
        let live = mapper.notification(
            "turn/completed",
            &json!({"turn":{"id":"t3","status":"failed","error":{"message":"live"}}}),
        );
        assert!(
            live.iter()
                .any(|e| matches!(e, AgentEvent::Error { message, .. } if message == "live"))
        );
    }
}

#[cfg(test)]
mod mcp_confirmation_tests {
    use super::*;

    #[test]
    fn confirmation_requires_user_approval_and_never_persists_it() {
        let request = json!({"serverName":"del_browser","mode":"form",
            "message":"Allow reply_to_session?", "requestedSchema":{"type":"object","properties":{}},
            "_meta":{"persist":["session","always"],"tool_params":{"text":"answer"}}});
        let action = Mapper::default().server_request(
            &RequestId::Int(7),
            "mcpServer/elicitation/request",
            &request,
        );
        let RequestAction::Approval { event, method } = action else {
            panic!("missing approval")
        };
        assert!(
            matches!(event, AgentEvent::ApprovalRequested { detail, .. } if detail.contains("reply_to_session") && detail.contains("answer"))
        );
        assert_eq!(
            method.result(ApprovalResponse::Approve),
            json!({"action":"accept","content":{}})
        );
        assert_eq!(
            method.result(ApprovalResponse::Deny),
            json!({"action":"decline","content":null})
        );
    }

    #[test]
    fn structured_forms_and_url_flows_are_not_accepted_as_confirmations() {
        for request in [
            json!({"mode":"form","requestedSchema":{"type":"object","properties":{"email":{"type":"string"}}}}),
            json!({"mode":"url","url":"https://example.com"}),
            json!({"mode":"form","requestedSchema":{"type":"object","properties":{},"required":["secret"]}}),
            json!({"mode":"form","requestedSchema":{"type":"object","properties":{},"oneOf":[{}]}}),
        ] {
            assert!(matches!(
                Mapper::default().server_request(
                    &RequestId::Int(1),
                    "mcpServer/elicitation/request",
                    &request
                ),
                RequestAction::Reject { .. }
            ));
        }
    }
}
