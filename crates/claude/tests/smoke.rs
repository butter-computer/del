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

/// Native catalog and local command path: no model turn or token usage.
#[tokio::test]
#[ignore]
async fn installed_cli_commands_and_goal_status() {
    let cwd = std::env::current_dir().unwrap();
    let backend = ClaudeBackend::new(ClaudeOptions {
        extra_args: vec!["--no-session-persistence".into()],
        ..Default::default()
    });
    let commands = backend.list_commands(&cwd).await.unwrap();
    let goal = commands
        .into_iter()
        .find(|command| command.kind == CommandKind::Goal)
        .expect("native goal command");
    let handle = backend
        .start_session(SessionConfig {
            cwd,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Claude,
            permission_mode: PermissionMode::Ask,
        })
        .await
        .unwrap();
    let mut events = handle.subscribe();
    backend
        .invoke_command(&handle.id, goal, UserInput::text("/goal"))
        .await
        .unwrap();
    let events = next_until(&mut events, |event| {
        matches!(event, AgentEvent::TurnCompleted { .. })
    })
    .await;
    backend.shutdown().await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::GoalUpdated { goal: None }))
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { .. } | AgentEvent::ToolCallStarted { .. }
    )));
}

/// Model discovery owns a fresh CLI process, independent of chat cleanup.
#[tokio::test]
#[ignore = "requires an installed Claude CLI; no model inference"]
async fn model_catalog_starts_without_a_running_chat() {
    let dir = tempfile::tempdir().unwrap();
    let backend = ClaudeBackend::new(ClaudeOptions {
        setting_sources: Some("project".into()),
        ..Default::default()
    });
    for _ in 0..2 {
        let result = backend.list_models(dir.path()).await;
        backend.shutdown().await.unwrap();
        let models = result.expect("model picker must start Claude on demand");
        assert!(!models.is_empty());
        assert!(models.iter().any(|model| model.is_default));
    }
}

/// Exercises only disposable test goals, using the real native transcript.
#[tokio::test]
#[ignore]
async fn real_goal_ui_set_interrupt_restore_clear_and_complete() {
    let dir = std::env::temp_dir().join(format!("del-goal-ui-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = SessionConfig {
        cwd: dir.clone(),
        model: None,
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Claude,
        permission_mode: PermissionMode::Ask,
    };
    let backend = ClaudeBackend::new(ClaudeOptions {
        setting_sources: Some("project".into()),
        extra_args: vec!["--tools".into(), "".into()],
        ..Default::default()
    });
    let result = tokio::time::timeout(Duration::from_secs(55), async {
        let handle = backend.start_session(config.clone()).await.unwrap();
        let mut events = handle.subscribe();
        backend.control_goal(&handle.id, GoalAction::Set("Reply READY, then wait for the user to say BLUE. The goal is only met after the user says BLUE. Do not use tools.".into())).await.unwrap();
        next_until(&mut events, |e| matches!(e, AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Active)).await;
        backend.interrupt(&handle.id).await.unwrap();
        next_until(&mut events, |e| matches!(e, AgentEvent::TurnCompleted { .. })).await;
        backend.close(&handle.id).await.unwrap();
        let resumed = backend.resume(config, &handle.id).await.unwrap();
        let mut events = resumed.subscribe();
        next_until(&mut events, |e| matches!(e, AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Active)).await;
        backend.control_goal(&handle.id, GoalAction::Clear).await.unwrap();
        next_until(&mut events, |e| matches!(e, AgentEvent::GoalUpdated { goal: None })).await;
        next_until(&mut events, |e| matches!(e, AgentEvent::TurnCompleted { .. })).await;
        backend.control_goal(&handle.id, GoalAction::Set("Reply with the single word DONE. This goal is met once you have replied. Do not use tools.".into())).await.unwrap();
        next_until(&mut events, |e| matches!(e, AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Complete)).await;
        backend.control_goal(&handle.id, GoalAction::Clear).await.unwrap();
    }).await;
    backend.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
    result.expect("native goal UI lifecycle timed out");
}

#[tokio::test]
#[ignore]
async fn real_haiku_title_without_thinking() {
    let backend = ClaudeBackend::default();
    let title = backend.generate_title("User: Build a simple chess game with a dark board and legal move highlighting.\nAssistant: I will implement the board and move validation.").await.unwrap().expect("title");
    eprintln!("Haiku title: {title}");
    assert!(title.to_lowercase().contains("chess"));
}
