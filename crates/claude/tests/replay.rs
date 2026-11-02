//! Replays transcripts recorded from a real `claude` 2.1.308 run through the translator.

use del_agent::{
    AgentEvent as E, ApprovalKind, ApprovalResponse, FileChangeKind, ToolStatus, TurnStatus,
};
use del_claude::Translator;

fn replay(name: &str, setup: impl FnOnce(&mut Translator) -> Vec<E>) -> Vec<E> {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut t = Translator::new();
    let mut out = setup(&mut t);
    for l in raw.lines() {
        out.extend(t.handle_line(l));
    }
    out
}

fn start(t: &mut Translator) -> Vec<E> {
    t.enqueue_turn("t1".into())
}

#[test]
fn edit_transcript_maps_tools_approvals_and_file_changes() {
    let evs = replay("edit", start);
    assert!(matches!(evs[0], E::TurnStarted { .. }));
    let approvals: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            E::ApprovalRequested { kind, detail, .. } => Some((*kind, detail.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        approvals,
        vec![
            (ApprovalKind::FileChange, "/work/b.txt".to_string()),
            (ApprovalKind::FileChange, "/work/b.txt".to_string()),
        ]
    );
    let changes: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            E::FileChange { kind, diff, .. } => Some((*kind, diff.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[0],
        (FileChangeKind::Created, Some("+hello\n".into()))
    );
    assert_eq!(changes[1].0, FileChangeKind::Modified);
    assert!(changes[1].1.as_ref().unwrap().contains("-hello\n+world"));
    let started: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            E::ToolCallStarted {
                name,
                input_summary,
                ..
            } => Some((name.as_str(), input_summary.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            ("Write", "/work/b.txt"),
            ("Edit", "/work/b.txt"),
            ("Bash", "ls")
        ]
    );
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(
                e,
                E::ToolCallFinished {
                    status: ToolStatus::Completed,
                    ..
                }
            ))
            .count(),
        3
    );
    assert!(
        evs.iter()
            .any(|e| matches!(e, E::AssistantTextDelta { .. }))
    );
    assert!(
        evs.iter()
            .any(|e| matches!(e, E::AssistantMessage { text, .. } if text.contains("world")))
    );
    // The replay never answers the prompts, so the result retires both.
    let n = evs.len();
    assert!(matches!(evs[n - 4], E::TokenUsage { output, .. } if output > 0));
    assert!(
        evs[n - 3..n - 1]
            .iter()
            .all(|e| matches!(e, E::ApprovalResolved { .. }))
    );
    assert_eq!(
        evs[n - 1],
        E::TurnCompleted {
            turn_id: "t1".into(),
            status: TurnStatus::Completed
        }
    );
}

#[test]
fn denied_write_is_declined_without_file_change() {
    let evs = replay("deny", start);
    assert!(evs.iter().any(|e| matches!(
        e,
        E::ToolCallFinished {
            status: ToolStatus::Declined,
            ..
        }
    )));
    assert!(!evs.iter().any(|e| matches!(e, E::FileChange { .. })));
    assert!(matches!(
        evs.last(),
        Some(E::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
}

#[test]
fn interrupt_maps_to_interrupted_turn() {
    let evs = replay("interrupt", start);
    assert!(
        evs.iter()
            .any(|e| matches!(e, E::AssistantTextDelta { .. }))
    );
    assert!(matches!(
        evs.last(),
        Some(E::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    // The aborted result is flagged by terminal_reason even without our interrupt flag.
    assert!(!evs.iter().any(|e| matches!(e, E::Error { .. })));
}

#[test]
fn queued_follow_up_becomes_second_turn() {
    // steer.jsonl: a second user message was sent mid-turn; the CLI ran two results.
    let evs = replay("steer", |t| {
        let mut v = t.enqueue_turn("t1".into());
        v.extend(t.enqueue_turn("t2".into())); // queued: no TurnStarted yet
        v
    });
    let seq: Vec<String> = evs
        .iter()
        .filter_map(|e| match e {
            E::TurnStarted { turn_id } => Some(format!("start {turn_id}")),
            E::TurnCompleted { turn_id, status } => Some(format!("done {turn_id} {status:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        seq,
        [
            "start t1",
            "done t1 Completed",
            "start t2",
            "done t2 Completed"
        ]
    );
}

#[test]
fn approval_reply_echoes_input_and_is_single_use() {
    let mut t = Translator::new();
    let req = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm -rf x"},"tool_use_id":"tu"}}"#;
    let evs = t.handle_line(req);
    assert_eq!(
        evs,
        vec![E::ApprovalRequested {
            id: "r1".into(),
            kind: ApprovalKind::Command,
            detail: "rm -rf x".into()
        }]
    );
    let ok: serde_json::Value =
        serde_json::from_str(&t.approval_reply("r1", ApprovalResponse::Approve).unwrap()).unwrap();
    assert_eq!(ok["response"]["request_id"], "r1");
    assert_eq!(ok["response"]["response"]["behavior"], "allow");
    assert_eq!(
        ok["response"]["response"]["updatedInput"]["command"],
        "rm -rf x"
    );
    assert!(t.approval_reply("r1", ApprovalResponse::Deny).is_none());
}

#[test]
fn process_death_fails_the_running_turn() {
    let mut t = Translator::new();
    t.enqueue_turn("t1".into());
    assert_eq!(
        t.abort(),
        vec![E::TurnCompleted {
            turn_id: "t1".into(),
            status: TurnStatus::Failed
        }]
    );
    assert!(t.abort().is_empty());
}

#[test]
fn garbage_lines_are_ignored() {
    let mut t = Translator::new();
    assert!(t.handle_line("not json").is_empty());
    assert!(t.handle_line(r#"{"type":"rate_limit_event"}"#).is_empty());
}

#[test]
fn native_goal_continuations_get_distinct_turns_and_can_be_interrupted() {
    let mut translator = Translator::new();
    for _ in 0..2 {
        let events = translator.handle_line(
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"native"}}}"#,
        );
        assert!(matches!(&events[..], [E::TurnStarted { turn_id }] if !turn_id.is_empty()));
        assert!(translator.is_busy());
        translator.mark_interrupting();
        let events = translator.handle_line(r#"{"type":"result","is_error":false,"usage":{}}"#);
        assert!(events.iter().any(|event| matches!(event, E::TurnCompleted { status: TurnStatus::Interrupted, turn_id } if !turn_id.is_empty())));
        assert!(!translator.is_busy());
    }
}

#[test]
fn steering_consumed_in_current_response_does_not_leave_phantom_turn() {
    let mut translator = Translator::new();
    translator.enqueue_turn("first".into());
    translator.enqueue_turn("steering".into());
    for text in ["first", "steering"] {
        translator.handle_line(
            &serde_json::json!({
                "type": "user", "isReplay": true,
                "message": {"role": "user", "content": text}
            })
            .to_string(),
        );
    }
    let events = translator.handle_line(r#"{"type":"result","is_error":false}"#);
    assert!(
        !translator.is_busy(),
        "both inputs were consumed by the completed response"
    );
    assert!(!events.iter().any(|e| matches!(e, E::TurnStarted { .. })));
}

fn line(t: &mut Translator, value: serde_json::Value) -> Vec<E> {
    t.handle_line(&value.to_string())
}

#[test]
fn failed_result_reports_its_error_and_usage_counts_cache_creation() {
    let mut t = Translator::new();
    t.enqueue_turn("t".into());
    let evs = line(
        &mut t,
        serde_json::json!({"type":"result","is_error":true,"result":"Prompt is too long",
            "usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}),
    );
    assert_eq!(
        evs,
        vec![
            E::TokenUsage {
                input: 3,
                output: 4,
                cached: 3
            },
            E::Error {
                message: "Prompt is too long".into(),
                recoverable: true
            },
            E::TurnCompleted {
                turn_id: "t".into(),
                status: TurnStatus::Failed
            },
        ]
    );
    // Without a result string the error list is joined; missing usage is zero.
    t.enqueue_turn("u".into());
    let evs = line(
        &mut t,
        serde_json::json!({"type":"result","is_error":true,"errors":["a",7,"b"]}),
    );
    assert_eq!(
        evs[0],
        E::TokenUsage {
            input: 0,
            output: 0,
            cached: 0
        }
    );
    assert_eq!(
        evs[1],
        E::Error {
            message: "a; b".into(),
            recoverable: true
        }
    );
}

#[test]
fn interrupt_wins_over_an_error_result() {
    let mut t = Translator::new();
    t.enqueue_turn("t".into());
    t.mark_interrupting();
    let evs = line(
        &mut t,
        serde_json::json!({"type":"result","is_error":true,"result":"aborted"}),
    );
    assert!(!evs.iter().any(|e| matches!(e, E::Error { .. })));
    assert_eq!(
        evs.last(),
        Some(&E::TurnCompleted {
            turn_id: "t".into(),
            status: TurnStatus::Interrupted
        })
    );
    // The flag is per turn.
    t.enqueue_turn("u".into());
    let evs = line(
        &mut t,
        serde_json::json!({"type":"result","is_error":false}),
    );
    assert_eq!(
        evs.last(),
        Some(&E::TurnCompleted {
            turn_id: "u".into(),
            status: TurnStatus::Completed
        })
    );
}

#[test]
fn approval_kinds_details_and_deny_reply() {
    let mut t = Translator::new();
    t.enqueue_turn("t".into());
    let ask = |t: &mut Translator, id: &str, tool: &str, input: serde_json::Value| {
        line(
            t,
            serde_json::json!({"type":"control_request","request_id":id,
                "request":{"subtype":"can_use_tool","tool_name":tool,"input":input}}),
        )
    };
    assert_eq!(
        ask(
            &mut t,
            "1",
            "Edit",
            serde_json::json!({"file_path":"/w/a.rs","old_string":"a","new_string":"b"})
        ),
        vec![E::ApprovalRequested {
            id: "1".into(),
            kind: ApprovalKind::FileChange,
            detail: "/w/a.rs".into()
        }]
    );
    assert_eq!(
        ask(&mut t, "2", "mcp__server__tool", serde_json::json!({"q":1})),
        vec![E::ApprovalRequested {
            id: "2".into(),
            kind: ApprovalKind::Other,
            detail: "mcp__server__tool: {\"q\":1}".into()
        }]
    );
    let deny: serde_json::Value =
        serde_json::from_str(&t.approval_reply("1", ApprovalResponse::Deny).unwrap()).unwrap();
    assert_eq!(
        deny,
        serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":"1",
            "response":{"behavior":"deny","message":"Denied by the user in del"}}})
    );
    // Other control requests (and malformed ones) raise no prompt.
    assert!(
        line(
            &mut t,
            serde_json::json!({"type":"control_request","request_id":"3","request":{"subtype":"hook_callback"}})
        )
        .is_empty()
    );
    assert!(
        line(
            &mut t,
            serde_json::json!({"type":"control_request","request":{"subtype":"can_use_tool"}})
        )
        .is_empty()
    );
}

#[test]
fn tool_results_map_status_output_and_edit_diffs() {
    let mut t = Translator::new();
    t.enqueue_turn("t".into());
    line(
        &mut t,
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"e","name":"Edit","input":{"file_path":"/w/a.rs","old_string":"old","new_string":"new"}},
            {"type":"tool_use","id":"p","name":"Edit","input":{"file_path":"/w/b.rs"}},
            {"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/w/c.rs"}},
            {"type":"tool_use","name":"Broken"}
        ]}}),
    );
    let evs = line(
        &mut t,
        serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"e","content":"ok"},
            {"type":"tool_result","tool_use_id":"r","content":[{"type":"text","text":"x".repeat(600)}]}
        ]}}),
    );
    assert_eq!(
        evs[1],
        E::FileChange {
            path: "/w/a.rs".into(),
            kind: FileChangeKind::Modified,
            diff: Some("-old\n+new\n".into())
        }
    );
    let E::ToolCallFinished { output_summary, .. } = &evs[2] else {
        panic!("{evs:?}")
    };
    assert_eq!(
        output_summary.chars().count(),
        501,
        "500 chars and an ellipsis"
    );
    assert!(output_summary.ends_with('…'));
    // structuredPatch hunks win over the input-derived diff.
    let evs = line(
        &mut t,
        serde_json::json!({"type":"user",
            "tool_use_result":{"structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-a","+b"]}]},
            "message":{"content":[{"type":"tool_result","tool_use_id":"p","content":"ok"}]}}),
    );
    assert_eq!(
        evs[1],
        E::FileChange {
            path: "/w/b.rs".into(),
            kind: FileChangeKind::Modified,
            diff: Some("@@ -1,1 +1,1 @@\n-a\n+b\n".into())
        }
    );
}

#[test]
fn malformed_and_unknown_records_are_tolerated() {
    let mut t = Translator::new();
    t.enqueue_turn("t".into());
    for raw in [
        "",
        "   ",
        "[1,2]",
        "42",
        "null",
        "{\"type\":7}",
        "{\"type\":\"assistant\"}",
        "{\"type\":\"assistant\",\"message\":{\"content\":\"text, not blocks\"}}",
        "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}",
        "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"\"}}}",
        "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\"}}}",
        "{\"type\":\"control_cancel_request\"}",
        "{\"type\":\"system\",\"subtype\":\"hook_started\"}",
        "{\"type\":\"result\"",
    ] {
        assert!(t.handle_line(raw).is_empty(), "{raw:?}");
    }
    assert!(t.is_busy(), "noise does not end the turn");
    let evs = line(
        &mut t,
        serde_json::json!({"type":"system","subtype":"init","session_id":"native-1"}),
    );
    assert!(evs.is_empty());
    assert_eq!(t.session_id(), Some("native-1"));
}

#[test]
fn native_goal_acknowledgements_and_status_records() {
    let mut t = Translator::new();
    let ack = |text: &str| {
        serde_json::json!({"type":"user","local_command_run":{"command":"goal"},
            "message":{"content":[{"type":"text","text":text}]}})
    };
    assert!(matches!(
        &line(&mut t, ack("Goal set: ship it"))[..],
        [E::GoalUpdated { goal: Some(goal) }] if goal.objective == "ship it"
    ));
    assert_eq!(
        line(&mut t, ack("Goal cleared: ship it")),
        vec![E::GoalUpdated { goal: None }]
    );
    assert_eq!(
        line(&mut t, ack("No goal set")),
        vec![E::GoalUpdated { goal: None }]
    );
    // Prose quoting the acknowledgement is not a goal record.
    assert!(
        !line(
            &mut t,
            serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":"Goal set: fake"}]}})
        )
        .iter()
        .any(|e| matches!(e, E::GoalUpdated { .. }))
    );
}
