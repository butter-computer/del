//! Smoke tests against the real `codex` CLI. Ignored by default because they
//! need the binary and (except the not-logged-in one) a logged-in account and
//! spend model tokens:  `cargo test -p del-codex --test e2e -- --ignored --test-threads=1`

use del_agent::*;
use del_codex::{AuthStatus, CodexBackend, CodexError, CodexOptions, SpawnOptions};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

fn config(cwd: &std::path::Path, mode: PermissionMode) -> SessionConfig {
    SessionConfig {
        cwd: cwd.into(),
        model: None,
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Codex,
        permission_mode: mode,
    }
}

async fn collect_until<F: Fn(&AgentEvent) -> bool>(
    rx: &mut Receiver<AgentEvent>,
    stop: F,
) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    loop {
        let e = tokio::time::timeout(Duration::from_secs(120), rx.recv())
            .await
            .expect("timed out")
            .expect("stream closed");
        let done = stop(&e);
        out.push(e);
        if done {
            return out;
        }
    }
}

fn is_end(e: &AgentEvent) -> bool {
    matches!(e, AgentEvent::TurnCompleted { .. })
}

/// Restores an existing local transcript without sending a model prompt.
#[tokio::test]
#[ignore]
async fn saved_thread_restores_messages() {
    let id = SessionId::new(std::env::var("DEL_TEST_SESSION_ID").expect("set DEL_TEST_SESSION_ID"));
    let index =
        del_agent::index::SessionIndex::open(del_agent::index::default_index_path().unwrap())
            .unwrap();
    let record = index.get(&id).expect("saved session exists");
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let handle = backend.resume(record.config(), &id).await.unwrap();
    let users = handle
        .history()
        .iter()
        .filter(|e| matches!(e, AgentEvent::UserMessage { .. }))
        .count();
    let assistants = handle
        .history()
        .iter()
        .filter(|e| matches!(e, AgentEvent::AssistantMessage { .. }))
        .count();
    backend.shutdown().await.unwrap();
    assert!(users > 0, "saved user messages missing");
    assert!(assistants > 0, "saved assistant replies missing");
    eprintln!("Restored {users} user messages and {assistants} assistant messages");
}

/// Read-only startup probe: no model turn and no token usage.
#[tokio::test]
#[ignore]
async fn installed_cli_returns_model_catalog() {
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let result = backend.list_models(&std::env::current_dir().unwrap()).await;
    backend.shutdown().await.unwrap();
    let models = result.expect("installed Codex must return its model catalog");
    assert!(!models.is_empty());
    assert!(models.iter().any(|model| model.is_default));
}

/// A stopped idle server must be started on demand without a manual refresh.
#[tokio::test]
#[ignore = "requires an installed and authenticated Codex CLI"]
async fn model_catalog_restarts_after_last_chat_closes() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let session = backend
        .start_session(config(dir.path(), PermissionMode::Plan))
        .await
        .unwrap();
    let original = backend.session_process(&session.id).await.unwrap().0;
    backend.close(&session.id).await.unwrap();
    assert!(backend.session_process(&session.id).await.is_none());

    let result = backend.list_models(dir.path()).await;
    let replacement = backend.session_process(&session.id).await;
    backend.shutdown().await.unwrap();
    let models = result.expect("model picker must start Codex automatically after idle shutdown");
    assert!(!models.is_empty());
    assert!(models.iter().any(|model| model.is_default));
    assert_ne!(replacement.unwrap().0, original);
}

#[tokio::test]
#[ignore]
async fn real_turn_streams_and_completes() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let status = backend
        .check_auth()
        .await
        .expect("codex must be installed and logged in");
    eprintln!("auth: {status:?}");
    let h = backend
        .start_session(config(dir.path(), PermissionMode::Plan))
        .await
        .unwrap();
    let mut rx = h.subscribe();
    backend
        .send_prompt(
            &h.id,
            UserInput::text("Reply with exactly the word: pong. Do not run any tools."),
        )
        .await
        .unwrap();
    let evs = collect_until(&mut rx, is_end).await;
    eprintln!("{evs:#?}");
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::TurnStarted { .. }))
    );
    assert!(evs.iter().any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text.to_lowercase().contains("pong"))));
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::TokenUsage { input, .. } if *input > 0))
    );
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));

    // A second turn on the same thread, then resume from a fresh backend.
    backend
        .send_prompt(
            &h.id,
            UserInput::text("What word did I ask you to say? One word."),
        )
        .await
        .unwrap();
    let evs = collect_until(&mut rx, is_end).await;
    assert!(evs.iter().any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text.to_lowercase().contains("pong"))));
    backend.close(&h.id).await.unwrap();
    // Simulates an app restart: the old process is gone before we resume.
    backend.shutdown().await.unwrap();

    let backend2 = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let h2 = backend2
        .resume(config(dir.path(), PermissionMode::Plan), &h.id)
        .await
        .unwrap();
    assert_eq!(h2.id, h.id);
    let mut rx2 = h2.subscribe();
    backend2
        .send_prompt(
            &h2.id,
            UserInput::text("Repeat my first request's word again. One word."),
        )
        .await
        .unwrap();
    let evs = collect_until(&mut rx2, is_end).await;
    assert!(evs.iter().any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text.to_lowercase().contains("pong"))), "{evs:?}");
    backend2.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_approval_flow_and_file_change() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let h = backend
        .start_session(config(dir.path(), PermissionMode::Ask))
        .await
        .unwrap();
    let mut rx = h.subscribe();
    backend
        .send_prompt(
            &h.id,
            UserInput::text(
                "Create a file hello.txt containing the text hi using apply_patch, then say done.",
            ),
        )
        .await
        .unwrap();
    let mut approvals = 0;
    let mut seen = Vec::new();
    loop {
        let e = collect_until(&mut rx, |_| true).await.remove(0);
        if let AgentEvent::ApprovalRequested { id, .. } = &e {
            approvals += 1;
            backend
                .respond_approval(&h.id, id, ApprovalResponse::Approve)
                .await
                .unwrap();
        }
        let end = is_end(&e);
        seen.push(e);
        if end {
            break;
        }
    }
    eprintln!("{seen:#?}");
    assert!(approvals >= 1);
    assert!(seen.iter().any(|e| matches!(
        e,
        AgentEvent::FileChange {
            kind: FileChangeKind::Created,
            ..
        }
    )));
    assert!(dir.path().join("hello.txt").exists());

    // Deny path: the agent must not get its file.
    backend
        .send_prompt(
            &h.id,
            UserInput::text("Create another file nope.txt containing no using apply_patch."),
        )
        .await
        .unwrap();
    loop {
        let e = collect_until(&mut rx, |_| true).await.remove(0);
        if let AgentEvent::ApprovalRequested { id, .. } = &e {
            backend
                .respond_approval(&h.id, id, ApprovalResponse::Deny)
                .await
                .unwrap();
        }
        if is_end(&e) {
            break;
        }
    }
    assert!(!dir.path().join("nope.txt").exists());
    backend.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn real_interrupt_and_steer() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let h = backend
        .start_session(config(dir.path(), PermissionMode::FullAuto))
        .await
        .unwrap();
    let mut rx = h.subscribe();

    // Interrupt a long-running command.
    backend
        .send_prompt(
            &h.id,
            UserInput::text("Run the shell command `sleep 60` and wait for it."),
        )
        .await
        .unwrap();
    collect_until(&mut rx, |e| matches!(e, AgentEvent::ToolCallStarted { .. })).await;
    backend.interrupt(&h.id).await.unwrap();
    let evs = collect_until(&mut rx, is_end).await;
    assert!(
        matches!(
            evs.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            })
        ),
        "{evs:?}"
    );

    // Steer a running turn.
    backend
        .send_prompt(
            &h.id,
            UserInput::text("Run the shell command `sleep 20` then reply with the word FIRST."),
        )
        .await
        .unwrap();
    collect_until(&mut rx, |e| matches!(e, AgentEvent::ToolCallStarted { .. })).await;
    backend
        .steer(
            &h.id,
            "Change of plan: skip waiting, reply with the single word STEERED and nothing else."
                .into(),
        )
        .await
        .unwrap();
    let evs = collect_until(&mut rx, is_end).await;
    eprintln!("{evs:#?}");
    assert!(matches!(evs.last(), Some(AgentEvent::TurnCompleted { .. })));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn not_logged_in_with_empty_codex_home() {
    let home = tempfile::tempdir().unwrap();
    let mut spawn = SpawnOptions::default();
    spawn
        .env
        .push(("CODEX_HOME".into(), home.path().display().to_string()));
    let backend = CodexBackend::new(CodexOptions {
        spawn,
        api_key: None,
        ..Default::default()
    });
    let err = backend.check_auth().await.unwrap_err();
    assert!(matches!(err, CodexError::NotLoggedIn), "{err:?}");
    println!("{err}");
    backend.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn api_key_mode_with_isolated_codex_home() {
    let home = tempfile::tempdir().unwrap();
    let mut spawn = SpawnOptions::default();
    spawn
        .env
        .push(("CODEX_HOME".into(), home.path().display().to_string()));
    let backend = CodexBackend::new(CodexOptions {
        spawn,
        api_key: Some("sk-bogus-for-test".into()),
        ..Default::default()
    });
    let status = backend.check_auth().await;
    eprintln!("{status:?}");
    assert!(matches!(
        status,
        Ok(AuthStatus::ApiKey) | Err(CodexError::ApiKeyRejected(_))
    ));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn missing_binary_reports_not_installed() {
    let spawn = SpawnOptions {
        program: "/nonexistent/codex".into(),
        ..Default::default()
    };
    let backend = CodexBackend::new(CodexOptions {
        spawn,
        api_key: None,
        ..Default::default()
    });
    let err = backend
        .start_session(config(std::path::Path::new("/tmp"), PermissionMode::Ask))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("was not found"), "{err}");
}

/// Native catalog and local goal status: no model turn or token usage.
#[tokio::test]
#[ignore]
async fn installed_cli_commands_and_goal_status() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let commands = backend.list_commands(dir.path()).await.unwrap();
    let goal = commands
        .into_iter()
        .find(|command| command.kind == CommandKind::Goal)
        .expect("native goals");
    let handle = backend
        .start_session(config(dir.path(), PermissionMode::Ask))
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let result = backend
        .invoke_command(&handle.id, goal, UserInput::text("/goal"))
        .await;
    if let Err(error) = result {
        backend.shutdown().await.unwrap();
        panic!("goal command failed: {error}");
    }
    let events = collect_until(&mut events, is_end).await;
    backend.shutdown().await.unwrap();
    assert!(events.iter().any(|event| matches!(event, AgentEvent::AssistantMessage { text, .. } if text.contains("No goal set"))));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { .. } | AgentEvent::ToolCallStarted { .. }
    )));
}

/// Bounded live verification of the harness's own goal loop.
#[tokio::test]
#[ignore]
async fn real_native_goal_starts_work_and_can_be_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let commands = backend.list_commands(dir.path()).await.unwrap();
    let goal = commands
        .into_iter()
        .find(|command| command.kind == CommandKind::Goal)
        .unwrap();
    let handle = backend
        .start_session(config(dir.path(), PermissionMode::Ask))
        .await
        .unwrap();
    let mut events = handle.subscribe();
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        backend.invoke_command(&handle.id, goal.clone(), UserInput::text("/goal Reply with the single word DONE and mark this goal complete once that response is delivered. Do not access files, run commands, or use external services.")).await?;
        let mut started = false;
        let mut answered = false;
        loop {
            let event = events.recv().await.map_err(|error| AgentError::Backend(error.to_string()))?;
            match event {
                AgentEvent::TurnStarted { .. } => started = true,
                AgentEvent::AssistantMessage { text, .. } if text.trim() == "DONE" => answered = true,
                AgentEvent::TurnCompleted { .. } if started && answered => return Ok::<_, AgentError>(()),
                AgentEvent::Error { message, .. } => return Err(AgentError::Backend(message)),
                _ => {}
            }
        }
    }).await;
    let clear = backend
        .invoke_command(&handle.id, goal, UserInput::text("/goal clear"))
        .await;
    backend.shutdown().await.unwrap();
    clear.expect("clear smoke-test goal");
    result
        .expect("native goal turn timed out")
        .expect("native goal execution");
}

#[tokio::test]
#[ignore]
async fn real_goal_ui_pause_resume_clear_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let backend = CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    });
    let config = config(dir.path(), PermissionMode::Ask);
    let handle = backend.start_session(config.clone()).await.unwrap();
    let mut events = handle.subscribe();
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        backend.control_goal(&handle.id, GoalAction::Set("Reply with DONE and mark this goal complete after replying. Do not use tools, files, or external services.".into())).await.unwrap();
        backend.control_goal(&handle.id, GoalAction::Pause).await.unwrap();
        loop {
            if matches!(events.recv().await.unwrap(), AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Paused) { break; }
        }
        backend.close(&handle.id).await.unwrap();
        let resumed = backend.resume(config, &handle.id).await.unwrap();
        assert!(resumed.history().iter().any(|e| matches!(e, AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Paused)));
        let mut events = resumed.subscribe();
        backend.control_goal(&handle.id, GoalAction::Resume).await.unwrap();
        loop {
            if matches!(events.recv().await.unwrap(), AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Active) { break; }
        }
        backend.control_goal(&handle.id, GoalAction::Clear).await.unwrap();
        loop {
            if matches!(events.recv().await.unwrap(), AgentEvent::GoalUpdated { goal: None }) { break; }
        }
    }).await;
    backend.shutdown().await.unwrap();
    result.expect("native goal UI controls timed out");
}

#[tokio::test]
#[ignore]
async fn real_luna_title_without_reasoning() {
    let backend = CodexBackend::new(Default::default());
    let result = backend.generate_title("User: Build a simple chess game with a dark board and legal move highlighting.\nAssistant: I will implement the board and move validation.").await;
    backend.shutdown().await.unwrap();
    let title = result.unwrap().expect("title");
    eprintln!("Luna title: {title}");
    assert!(title.to_lowercase().contains("chess"));
}

/// Verify native context inheritance and isolation against the installed CLI.
#[tokio::test]
#[ignore = "requires installed authenticated Codex; sends two short model turns"]
async fn native_side_chat_inherits_context() {
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    }));
    let manager = SessionManager::new(
        dir.path().join("sessions.jsonl"),
        [backend as Arc<dyn AgentBackend>],
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let parent = manager.start_session(config(dir.path(), PermissionMode::Plan)).await?;
        let mut parent_events = manager.subscribe(&parent.id)?;
        let marker = format!("fork-check-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        manager.send_prompt(&parent.id, UserInput::text(format!("Remember this exact marker for our conversation: {marker}. Reply only OK. Do not use tools."))).await?;
        collect_until(&mut parent_events, is_end).await;
        let before = manager.subscribe_with_history(&parent.id)?.0;
        let child = manager.fork(ForkSource::Session(parent.id.clone()), ForkLifetime::Temporary).await?;
        assert_eq!(child.context, ForkContext::Native);
        assert_eq!(manager.subscribe_with_history(&child.handle.id)?.0.len(), 1);
        let mut events = manager.subscribe(&child.handle.id)?;
        manager.send_prompt(&child.handle.id, UserInput::text("What is the exact marker I asked you to remember? Reply with just that marker. Do not use tools.")).await?;
        let response = collect_until(&mut events, is_end).await;
        let text: String = response.iter().filter_map(|e| match e { AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert!(text.contains(&marker), "native fork did not recall parent marker: {text}");
        assert_eq!(manager.subscribe_with_history(&parent.id)?.0, before);
        assert_eq!(manager.list_sessions().len(), 1);
        manager.delete(&child.handle.id).await?;
        assert!(manager.is_live(&parent.id));
        Ok::<_, AgentError>(())
    }).await;
    manager.shutdown().await;
    result.expect("bounded fork probe timed out").unwrap();
}

/// Temporary native threads have no rollout to fork again. Switching settings
/// and restoring durable context must use the transcript path instead.
#[tokio::test]
#[ignore = "requires authenticated Codex; sends three short model turns"]
async fn side_chat_model_switch_and_restart_preserve_context() {
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(CodexBackend::new(CodexOptions {
        api_key: None,
        ..Default::default()
    }));
    let manager = SessionManager::new(
        dir.path().join("index"),
        [backend.clone() as Arc<dyn AgentBackend>],
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let mut selected = config(dir.path(), PermissionMode::Plan);
        let models = manager.account_models(Provider::Codex, None, dir.path()).await?;
        let model = models.iter().find(|model| model.id.contains("luna")).expect("Luna catalog entry").id.clone();
        let first = manager.fork(ForkSource::New { config: selected.clone(), account_id: None }, ForkLifetime::Temporary).await?;
        let mut events = manager.subscribe(&first.handle.id)?;
        manager.send_prompt(&first.handle.id, UserInput::text("Remember the marker durable-ibis-6182. Reply only OK. Do not use tools.")).await?;
        collect_until(&mut events, is_end).await;
        selected.model = Some(model.clone());
        let changed = manager.reconfigure_fork(&first.handle.id, selected, None).await?;
        assert_eq!(changed.context, ForkContext::Transcript);
        assert_eq!(manager.session_record(&changed.handle.id)?.model.as_deref(), Some(model.as_str()));
        manager.delete(&first.handle.id).await?;
        let mut events = manager.subscribe(&changed.handle.id)?;
        manager.send_prompt(&changed.handle.id, UserInput::text("What marker did I ask you to remember? Reply only the marker. Do not use tools.")).await?;
        let response = collect_until(&mut events, is_end).await;
        let answer: String = response.iter().filter_map(|event| match event { AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert!(answer.contains("durable-ibis-6182"), "model switch lost context: {answer}");
        let saved = serde_json::to_vec(&manager.export_fork(&changed.handle.id)?).unwrap();
        manager.delete(&changed.handle.id).await?;
        let restored = manager.restore_fork(serde_json::from_slice(&saved).unwrap()).await?;
        let mut events = manager.subscribe(&restored.handle.id)?;
        manager.send_prompt(&restored.handle.id, UserInput::text("Repeat the marker from our conversation. Reply only the marker. Do not use tools.")).await?;
        let response = collect_until(&mut events, is_end).await;
        let answer: String = response.iter().filter_map(|event| match event { AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert!(answer.contains("durable-ibis-6182"), "restart lost context: {answer}");
        assert!(manager.list_sessions().is_empty());
        manager.delete(&restored.handle.id).await?;
        Ok::<_, AgentError>(())
    }).await;
    manager.shutdown().await;
    result.expect("bounded side chat probe timed out").unwrap();
}
