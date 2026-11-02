//! OpenCode v2 native schemas, verified against the v2.1.4 protocol package.
use del_agent::*;
use serde_json::Value;
use std::collections::HashMap;
pub fn string(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().into()
}
fn error_message(error: &Value) -> String {
    let mut message = error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("OpenCode request failed.")
        .to_owned();
    if let Some(ms) = error["retryAfterMs"].as_u64().filter(|ms| *ms > 0) {
        let minutes = ms.div_ceil(60_000);
        let wait = if minutes >= 60 {
            format!("{}h {}m", minutes / 60, minutes % 60)
        } else {
            format!("{minutes}m")
        };
        message.push_str(&format!(" Provider retry hint: about {wait}."));
    }
    message
}
pub fn models(value: &Value, default: &Value) -> Vec<ModelInfo> {
    value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["enabled"] != false)
        .map(|m| {
            let id = format!("{}/{}", string(m, "providerID"), string(m, "id"));
            let efforts = m["variants"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| EffortOption::named(string(v, "id")))
                .collect();
            ModelInfo {
                id,
                name: string(m, "name"),
                description: string(m, "providerID"),
                efforts,
                default_effort: None,
                fast_tier: None,
                is_default: m["id"] == default["data"]["id"]
                    && m["providerID"] == default["data"]["providerID"],
            }
        })
        .collect()
}
pub fn commands(commands: &Value, skills: &Value) -> Vec<ComposerCommand> {
    let mut result: Vec<_> = skills["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| ComposerCommand {
            name: string(s, "name"),
            description: string(s, "description"),
            argument_hint: String::new(),
            kind: CommandKind::Skill,
            source: "OpenCode".into(),
            target: Some(string(s, "id")),
        })
        .collect();
    for c in commands["data"].as_array().into_iter().flatten() {
        if result.iter().any(|s| s.name == string(c, "name")) {
            continue;
        }
        result.push(ComposerCommand {
            name: string(c, "name"),
            description: string(c, "description"),
            argument_hint: "arguments".into(),
            kind: CommandKind::Action,
            source: "OpenCode".into(),
            target: None,
        });
    }
    result
}
pub fn diffs(cwd: &std::path::Path, value: &Value) -> Vec<AgentEvent> {
    value["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let path = file["file"].as_str()?;
            let kind = match file["status"].as_str()? {
                "added" => FileChangeKind::Created,
                "deleted" => FileChangeKind::Deleted,
                "modified" => FileChangeKind::Modified,
                _ => return None,
            };
            Some(AgentEvent::FileChange {
                path: cwd.join(path),
                kind,
                diff: file["patch"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

/// Cap on summaries copied into events, as for the other backends; the full
/// tool input and output stay in OpenCode's own transcript.
const SUMMARY_LIMIT: usize = 4096;

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

fn content(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
fn usage(v: &Value) -> AgentEvent {
    AgentEvent::TokenUsage {
        input: v["input"].as_u64().unwrap_or(0),
        output: v["output"].as_u64().unwrap_or(0),
        cached: v["cache"]["read"].as_u64().unwrap_or(0),
    }
}
fn text_id(d: &Value) -> String {
    format!(
        "{}:{}",
        string(d, "assistantMessageID"),
        d["ordinal"].as_u64().unwrap_or(0)
    )
}
#[derive(Default)]
pub struct Stream {
    reasoning: HashMap<String, String>,
    turn: String,
}
impl Stream {
    pub fn event(&mut self, e: &Value) -> Vec<AgentEvent> {
        let d = &e["data"];
        match e["type"].as_str() {
            Some("session.execution.started") => {
                self.turn = string(e, "id");
                vec![AgentEvent::TurnStarted {
                    turn_id: self.turn.clone(),
                }]
            }
            Some("session.execution.succeeded") => vec![AgentEvent::TurnCompleted {
                turn_id: self.turn.clone(),
                status: TurnStatus::Completed,
            }],
            Some("session.execution.interrupted") => vec![AgentEvent::TurnCompleted {
                turn_id: self.turn.clone(),
                status: TurnStatus::Interrupted,
            }],
            Some("session.execution.failed") => vec![
                AgentEvent::Error {
                    message: error_message(&d["error"]),
                    recoverable: true,
                },
                AgentEvent::TurnCompleted {
                    turn_id: self.turn.clone(),
                    status: TurnStatus::Failed,
                },
            ],
            Some("session.text.delta") => vec![AgentEvent::AssistantTextDelta {
                turn_id: text_id(d),
                text: string(d, "delta"),
            }],
            Some("session.text.ended") => vec![AgentEvent::AssistantMessage {
                turn_id: text_id(d),
                text: string(d, "text"),
            }],
            Some("session.reasoning.delta") => {
                let id = text_id(d);
                let delta = string(d, "delta");
                self.reasoning
                    .entry(id.clone())
                    .or_default()
                    .push_str(&delta);
                vec![AgentEvent::ReasoningDelta {
                    turn_id: id,
                    text: delta,
                }]
            }
            Some("session.reasoning.ended") => {
                let id = text_id(d);
                let previous = self.reasoning.remove(&id).unwrap_or_default();
                let full = string(d, "text");
                full.strip_prefix(&previous)
                    .filter(|t| !t.is_empty())
                    .map(|text| {
                        vec![AgentEvent::ReasoningDelta {
                            turn_id: id,
                            text: text.into(),
                        }]
                    })
                    .unwrap_or_default()
            }
            Some("session.tool.input.started") => vec![AgentEvent::ToolCallStarted {
                id: string(d, "id"),
                name: string(d, "name"),
                input_summary: String::new(),
            }],
            Some("session.tool.called") => vec![AgentEvent::ToolCallUpdated {
                id: string(d, "id"),
                output_summary: truncate(&d["input"].to_string()),
            }],
            Some("session.tool.progress") => vec![AgentEvent::ToolCallUpdated {
                id: string(d, "id"),
                output_summary: truncate(&d["metadata"].to_string()),
            }],
            Some("session.tool.success") => vec![AgentEvent::ToolCallFinished {
                id: string(d, "id"),
                status: ToolStatus::Completed,
                output_summary: truncate(&content(&d["content"])),
            }],
            Some("session.tool.failed") => vec![AgentEvent::ToolCallFinished {
                id: string(d, "id"),
                status: ToolStatus::Failed,
                output_summary: truncate(&error_message(&d["error"])),
            }],
            Some("session.step.ended") => vec![usage(&d["tokens"])],
            Some("permission.asked") => vec![AgentEvent::ApprovalRequested {
                id: string(d, "id"),
                kind: ApprovalKind::Other,
                detail: truncate(&format!("{}: {}", string(d, "action"), d["resources"])),
            }],
            _ => vec![],
        }
    }
    pub fn message(&mut self, m: &Value) -> Vec<AgentEvent> {
        match m["type"].as_str() {
            Some("user") => vec![AgentEvent::UserMessage {
                text: string(m, "text"),
            }],
            Some("assistant") => {
                let mut out = vec![];
                for (ordinal, c) in m["content"].as_array().into_iter().flatten().enumerate() {
                    let id = format!("{}:{ordinal}", string(m, "id"));
                    match c["type"].as_str() {
                        Some("text") => out.push(AgentEvent::AssistantMessage {
                            turn_id: id,
                            text: string(c, "text"),
                        }),
                        Some("reasoning") => out.push(AgentEvent::ReasoningDelta {
                            turn_id: id,
                            text: string(c, "text"),
                        }),
                        Some("tool") => {
                            let state = &c["state"];
                            out.push(AgentEvent::ToolCallStarted {
                                id: string(c, "id"),
                                name: string(c, "name"),
                                input_summary: truncate(&state["input"].to_string()),
                            });
                            if matches!(state["status"].as_str(), Some("completed" | "error")) {
                                out.push(AgentEvent::ToolCallFinished {
                                    id: string(c, "id"),
                                    status: if state["status"] == "error" {
                                        ToolStatus::Failed
                                    } else {
                                        ToolStatus::Completed
                                    },
                                    output_summary: truncate(&if state["status"] == "error" {
                                        error_message(&state["error"])
                                    } else {
                                        content(&state["content"])
                                    }),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if m["tokens"].is_object() {
                    out.push(usage(&m["tokens"]));
                }
                out
            }
            _ => vec![],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tool_summaries_are_capped() {
        let big = "é".repeat(5000);
        let events = Stream::default().event(&json!({"type":"session.tool.called",
            "data":{"id":"t","input":{"command":big}}}));
        let AgentEvent::ToolCallUpdated { output_summary, .. } = &events[0] else {
            panic!("tool update");
        };
        assert!(output_summary.len() < SUMMARY_LIMIT + 32);
        assert!(output_summary.ends_with("[truncated]"));
    }

    #[test]
    fn provider_quota_error_is_readable_and_preserves_retry_hint() {
        let mut stream = Stream::default();
        let events = stream.event(&json!({"type":"session.execution.failed","data":{"error":{
            "type":"provider.free-limit", "message":"Free usage limit reached.",
            "status":429, "retryAfterMs":23646000,
            "response":{"body":"raw provider diagnostics"}
        }}}));
        let AgentEvent::Error { message, .. } = &events[0] else {
            panic!("missing error")
        };
        assert_eq!(
            message,
            "Free usage limit reached. Provider retry hint: about 6h 35m."
        );
        assert!(matches!(
            events[1],
            AgentEvent::TurnCompleted {
                status: TurnStatus::Failed,
                ..
            }
        ));
    }
    #[test]
    fn variants_and_native_skill_id_are_preserved() {
        let m = json!({"data":[{"id":"org/model","providerID":"local","name":"Model","enabled":true,"variants":[{"id":"low"},{"id":"high"}]},{"id":"off","enabled":false}]});
        let result = models(&m, &json!({"data":{"id":"org/model","providerID":"local"}}));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "local/org/model");
        assert_eq!(result[0].efforts[1].id, "high");
        assert!(result[0].is_default);
        assert!(result[0].fast_tier.is_none());
        let c = commands(
            &json!({"data":[]}),
            &json!({"data":[{"id":"opaque-skill","name":"review","path":"/tmp/SKILL.md"}]}),
        );
        assert_eq!(c[0].target.as_deref(), Some("opaque-skill"));
        assert_eq!(c[0].kind, CommandKind::Skill);
    }
    #[test]
    fn native_events_keep_text_and_reasoning_separate() {
        let mut s = Stream::default();
        let event = |kind: &str, text: &str| json!({"type":kind,"data":{"assistantMessageID":"msg_1","ordinal":0,"delta":text,"text":text}});
        assert!(
            matches!(&s.event(&event("session.text.delta","Hi"))[0],AgentEvent::AssistantTextDelta {text,..} if text=="Hi")
        );
        assert!(
            matches!(&s.event(&event("session.text.ended","Hi"))[0],AgentEvent::AssistantMessage {text,..} if text=="Hi")
        );
        s.event(&event("session.reasoning.delta", "Think"));
        assert!(
            s.event(&event("session.reasoning.ended", "Think"))
                .is_empty()
        );
    }

    #[test]
    fn execution_lifecycle_tools_usage_and_permissions() {
        let mut s = Stream::default();
        let ev = |kind: &str, data: Value| json!({"type":kind,"id":"evt","data":data});
        assert_eq!(
            s.event(&json!({"type":"session.execution.started","id":"exec_1","data":{}})),
            vec![AgentEvent::TurnStarted {
                turn_id: "exec_1".into()
            }]
        );
        assert_eq!(
            s.event(&ev(
                "session.tool.input.started",
                json!({"id":"call","name":"bash"})
            )),
            vec![AgentEvent::ToolCallStarted {
                id: "call".into(),
                name: "bash".into(),
                input_summary: String::new()
            }]
        );
        assert_eq!(
            s.event(&ev(
                "session.tool.called",
                json!({"id":"call","input":{"command":"ls"}})
            )),
            vec![AgentEvent::ToolCallUpdated {
                id: "call".into(),
                output_summary: "{\"command\":\"ls\"}".into()
            }]
        );
        assert_eq!(
            s.event(&ev(
                "session.tool.success",
                json!({"id":"call","content":[{"type":"text","text":"a"},{"type":"file"},{"type":"text","text":"b"}]})
            )),
            vec![AgentEvent::ToolCallFinished {
                id: "call".into(),
                status: ToolStatus::Completed,
                output_summary: "a\nb".into()
            }]
        );
        assert_eq!(
            s.event(&ev(
                "session.tool.failed",
                json!({"id":"other","error":"plain string error"})
            )),
            vec![AgentEvent::ToolCallFinished {
                id: "other".into(),
                status: ToolStatus::Failed,
                output_summary: "plain string error".into()
            }]
        );
        assert_eq!(
            s.event(&ev(
                "session.step.ended",
                json!({"tokens":{"input":5,"output":2,"cache":{"read":3,"write":1}}})
            )),
            vec![AgentEvent::TokenUsage {
                input: 5,
                output: 2,
                cached: 3
            }]
        );
        assert_eq!(
            s.event(&ev(
                "permission.asked",
                json!({"id":"perm_1","action":"edit","resources":["/w/a.rs"]})
            )),
            vec![AgentEvent::ApprovalRequested {
                id: "perm_1".into(),
                kind: ApprovalKind::Other,
                detail: "edit: [\"/w/a.rs\"]".into()
            }]
        );
        assert_eq!(
            s.event(&json!({"type":"session.execution.interrupted"})),
            vec![AgentEvent::TurnCompleted {
                turn_id: "exec_1".into(),
                status: TurnStatus::Interrupted
            }]
        );
        assert_eq!(
            s.event(&json!({"type":"session.execution.succeeded"})),
            vec![AgentEvent::TurnCompleted {
                turn_id: "exec_1".into(),
                status: TurnStatus::Completed
            }]
        );
        // A failure without a message still says something readable.
        let failed = s.event(&ev(
            "session.execution.failed",
            json!({"error":{"message":"  "}}),
        ));
        assert_eq!(
            failed[0],
            AgentEvent::Error {
                message: "OpenCode request failed.".into(),
                recoverable: true
            }
        );
    }

    #[test]
    fn unknown_and_malformed_events_are_ignored() {
        let mut s = Stream::default();
        for event in [
            json!({"type":"session.future.event","data":{}}),
            json!({"type":7}),
            json!({}),
            json!(null),
            json!([1, 2]),
            json!({"type":"server.connected"}),
        ] {
            assert!(s.event(&event).is_empty(), "{event}");
        }
        // Missing fields degrade to empty values rather than panicking.
        assert_eq!(
            s.event(&json!({"type":"session.text.delta"})),
            vec![AgentEvent::AssistantTextDelta {
                turn_id: ":0".into(),
                text: String::new()
            }]
        );
        assert_eq!(
            s.event(&json!({"type":"session.step.ended"})),
            vec![AgentEvent::TokenUsage {
                input: 0,
                output: 0,
                cached: 0
            }]
        );
    }

    #[test]
    fn reasoning_end_emits_only_the_unstreamed_remainder() {
        let mut s = Stream::default();
        let event = |kind: &str, text: &str| json!({"type":kind,"data":{"assistantMessageID":"m","ordinal":1,"delta":text,"text":text}});
        s.event(&event("session.reasoning.delta", "Think"));
        assert_eq!(
            s.event(&event("session.reasoning.ended", "Think harder")),
            vec![AgentEvent::ReasoningDelta {
                turn_id: "m:1".into(),
                text: " harder".into()
            }]
        );
        // Text that does not extend what was streamed is not repeated.
        s.event(&event("session.reasoning.delta", "abc"));
        assert!(s.event(&event("session.reasoning.ended", "xyz")).is_empty());
    }

    #[test]
    fn saved_messages_restore_text_reasoning_tools_and_usage() {
        let mut s = Stream::default();
        assert_eq!(
            s.message(&json!({"type":"user","id":"u","text":"hello"})),
            vec![AgentEvent::UserMessage {
                text: "hello".into()
            }]
        );
        let events = s.message(&json!({"type":"assistant","id":"a","content":[
            {"type":"reasoning","text":"think"},
            {"type":"text","text":"answer"},
            {"type":"tool","id":"t1","name":"read","state":{"status":"completed","input":{"path":"x"},"content":[{"text":"body"}]}},
            {"type":"tool","id":"t2","name":"bash","state":{"status":"error","input":{},"error":{"message":"denied"}}},
            {"type":"tool","id":"t3","name":"edit","state":{"status":"running","input":{}}},
            {"type":"future"}
        ],"tokens":{"input":1,"output":2,"cache":{"read":0}}}));
        assert_eq!(
            events,
            vec![
                AgentEvent::ReasoningDelta {
                    turn_id: "a:0".into(),
                    text: "think".into()
                },
                AgentEvent::AssistantMessage {
                    turn_id: "a:1".into(),
                    text: "answer".into()
                },
                AgentEvent::ToolCallStarted {
                    id: "t1".into(),
                    name: "read".into(),
                    input_summary: "{\"path\":\"x\"}".into()
                },
                AgentEvent::ToolCallFinished {
                    id: "t1".into(),
                    status: ToolStatus::Completed,
                    output_summary: "body".into()
                },
                AgentEvent::ToolCallStarted {
                    id: "t2".into(),
                    name: "bash".into(),
                    input_summary: "{}".into()
                },
                AgentEvent::ToolCallFinished {
                    id: "t2".into(),
                    status: ToolStatus::Failed,
                    output_summary: "denied".into()
                },
                AgentEvent::ToolCallStarted {
                    id: "t3".into(),
                    name: "edit".into(),
                    input_summary: "{}".into()
                },
                AgentEvent::TokenUsage {
                    input: 1,
                    output: 2,
                    cached: 0
                },
            ]
        );
        assert!(s.message(&json!({"type":"system"})).is_empty());
        assert!(s.message(&json!(null)).is_empty());
    }

    #[test]
    fn diffs_and_command_catalog_are_tolerant() {
        let cwd = std::path::Path::new("/w");
        let events = diffs(
            cwd,
            &json!({"data":[
                {"file":"a.rs","status":"added","patch":"+a"},
                {"file":"b.rs","status":"deleted"},
                {"file":"c.rs","status":"modified","patch":"@@"},
                {"file":"d.rs","status":"renamed"},
                {"status":"added"}
            ]}),
        );
        assert_eq!(
            events,
            vec![
                AgentEvent::FileChange {
                    path: "/w/a.rs".into(),
                    kind: FileChangeKind::Created,
                    diff: Some("+a".into())
                },
                AgentEvent::FileChange {
                    path: "/w/b.rs".into(),
                    kind: FileChangeKind::Deleted,
                    diff: None
                },
                AgentEvent::FileChange {
                    path: "/w/c.rs".into(),
                    kind: FileChangeKind::Modified,
                    diff: Some("@@".into())
                },
            ]
        );
        assert!(diffs(cwd, &json!({"error":"x"})).is_empty());
        // A command shadowed by a skill of the same name is listed once, as the skill.
        let rows = commands(
            &json!({"data":[{"name":"review","description":"cmd"},{"name":"init","description":"Init"}]}),
            &json!({"data":[{"id":"sk","name":"review","description":"skill"}]}),
        );
        let rows: Vec<_> = rows
            .iter()
            .map(|c| (c.name.as_str(), c.kind, c.target.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("review", CommandKind::Skill, Some("sk")),
                ("init", CommandKind::Action, None)
            ]
        );
        assert!(models(&json!({}), &json!({})).is_empty());
        let unnamed = models(&json!({"data":[{"id":"m","providerID":"p"}]}), &json!({}));
        assert_eq!(unnamed[0].id, "p/m");
        assert!(!unnamed[0].is_default);
    }
}
