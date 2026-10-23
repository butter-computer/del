//! Real-CLI smoke test. Needs an installed, logged-in `claude`; run with
//! `cargo test -p del-claude --test smoke -- --ignored --nocapture`.

use std::time::Duration;

use del_agent::*;
use del_claude::{ClaudeBackend, ClaudeOptions, auth_status};

async fn next_until(
    rx: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
    pred: impl Fn(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let mut seen = vec![];
    loop {
        let e = tokio::time::timeout(Duration::from_secs(90), rx.recv())
            .await
            .expect("timeout")
            .unwrap();
        let done = pred(&e);
        seen.push(e);
        if done {
            return seen;
        }
    }
}

#[tokio::test]
#[ignore]
async fn real_cli_prompt_approve_interrupt_resume() {
    let opts = ClaudeOptions {
        setting_sources: Some("project".into()),
        ..Default::default()
    };
    assert!(
        auth_status(&opts).await.unwrap().logged_in,
        "claude is not logged in"
    );
    let dir = std::env::temp_dir().join(format!("del-claude-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("haiku".into()),
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Claude,
        permission_mode: PermissionMode::Ask,
    };
    let be = ClaudeBackend::new(opts.clone());
    let h = be.start_session(cfg.clone()).await.unwrap();
    let mut rx = h.subscribe();

    // Prompt -> approval request -> approve -> file written -> turn completes.
    be.send_prompt(
        &h.id,
        UserInput::text("Use the Write tool to create hello.txt containing 'hi'. Reply 'ok'."),
    )
    .await
    .unwrap();
    let evs = next_until(&mut rx, |e| {
        matches!(e, AgentEvent::ApprovalRequested { .. })
    })
    .await;
    let AgentEvent::ApprovalRequested { id, kind, .. } = evs.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(kind, ApprovalKind::FileChange);
    be.respond_approval(&h.id, &id, ApprovalResponse::Approve)
        .await
        .unwrap();
    let evs = next_until(&mut rx, |e| matches!(e, AgentEvent::TurnCompleted { .. })).await;
    assert!(evs.iter().any(|e| matches!(
        e,
        AgentEvent::FileChange {
            kind: FileChangeKind::Created,
            ..
        }
    )));
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("hello.txt"))
            .unwrap()
            .trim(),
        "hi"
    );

    // Interrupt mid-stream.
    be.send_prompt(
        &h.id,
        UserInput::text("Count from 1 to 300, one number per line, no tools."),
    )
    .await
    .unwrap();
    next_until(&mut rx, |e| {
        matches!(e, AgentEvent::AssistantTextDelta { .. })
    })
    .await;
    be.interrupt(&h.id).await.unwrap();
    let evs = next_until(&mut rx, |e| matches!(e, AgentEvent::TurnCompleted { .. })).await;
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));

    // Resume in a fresh backend (new process) and recall earlier context.
    be.shutdown().await.unwrap();
    let be2 = ClaudeBackend::new(opts);
    let h2 = be2.resume(cfg, &h.id).await.unwrap();
    assert!(h2.history().iter().any(|event| matches!(event,
        AgentEvent::UserMessage { text } if text.contains("hello.txt"))));
    assert!(
        h2.history()
            .iter()
            .any(|event| matches!(event, AgentEvent::AssistantMessage { .. }))
    );
    let mut rx2 = h2.subscribe();
    be2.send_prompt(
        &h2.id,
        UserInput::text(
            "What file did I ask you to create earlier? Answer with just the filename.",
        ),
    )
    .await
    .unwrap();
    let evs = next_until(&mut rx2, |e| matches!(e, AgentEvent::TurnCompleted { .. })).await;
    let text: String = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AssistantMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("hello.txt"), "resume lost context: {text}");
    be2.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
