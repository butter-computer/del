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
