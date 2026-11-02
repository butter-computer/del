//! Pure notification / server-request mapping for representative app-server
//! messages, including fields and variants del does not know about.
use del_agent::{
    AgentEvent as E, ApprovalKind, ApprovalResponse, FileChangeKind, ToolStatus, TurnStatus,
};
use del_codex::RequestId;
use del_codex::mapping::{ApprovalMethod, Mapper, RequestAction};
use serde_json::{Value, json};

fn item(mapper: &mut Mapper, phase: &str, item: Value) -> Vec<E> {
    mapper.notification(
        &format!("item/{phase}"),
        &json!({"threadId":"t","turnId":"u","item":item}),
    )
}

#[test]
fn command_execution_lifecycle_reports_output_and_exit_code() {
    let mut m = Mapper::default();
    assert_eq!(
        item(
            &mut m,
            "started",
            json!({"type":"commandExecution","id":"c","command":"ls","status":"inProgress","futureField":1})
        ),
        vec![E::ToolCallStarted {
            id: "c".into(),
            name: "shell".into(),
            input_summary: "ls".into()
        }]
    );
    assert_eq!(
        m.notification(
            "item/commandExecution/outputDelta",
            &json!({"threadId":"t","turnId":"u","itemId":"c","delta":"a.txt\n"})
        ),
        vec![E::ToolCallUpdated {
            id: "c".into(),
            output_summary: "a.txt\n".into()
        }]
    );
    assert_eq!(
        item(
            &mut m,
            "completed",
            json!({"type":"commandExecution","id":"c","command":"ls","status":"failed",
                "aggregatedOutput":"nope","exitCode":2})
        ),
        vec![E::ToolCallFinished {
            id: "c".into(),
            status: ToolStatus::Failed,
            output_summary: "nope\n[exit code 2]".into()
        }]
    );
    // A declined command, with null output and a zero exit code.
    assert_eq!(
        item(
            &mut m,
            "completed",
            json!({"type":"commandExecution","id":"d","command":"rm","status":"declined",
                "aggregatedOutput":null,"exitCode":0})
        ),
        vec![E::ToolCallFinished {
            id: "d".into(),
            status: ToolStatus::Declined,
            output_summary: String::new()
        }]
    );
}

#[test]
fn file_changes_are_reported_only_when_applied() {
    let changes = json!([
        {"path":"/w/new.rs","kind":{"type":"add"},"diff":"+x"},
        {"path":"/w/old.rs","kind":{"type":"delete"},"diff":""},
        {"path":"/w/mod.rs","kind":{"type":"update","move_path":null},"diff":"@@"},
        {"path":"/w/odd.rs","kind":{"type":"rename"}}
    ]);
    let mut m = Mapper::default();
    let started = item(
        &mut m,
        "started",
        json!({"type":"fileChange","id":"f","status":"inProgress","changes":changes}),
    );
    let summary = "add /w/new.rs\ndelete /w/old.rs\nupdate /w/mod.rs\nupdate /w/odd.rs";
    assert_eq!(
        started,
        vec![E::ToolCallStarted {
            id: "f".into(),
            name: "apply_patch".into(),
            input_summary: summary.into()
        }]
    );
    // The approval card shows what the item will change.
    let RequestAction::Approval { event, method } = m.server_request(
        &RequestId::Int(9),
        "item/fileChange/requestApproval",
        &json!({"threadId":"t","turnId":"u","itemId":"f","reason":"outside workspace"}),
    ) else {
        panic!("expected approval")
    };
    assert_eq!(
        event,
        E::ApprovalRequested {
            id: "9".into(),
            kind: ApprovalKind::FileChange,
            detail: format!("{summary}\noutside workspace")
        }
    );
    assert_eq!(
        method.result(ApprovalResponse::Approve),
        json!({"decision":"accept"})
    );
    let done = item(
        &mut m,
        "completed",
        json!({"type":"fileChange","id":"f","status":"completed","changes":changes}),
    );
    let kinds: Vec<_> = done
        .iter()
        .filter_map(|e| match e {
            E::FileChange { path, kind, diff } => Some((path.clone(), *kind, diff.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                "/w/new.rs".into(),
                FileChangeKind::Created,
                Some("+x".into())
            ),
            ("/w/old.rs".into(), FileChangeKind::Deleted, None),
            (
                "/w/mod.rs".into(),
                FileChangeKind::Modified,
                Some("@@".into())
            ),
            ("/w/odd.rs".into(), FileChangeKind::Modified, None),
        ]
    );
    assert!(matches!(
        done.last(),
        Some(E::ToolCallFinished {
            status: ToolStatus::Completed,
            ..
        })
    ));
    let declined = item(
        &mut m,
        "completed",
        json!({"type":"fileChange","id":"g","status":"declined","changes":changes}),
    );
    assert!(!declined.iter().any(|e| matches!(e, E::FileChange { .. })));
    assert!(matches!(
        declined[..],
        [E::ToolCallFinished {
            status: ToolStatus::Declined,
            ..
        }]
    ));
}

#[test]
fn mcp_dynamic_and_web_search_tools() {
    let mut m = Mapper::default();
    assert_eq!(
        item(
            &mut m,
            "started",
            json!({"type":"mcpToolCall","id":"m","server":"docs","tool":"search","status":"inProgress","arguments":{"q":"x"}})
        ),
        vec![E::ToolCallStarted {
            id: "m".into(),
            name: "docs/search".into(),
            input_summary: "{\"q\":\"x\"}".into()
        }]
    );
    assert_eq!(
        m.notification(
            "item/mcpToolCall/progress",
            &json!({"threadId":"t","itemId":"m","message":"halfway"})
        ),
        vec![E::ToolCallUpdated {
            id: "m".into(),
            output_summary: "halfway".into()
        }]
    );
    assert_eq!(
        item(
            &mut m,
            "completed",
            json!({"type":"mcpToolCall","id":"m","server":"docs","tool":"search","status":"completed",
                "result":{"content":[{"type":"text","text":"one"},{"type":"image"},{"type":"text","text":"two"}]}})
        ),
        vec![E::ToolCallFinished {
            id: "m".into(),
            status: ToolStatus::Completed,
            output_summary: "one\ntwo".into()
        }]
    );
    assert_eq!(
        item(
            &mut m,
            "completed",
            json!({"type":"mcpToolCall","id":"n","server":"docs","tool":"search","status":"failed",
                "error":{"message":"server down"}})
        ),
        vec![E::ToolCallFinished {
            id: "n".into(),
            status: ToolStatus::Failed,
            output_summary: "server down".into()
        }]
    );
    // A dynamic tool that reports success=false fails even when "completed".
    assert!(matches!(
        &item(
            &mut m,
            "completed",
            json!({"type":"dynamicToolCall","id":"d","tool":"browser","status":"completed","success":false})
        )[..],
        [E::ToolCallFinished {
            status: ToolStatus::Failed,
            ..
        }]
    ));
    assert_eq!(
        item(
            &mut m,
            "started",
            json!({"type":"webSearch","id":"w","query":"rust"})
        ),
        vec![E::ToolCallStarted {
            id: "w".into(),
            name: "web_search".into(),
            input_summary: "rust".into()
        }]
    );
}

#[test]
fn unknown_or_malformed_messages_are_ignored_without_ending_the_turn() {
    let mut m = Mapper::default();
    m.notification("turn/started", &json!({"threadId":"t","turn":{"id":"u"}}));
    for (method, params) in [
        (
            "item/started",
            json!({"turnId":"u","item":{"type":"futureItem","id":"x"}}),
        ),
        (
            "item/completed",
            json!({"turnId":"u","item":{"type":"futureItem","id":"x"}}),
        ),
        // Known type, required field missing.
        (
            "item/started",
            json!({"turnId":"u","item":{"type":"commandExecution","id":"x"}}),
        ),
        (
            "item/completed",
            json!({"item":{"type":"agentMessage","id":"x","text":"no turn id"}}),
        ),
        (
            "item/agentMessage/delta",
            json!({"turnId":"u","delta":"no item id"}),
        ),
        ("thread/tokenUsage/updated", json!({"tokenUsage":{}})),
        ("error", json!({"message":"not an error object"})),
        ("thread/status/changed", json!({"status":"active"})),
        ("account/rateLimits/updated", json!({})),
        (
            "thread/goal/updated",
            json!({"goal":{"objective":"x","status":"future"}}),
        ),
        ("turn/completed", json!({"turn":"not an object"})),
        ("totally/new", Value::Null),
    ] {
        assert!(
            m.notification(method, &params).is_empty(),
            "{method} {params}"
        );
    }
    assert_eq!(m.turn_id(), Some("u"));
    // Unknown statuses fall back to Completed rather than failing decoding.
    assert_eq!(
        m.notification(
            "turn/completed",
            &json!({"turn":{"id":"u","status":"somethingNew"}})
        ),
        vec![E::TurnCompleted {
            turn_id: "u".into(),
            status: TurnStatus::Completed
        }]
    );
    assert_eq!(m.turn_id(), None);
}

#[test]
fn streaming_usage_errors_and_goals() {
    let mut m = Mapper::default();
    assert_eq!(
        m.notification(
            "item/reasoning/summaryTextDelta",
            &json!({"threadId":"t","turnId":"u","itemId":"r","delta":"think","summaryIndex":0})
        ),
        vec![E::ReasoningDelta {
            turn_id: "u".into(),
            text: "think".into()
        }]
    );
    assert_eq!(
        m.notification(
            "thread/tokenUsage/updated",
            &json!({"threadId":"t","turnId":"u","tokenUsage":{
                "total":{"inputTokens":10,"outputTokens":2,"cachedInputTokens":4,"reasoningOutputTokens":1},
                "last":{"inputTokens":1,"outputTokens":1,"cachedInputTokens":0}}})
        ),
        vec![E::UsageSnapshot { context_tokens: Some(2), context_window: None, cost_scope: None, cost_usd: None }, E::TokenUsage {
            input: 10,
            output: 2,
            cached: 4
        }]
    );
    assert_eq!(
        m.notification(
            "error",
            &json!({"threadId":"t","turnId":"u","error":{"message":"retrying"},"willRetry":true})
        ),
        vec![E::Error {
            message: "retrying".into(),
            recoverable: true
        }]
    );
    assert!(matches!(
        &m.notification(
            "thread/goal/updated",
            &json!({"threadId":"t","goal":{"objective":"ship","status":"paused","tokensUsed":5}})
        )[..],
        [E::GoalUpdated { goal: Some(goal) }] if goal.objective == "ship" && goal.tokens_used == 5
    ));
    assert_eq!(
        m.notification("thread/goal/cleared", &json!({"threadId":"t"})),
        vec![E::GoalUpdated { goal: None }]
    );
}

#[test]
fn approvals_answer_with_the_wire_shape_codex_expects() {
    let mut m = Mapper::default();
    let RequestAction::Approval { event, method } = m.server_request(
        &RequestId::Str("srv-1".into()),
        "item/commandExecution/requestApproval",
        &json!({"threadId":"t","itemId":"c","command":"git push","reason":"network"}),
    ) else {
        panic!("expected approval")
    };
    assert_eq!(
        event,
        E::ApprovalRequested {
            id: "srv-1".into(),
            kind: ApprovalKind::Command,
            detail: "git push\nnetwork".into()
        }
    );
    assert_eq!(method, ApprovalMethod::Command);
    assert_eq!(
        method.result(ApprovalResponse::Deny),
        json!({"decision":"decline"})
    );
    // Missing command text still produces a card.
    let RequestAction::Approval { event, .. } = m.server_request(
        &RequestId::Int(2),
        "item/commandExecution/requestApproval",
        &json!({"threadId":"t"}),
    ) else {
        panic!("expected approval")
    };
    assert!(matches!(event, E::ApprovalRequested { detail, .. } if detail == "(unknown command)"));
    // Permission grants are scoped to the turn; a denial grants nothing.
    let requested = json!({"network":{"enabled":true}});
    let RequestAction::Approval { method, .. } = m.server_request(
        &RequestId::Int(3),
        "item/permissions/requestApproval",
        &json!({"threadId":"t","permissions":requested,"reason":"fetch"}),
    ) else {
        panic!("expected approval")
    };
    assert_eq!(
        method.result(ApprovalResponse::Approve),
        json!({"permissions":requested,"scope":"turn"})
    );
    assert_eq!(
        method.result(ApprovalResponse::Deny),
        json!({"permissions":{},"scope":"turn"})
    );
}

#[test]
fn history_restores_user_input_reasoning_and_items_once() {
    let turns = vec![
        json!({"id":"u1","status":"completed","items":[
            {"type":"userMessage","id":"i1","content":[
                {"type":"text","text":"look"},{"type":"localImage","path":"/a.png"},
                {"type":"image","url":"data:"},{"type":"mention","name":"x"}]},
            {"type":"reasoning","id":"r","summary":["first"],"content":["second"]},
            {"type":"reasoning","id":"r2","summary":[],"content":[]},
            {"type":"commandExecution","id":"c","command":"ls","status":"completed","aggregatedOutput":"a"},
            {"type":"agentMessage","id":"a","text":"done"},
            {"type":"contextCompaction","id":"z"}
        ]}),
        json!({"status":"completed","items":[]}),
    ];
    let events = Mapper::default().history(&turns);
    assert_eq!(
        events,
        vec![
            E::TurnStarted {
                turn_id: "u1".into()
            },
            E::UserMessage {
                text: "look\n[attached image: /a.png]\n[attached image]".into()
            },
            E::ReasoningDelta {
                turn_id: "u1".into(),
                text: "first\nsecond".into()
            },
            E::ToolCallStarted {
                id: "c".into(),
                name: "shell".into(),
                input_summary: "ls".into()
            },
            E::ToolCallFinished {
                id: "c".into(),
                status: ToolStatus::Completed,
                output_summary: "a".into()
            },
            E::AssistantMessage {
                turn_id: "u1".into(),
                text: "done".into()
            },
            E::TurnCompleted {
                turn_id: "u1".into(),
                status: TurnStatus::Completed
            },
        ],
        "turns without an id are skipped"
    );
}

#[test]
fn turn_start_response_after_completion_does_not_reopen_the_turn() {
    let mut m = Mapper::default();
    m.notification("turn/started", &json!({"turn":{"id":"u"}}));
    m.notification(
        "turn/completed",
        &json!({"turn":{"id":"u","status":"completed"}}),
    );
    m.note_turn("u");
    assert_eq!(
        m.turn_id(),
        None,
        "a late turn/start reply must not mark it busy"
    );
    m.note_turn("v");
    assert_eq!(m.turn_id(), Some("v"));
}

#[test]
fn large_multibyte_output_is_truncated_on_a_character_boundary() {
    let mut m = Mapper::default();
    let output = "é".repeat(5000);
    let events = item(
        &mut m,
        "completed",
        json!({"type":"commandExecution","id":"c","command":"cat","status":"completed","aggregatedOutput":output}),
    );
    let E::ToolCallFinished { output_summary, .. } = &events[0] else {
        panic!("{events:?}")
    };
    assert!(output_summary.ends_with("… [truncated]"));
    assert!(output_summary.len() <= 4096 + "… [truncated]".len());
}

#[test]
fn context_uses_last_request_not_cumulative_or_double_counted_cache() {
    let events = Mapper::default().notification(
        "thread/tokenUsage/updated",
        &json!({"tokenUsage": {
            "total": {"inputTokens": 900000, "outputTokens": 30000},
            "last": {"inputTokens": 50000, "outputTokens": 1000, "cachedInputTokens": 40000},
            "modelContextWindow": 200000
        }}),
    );
    assert!(matches!(
        events[0],
        E::UsageSnapshot {
            context_tokens: Some(51000),
            context_window: Some(200000),
            cost_usd: None,
            ..
        }
    ));
}

#[test]
fn cumulative_cost_estimate_uses_cached_rates_and_stable_thread_scope() {
    let mut mapper = Mapper::default();
    mapper.set_pricing("thread-a", Some("gpt-5.3-codex"));
    let data = json!({"tokenUsage":{"total":{"inputTokens":2000000,"cachedInputTokens":1000000,"outputTokens":1000000}}});
    for _ in 0..2 {
        let events = mapper.notification("thread/tokenUsage/updated", &data);
        assert!(events.iter().any(|e| matches!(e, E::UsageSnapshot {cost_scope:Some(scope), cost_usd:Some(cost), ..} if scope == &del_agent::pricing::cost_scope(Some("thread-a"), "estimate:codex:thread-a".into()) && (*cost - 15.925).abs() < 1e-9)));
    }
}
