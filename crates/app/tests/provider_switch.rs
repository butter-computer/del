//! Opt-in native-provider acceptance test. Uses disposable chats and a real
//! Del MCP bridge; never modifies the user's existing conversations.
#[path = "../src/mcp_bridge.rs"]
#[allow(dead_code)]
mod mcp_bridge;

use del_agent::*;
use std::{sync::Arc, time::Duration};

fn manager(path: &std::path::Path, opencode_root: &std::path::Path) -> SessionManager {
    let environment = [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ]
    .map(|key| {
        (
            key.to_owned(),
            opencode_root.join(key).to_string_lossy().into_owned(),
        )
    })
    .into();
    SessionManager::new(
        path,
        [
            Arc::new(del_codex::CodexBackend::new(del_codex::CodexOptions {
                api_key: None,
                ..Default::default()
            })) as Arc<dyn AgentBackend>,
            Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
                setting_sources: Some("".into()),
                ..Default::default()
            })),
            Arc::new(del_opencode::OpenCodeBackend::with_environment(environment)),
        ],
    )
    .unwrap()
}

async fn prompt(manager: &SessionManager, chat: &SessionId, input: &str) -> Vec<AgentEvent> {
    let mut rx = manager.subscribe(chat).unwrap();
    manager
        .send_prompt(chat, UserInput::text(input))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        let mut events = Vec::new();
        loop {
            let event = rx.recv().await.unwrap();
            if let AgentEvent::Error { message, .. } = &event {
                panic!("native provider error: {message}");
            }
            if let AgentEvent::ApprovalRequested { id, .. } = &event {
                manager
                    .respond_approval(chat, id, ApprovalResponse::Approve)
                    .await
                    .unwrap();
            }
            let done = matches!(
                event,
                AgentEvent::TurnCompleted {
                    status: TurnStatus::Completed,
                    ..
                }
            );
            events.push(event);
            if done {
                return events;
            }
        }
    })
    .await
    .expect("native turn timeout")
}

#[tokio::test]
#[ignore = "requires installed authenticated Codex and Claude plus OpenCode network access; sends seven short model turns"]
async fn native_all_directions_attach_without_inference_and_read_saved_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.jsonl");
    let manager = manager(&path, dir.path());
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let bridge = mcp_bridge::McpBridge::start_with_options(
        manager.clone(),
        tx,
        dir.path().join("mcp"),
        del_mcp::ServerOptions::default(),
    )
    .await
    .unwrap();
    manager.set_browser_tools(bridge.handle().chat_browser_tools().unwrap());
    let chat = manager
        .start_session(SessionConfig {
            provider: Provider::Codex,
            cwd: dir.path().to_path_buf(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::Plan,
        })
        .await
        .unwrap()
        .id;
    let fact = format!("DEL_SWITCH_FACT_{}", uuid::Uuid::new_v4().simple());
    prompt(
        &manager,
        &chat,
        &format!(
            "Remember this test fact: {fact}. Reply with exactly that fact. Do not use tools."
        ),
    )
    .await;
    for provider in [
        Provider::Claude,
        Provider::OpenCode,
        Provider::Codex,
        Provider::OpenCode,
        Provider::Claude,
        Provider::Codex,
    ] {
        let from = manager.session_record(&chat).unwrap().provider;
        let mut rx = manager.subscribe(&chat).unwrap();
        manager
            .switch_provider(&chat, provider, None)
            .await
            .unwrap();
        eprintln!("Native switch {from:?} → {provider:?} acknowledged");
        let models = manager
            .account_models(provider, None, dir.path())
            .await
            .unwrap();
        assert!(
            !models.is_empty(),
            "{provider:?} returned an empty catalog after switch"
        );
        eprintln!("Native {provider:?}: {} available models", models.len());
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(
                    event,
                    AgentEvent::TurnStarted { .. } | AgentEvent::TokenUsage { .. }
                ),
                "switch started inference: {event:?}"
            );
        }
        if provider == Provider::OpenCode {
            manager
                .set_model_settings(&chat, Some("opencode/big-pickle".into()), None, None)
                .unwrap();
        }
        if provider == Provider::Claude {
            manager
                .set_model_settings(&chat, Some("haiku".into()), None, None)
                .unwrap();
        }
        // Exercise a cold native resume, including the empty target case.
        manager.close(&chat).await.unwrap();
        manager.resume(&chat).await.unwrap();
        let events = prompt(&manager, &chat, &format!("Use Del's read_session tool with session_id {chat} to read this chat's saved public history. Then reply with only the original DEL_SWITCH_FACT value. Do not use any other tools or modify files.")).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolCallStarted { .. })),
            "provider did not exercise historical retrieval"
        );
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text.contains(&fact))
            ),
            "provider lost the original fact: {events:?}"
        );
        assert_eq!(manager.list_sessions().len(), 1);
        eprintln!("Native {provider:?} restored context and read the original history");
    }
    manager.shutdown().await;
    bridge.shutdown();
}
