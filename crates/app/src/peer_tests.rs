//! Opt-in live transport check. Uses isolated Del metadata and two disposable
//! native conversations; existing user conversations are never messaged.
use crate::mcp_bridge::McpBridge;
use del_agent::{
    AgentBackend, AgentEvent, PermissionMode, Provider, SessionConfig, SessionManager, UserInput,
};
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires authenticated Codex CLI and spends model tokens"]
async fn live_codex_peers_exchange_context_and_finish_original_work() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(del_codex::CodexBackend::new(Default::default()));
    let manager = SessionManager::new(
        dir.path().join("sessions.jsonl"),
        [backend.clone() as Arc<dyn AgentBackend>],
    )
    .unwrap();
    let (events, _ui) = tokio::sync::mpsc::unbounded_channel();
    let bridge = McpBridge::start(manager.clone(), events, dir.path().join("mcp"))
        .await
        .unwrap();
    backend.set_browser_tools(bridge.handle().chat_browser_tools().unwrap());
    let mut observations = manager.subscribe_all();
    let trace = tokio::spawn(async move {
        while let Ok(event) = observations.recv().await {
            if let del_agent::ManagerEvent::Event { session, event } = event {
                match event {
                    AgentEvent::ToolCallStarted {
                        name,
                        input_summary,
                        ..
                    } => eprintln!("[peer live] {session} tool {name}: {input_summary}"),
                    AgentEvent::AssistantMessage { text, .. } => {
                        eprintln!("[peer live] {session}: {text}")
                    }
                    AgentEvent::ApprovalRequested { kind, detail, .. } => {
                        eprintln!("[peer live] approval {kind:?}: {detail}")
                    }
                    AgentEvent::TurnCompleted { status, .. } => {
                        eprintln!("[peer live] {session} turn {status:?}")
                    }
                    AgentEvent::PeerWorkChanged { work } => {
                        eprintln!("[peer live] {session} work {work:?}")
                    }
                    AgentEvent::Error { message, .. } => {
                        eprintln!("[peer live] {session} error: {message}")
                    }
                    AgentEvent::PeerMessage { message } => {
                        eprintln!("[peer live] receipt {} {:?}", message.id, message.state)
                    }
                    _ => {}
                }
            }
        }
    });
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::Codex,
        model: None,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: PermissionMode::Ask,
    };
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        let b = manager.start_session(config.clone()).await.unwrap();
        let a = manager.start_session(config).await.unwrap();
        manager.send_prompt(&b.id, UserInput::text("This is an isolated Del peer test. Remember the marker PEER_CONTEXT_COBALT. Reply READY now. If a Del peer later asks for the marker, reply through reply_to_session and explicitly complete your work with session_work complete. Do not use tools except Del peer tools; do not access files, commands, browsers, or external services.")).await.unwrap();
        loop {
            if manager.status(&b.id) == Some(del_agent::SessionStatus::Idle) { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        manager.send_prompt(&a.id, UserInput::text(format!("This is an isolated Del peer integration test. Your task is to use Del read_session to read peer {}'s context, then send_to_session to ask that peer for its marker (idempotency key live-marker-request). Do not just answer from the read: ask and receive its reply. Wait using session_work wait if necessary. Once the peer reply arrives, report the marker in your final answer and call session_work complete. Do not use any tools except Del peer tools; do not access files, commands, browsers, or external services.", b.id))).await.unwrap();
        loop {
            let messages = manager.peer_messages(&a.id);
            let answered = messages.iter().any(|m| m.reply_to.is_some() && m.text.contains("PEER_CONTEXT_COBALT"));
            let finished = manager.peer_work(&a.id).is_some_and(|w| w.status == del_agent::peers::WorkStatus::Complete);
            if answered && finished && manager.status(&a.id) == Some(del_agent::SessionStatus::Idle) {
                let history = manager.subscribe_with_history(&a.id).unwrap().0;
                assert!(history.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name.contains("read_session"))));
                assert!(history.iter().any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text.contains("PEER_CONTEXT_COBALT"))));
                assert!(messages.iter().any(|m| m.idempotency_key == "live-marker-request"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }).await;
    bridge.shutdown();
    manager.shutdown().await;
    trace.abort();
    assert!(
        result.is_ok(),
        "live peer exchange did not finish within 180 seconds"
    );
}

/// A finished assignment must not prevent a later peer from delegating new work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires authenticated Codex CLI and spends model tokens"]
async fn live_codex_peer_accepts_new_work_after_completed_task() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(del_codex::CodexBackend::new(Default::default()));
    let manager = SessionManager::new(
        dir.path().join("sessions.jsonl"),
        [backend.clone() as Arc<dyn AgentBackend>],
    )
    .unwrap();
    let (events, _ui) = tokio::sync::mpsc::unbounded_channel();
    let bridge = McpBridge::start(manager.clone(), events, dir.path().join("mcp"))
        .await
        .unwrap();
    backend.set_browser_tools(bridge.handle().chat_browser_tools().unwrap());
    let mut observations = manager.subscribe_all();
    let trace = tokio::spawn(async move {
        while let Ok(observation) = observations.recv().await {
            let del_agent::ManagerEvent::Event { session, event } = observation else {
                continue;
            };
            match event {
                AgentEvent::AssistantMessage { text, .. } => {
                    eprintln!("[peer new work] {session}: {text}")
                }
                AgentEvent::ToolCallStarted { name, .. } => {
                    eprintln!("[peer new work] {session} tool: {name}")
                }
                AgentEvent::Error { message, .. } => {
                    eprintln!("[peer new work] {session} error: {message}")
                }
                AgentEvent::ApprovalRequested { kind, .. } => {
                    eprintln!("[peer new work] {session} approval: {kind:?}")
                }
                _ => {}
            }
        }
    });
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::Codex,
        model: None,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: PermissionMode::AutoEdit,
    };
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        let recipient = manager.start_session(config.clone()).await.unwrap();
        let sender = manager.start_session(config).await.unwrap();
        manager.send_prompt(&recipient.id, UserInput::text(
            "Your assignment is to calculate 17 times 23. For this assignment, do not edit files, browse, or contact other sessions. Report the result and mark this assignment complete with session_work complete."
        )).await.unwrap();
        loop {
            if manager.status(&recipient.id) == Some(del_agent::SessionStatus::Idle)
                && manager.peer_work(&recipient.id).is_none_or(|w| w.status == del_agent::peers::WorkStatus::Complete) { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        manager.send_prompt(&sender.id, UserInput::text(format!(
            "Ask Del session {} to do a new task: create delegated.txt in this project's directory containing exactly PEER_NEW_WORK_OK followed by a newline, and reply with its result. Use send_to_session, wait for the reply, then report completion and call session_work complete. Do not create the file yourself. Do not contact any other sessions or access the network.", recipient.id
        ))).await.unwrap();
        loop {
            let messages = manager.peer_messages(&sender.id);
            if let Some(reply) = messages.iter().find(|m| m.from == recipient.id && m.reply_to.is_some()) {
                let content = std::fs::read_to_string(dir.path().join("delegated.txt"));
                if content.as_deref().ok() != Some("PEER_NEW_WORK_OK\n") {
                    return Err(format!("recipient replied without doing the delegated file task: {}", reply.text));
                }
                let history = manager.subscribe_with_history(&recipient.id).unwrap().0;
                // Verify the recipient edited the file, rather than accepting a sender's claim.
                if !history.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. }
                    if name == "apply_patch" || name == "shell")) {
                    return Err("recipient did not execute an edit tool".into());
                }
                if !history.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name.contains("reply_to_session"))) {
                    return Err("recipient did not send a tracked reply".into());
                }
                if manager.status(&recipient.id) == Some(del_agent::SessionStatus::Idle)
                    && manager.status(&sender.id) == Some(del_agent::SessionStatus::Idle)
                    && [&recipient.id, &sender.id].iter().all(|id| manager.peer_work(id).is_some_and(|w| w.status == del_agent::peers::WorkStatus::Complete)) { break; }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok::<(), String>(())
    }).await;
    bridge.shutdown();
    manager.shutdown().await;
    trace.abort();
    result
        .expect("new peer task did not finish within 240 seconds")
        .expect("delegated task failed");
}
