use std::collections::BTreeMap;
use std::time::Duration;

use del_acp::{AcpAgentConfig, AcpBackend};
use del_agent::*;
use tokio::sync::broadcast;

fn backend(env: &[(&str, &str)]) -> AcpBackend {
    AcpBackend::new(vec![AcpAgentConfig {
        id: "mock".into(),
        name: "Mock ACP".into(),
        command: "/usr/bin/python3".into(),
        args: vec![format!(
            "{}/tests/fixtures/agent.py",
            env!("CARGO_MANIFEST_DIR")
        )],
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        install_hint: None,
    }])
}

fn config() -> SessionConfig {
    SessionConfig {
        cwd: std::env::temp_dir(),
        model: Some("mock".into()),
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Acp,
        permission_mode: PermissionMode::Ask,
    }
}

async fn next(rx: &mut broadcast::Receiver<AgentEvent>) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(8), rx.recv())
        .await
        .expect("event timeout")
        .expect("event stream")
}

async fn completed(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = vec![];
    loop {
        let event = next(rx).await;
        let done = matches!(event, AgentEvent::TurnCompleted { .. });
        events.push(event);
        if done {
            return events;
        }
    }
}

async fn approval(rx: &mut broadcast::Receiver<AgentEvent>) -> String {
    loop {
        if let AgentEvent::ApprovalRequested { id, .. } = next(rx).await {
            return id;
        }
    }
}

#[tokio::test]
async fn non_image_attachments_reach_the_agent_as_file_references() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(
            &handle.id,
            UserInput {
                command: None,
                skills: Vec::new(),
                text: "inspect-attachments".into(),
                attachments: [
                    ("report.pdf", "application/pdf"),
                    (
                        "book.xlsx",
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    ),
                    ("clip.mp4", "video/mp4"),
                ]
                .into_iter()
                .map(|(name, mime)| Attachment {
                    path: format!("/attachments/{name}").into(),
                    mime: Some(mime.into()),
                })
                .collect(),
            },
        )
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    let response = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::AssistantMessage { text, .. } => Some(text),
            _ => None,
        })
        .unwrap();
    let blocks: serde_json::Value = serde_json::from_str(response).unwrap();
    for (index, name) in ["report.pdf", "book.xlsx", "clip.mp4"].iter().enumerate() {
        assert_eq!(blocks[index + 1]["type"], "text");
        assert!(blocks[index + 1]["text"].as_str().unwrap().contains(name));
    }
    backend.close(&handle.id).await.unwrap();
}

#[tokio::test]
async fn handshake_prompt_stream_tool_diff_and_reuse() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    for _ in 0..2 {
        backend
            .send_prompt(&handle.id, UserInput::text("hello"))
            .await
            .unwrap();
        let events = completed(&mut rx).await;
        assert!(matches!(events[0], AgentEvent::TurnStarted { .. }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "Hello"))
        );
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::ReasoningDelta { text, .. } if text == "Thinking")
            )
        );
        assert!(events.iter().any(|e| matches!(e, AgentEvent::FileChange { diff: Some(diff), .. } if diff.contains("+new"))));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }
    backend.close(&handle.id).await.unwrap();
}

#[tokio::test]
async fn permission_answers_use_offered_ids_and_expire() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    for (answer, expected) in [
        (ApprovalResponse::Approve, "yes"),
        (ApprovalResponse::Deny, "no"),
    ] {
        backend
            .send_prompt(&handle.id, UserInput::text("permission"))
            .await
            .unwrap();
        let id = approval(&mut rx).await;
        backend
            .respond_approval(&handle.id, &id, answer)
            .await
            .unwrap();
        assert!(
            backend
                .respond_approval(&handle.id, &id, answer)
                .await
                .is_err()
        );
        let events = completed(&mut rx).await;
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::AssistantMessage {text, ..} if text.contains(expected))
        ));
    }
    backend.close(&handle.id).await.unwrap();
}

#[tokio::test]
async fn binary_approval_does_not_grant_allow_always() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("permission-always"))
        .await
        .unwrap();
    let id = approval(&mut rx).await;
    backend
        .respond_approval(&handle.id, &id, ApprovalResponse::Approve)
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::AssistantMessage {text, ..} if text.contains("cancelled"))
    ));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_pending_approval_then_prompt_again() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("permission"))
        .await
        .unwrap();
    let id = approval(&mut rx).await;
    assert!(matches!(
        backend
            .send_prompt(&handle.id, UserInput::text("busy"))
            .await,
        Err(AgentError::Busy(_))
    ));
    backend.interrupt(&handle.id).await.unwrap();
    let events = completed(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    assert!(
        backend
            .respond_approval(&handle.id, &id, ApprovalResponse::Approve)
            .await
            .is_err()
    );
    backend
        .send_prompt(&handle.id, UserInput::text("hello"))
        .await
        .unwrap();
    completed(&mut rx).await;
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn immediate_cancel_is_ordered_after_prompt() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("wait"))
        .await
        .unwrap();
    backend.interrupt(&handle.id).await.unwrap();
    let events = completed(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsupported_client_callbacks_and_steering_are_explicit() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("callbacks"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::AssistantMessage {text, ..} if text == "callbacks declined")
    ));
    assert!(
        backend
            .steer(&handle.id, "more".into())
            .await
            .unwrap_err()
            .to_string()
            .contains("no standard steer")
    );
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn resume_checks_load_capability_and_preserves_agent_identity() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    backend.close(&handle.id).await.unwrap();
    let resumed = backend.resume(config(), &handle.id).await.unwrap();
    assert_eq!(resumed.id, handle.id);
    backend.close(&handle.id).await.unwrap();
    let no_load = self::backend(&[("NO_LOAD", "1")]);
    let error = no_load.resume(config(), &handle.id).await.unwrap_err();
    assert!(error.to_string().contains("does not support session/load"));
}

#[tokio::test]
async fn resume_after_agent_exit_respawns_with_replayed_history() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("exit"))
        .await
        .unwrap();
    completed(&mut rx).await;
    let resumed = backend.resume(config(), &handle.id).await.unwrap();
    assert_eq!(resumed.id, handle.id);
    let history = resumed.history();
    assert!(matches!(&history[0], AgentEvent::UserMessage { text } if text == "earlier"));
    assert!(
        history
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "reply"))
    );
    assert!(
        !history
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantTextDelta { .. }))
    );
    let mut rx = resumed.subscribe();
    backend
        .send_prompt(&resumed.id, UserInput::text("hello"))
        .await
        .unwrap();
    assert!(matches!(
        completed(&mut rx).await.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
    backend.close(&handle.id).await.unwrap();
}

#[tokio::test]
async fn manager_recovers_failed_process_without_reopening_chat() {
    manager_recovers_failed_process(true).await;
}

#[tokio::test]
async fn manager_recovers_failed_process_on_direct_send() {
    manager_recovers_failed_process(false).await;
}

async fn manager_recovers_failed_process(resume_first: bool) {
    let dir = tempfile::tempdir().unwrap();
    let backend = std::sync::Arc::new(backend(&[]));
    let manager = SessionManager::new(
        dir.path().join("sessions.jsonl"),
        [backend as std::sync::Arc<dyn AgentBackend>],
    )
    .unwrap();
    let handle = manager.start_session(config()).await.unwrap();
    let (_, mut events) = manager.subscribe_with_history(&handle.id).unwrap();
    manager
        .send_prompt(&handle.id, UserInput::text("exit"))
        .await
        .unwrap();
    loop {
        if matches!(
            next(&mut events).await,
            AgentEvent::Error {
                recoverable: false,
                ..
            }
        ) {
            break;
        }
    }
    assert_eq!(manager.status(&handle.id), Some(SessionStatus::Error));
    // Both app navigation and direct sends retain the chat and subscription.
    if resume_first {
        manager.resume(&handle.id).await.unwrap();
    }
    let result = manager
        .send_prompt(&handle.id, UserInput::text("hello"))
        .await;
    if result.is_ok() {
        assert!(matches!(
            completed(&mut events).await.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }
    manager.shutdown().await;
    result.expect("a failed provider process must restart on the next send");
}

#[tokio::test]
async fn authentication_error_is_actionable() {
    let backend = backend(&[("AUTH_REQUIRED", "1")]);
    let error = backend.start_session(config()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Sign in using the agent's own CLI")
    );
}

#[tokio::test]
async fn prompt_error_settles_and_allows_retry() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("fail"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Error {
            recoverable: true,
            ..
        }
    )));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Failed,
            ..
        })
    ));
    backend
        .send_prompt(&handle.id, UserInput::text("hello"))
        .await
        .unwrap();
    completed(&mut rx).await;
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn closed_stdout_cannot_deadlock_shutdown() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("close-stdout"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Failed,
            ..
        })
    ));
    tokio::time::timeout(Duration::from_secs(6), backend.close(&handle.id))
        .await
        .expect("shutdown deadline")
        .unwrap();
}

#[tokio::test]
async fn unresponsive_cancellation_retires_agent() {
    let backend = backend(&[("IGNORE_CANCEL", "1")]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("wait"))
        .await
        .unwrap();
    backend.interrupt(&handle.id).await.unwrap();
    let events = completed(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    assert!(
        backend
            .send_prompt(&handle.id, UserInput::text("hello"))
            .await
            .is_err()
    );
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_setup_kills_and_reaps_child() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("pid");
    let backend = std::sync::Arc::new(backend(&[
        ("HANG_INIT", "1"),
        ("PID_FILE", pid_file.to_str().unwrap()),
    ]));
    let b = backend.clone();
    let task = tokio::spawn(async move { b.start_session(config()).await });
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    task.abort();
    let _ = task.await;
    let pid = pid.expect("agent started");
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("child reaped after setup cancellation");
}

#[tokio::test]
async fn failed_stop_reason_is_not_reported_as_completed() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("stop-error"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Failed,
            ..
        })
    ));
    backend.shutdown().await.unwrap();
}

#[derive(Default)]
struct PeerCredentials(std::sync::Mutex<Vec<(String, String)>>);
impl backend::BrowserCredentials for PeerCredentials {
    fn mint(&self, id: &SessionId) -> std::io::Result<String> {
        self.0.lock().unwrap().push(("mint".into(), id.to_string()));
        Ok("test-only-peer-token".into())
    }
    fn rebind(&self, from: &SessionId, to: &SessionId) {
        self.0
            .lock()
            .unwrap()
            .push((from.to_string(), to.to_string()));
    }
    fn revoke(&self, id: &SessionId) {
        self.0
            .lock()
            .unwrap()
            .push(("revoke".into(), id.to_string()));
    }
    fn mint_shared(&self, _: &SessionId) -> std::io::Result<String> {
        panic!("ACP identity must not be shared")
    }
    fn set_shared_member(&self, _: &SessionId, _: &SessionId, _: bool) {
        panic!("ACP identity must not be shared")
    }
}

#[tokio::test]
async fn peer_tools_use_negotiated_transport_and_revoke_rebound_identity() {
    for http in ["1", "0"] {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("mcp.json");
        let backend = backend(&[
            ("MCP_CAPTURE", capture.to_str().unwrap()),
            ("MCP_HTTP", http),
        ]);
        let credentials = std::sync::Arc::new(PeerCredentials::default());
        backend.set_browser_tools(backend::BrowserTools {
            url: "http://127.0.0.1:12345/mcp".into(),
            credentials: credentials.clone(),
            claude_session_config: None,
            question: None,
        });
        let handle = backend.start_session(config()).await.unwrap();
        let servers: serde_json::Value =
            serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
        assert_eq!(servers.as_array().unwrap().len(), 1);
        assert_eq!(servers[0]["name"], "del");
        if http == "1" {
            assert_eq!(servers[0]["type"], "http");
            assert_eq!(
                servers[0]["headers"][0]["value"],
                "Bearer test-only-peer-token"
            );
        } else {
            assert_eq!(servers[0]["args"][0], "--mcp-stdio");
            assert!(servers[0]["command"].as_str().unwrap().starts_with('/'));
            assert_eq!(servers[0]["env"][1]["value"], "test-only-peer-token");
        }
        backend.close(&handle.id).await.unwrap();
        let events = credentials.0.lock().unwrap();
        assert_eq!(events[0].0, "mint");
        assert_eq!(events[1].0, events[0].1);
        assert_eq!(events[1].1, handle.id.to_string());
        assert_eq!(
            events.last().unwrap(),
            &("revoke".into(), handle.id.to_string())
        );
    }
}

fn mode_config(mode: PermissionMode) -> SessionConfig {
    SessionConfig {
        permission_mode: mode,
        ..config()
    }
}

/// Run one permission prompt and return (whether a card was shown, the reply text).
async fn permission_outcome(
    backend: &AcpBackend,
    handle: &SessionHandle,
    prompt: &str,
) -> (bool, String) {
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text(prompt))
        .await
        .unwrap();
    let mut asked = false;
    let mut reply = String::new();
    loop {
        match next(&mut rx).await {
            AgentEvent::ApprovalRequested { id, .. } => {
                asked = true;
                backend
                    .respond_approval(&handle.id, &id, ApprovalResponse::Deny)
                    .await
                    .unwrap();
            }
            AgentEvent::AssistantMessage { text, .. } => reply = text,
            AgentEvent::TurnCompleted { .. } => return (asked, reply),
            _ => {}
        }
    }
}

#[tokio::test]
async fn permission_modes_answer_without_prompting_only_where_allowed() {
    let backend = backend(&[("MODES", "1")]);
    let full = backend
        .start_session(mode_config(PermissionMode::FullAuto))
        .await
        .unwrap();
    let (asked, reply) = permission_outcome(&backend, &full, "permission").await;
    assert!(!asked, "full access never shows a card");
    assert!(reply.contains("\"optionId\": \"yes\""), "{reply}");

    let edit = backend
        .start_session(mode_config(PermissionMode::AutoEdit))
        .await
        .unwrap();
    let (asked, reply) = permission_outcome(&backend, &edit, "permission-edit").await;
    assert!(!asked && reply.contains("\"optionId\": \"yes\""), "{reply}");
    let (asked, reply) = permission_outcome(&backend, &edit, "permission").await;
    assert!(asked, "auto-edit still asks before commands");
    assert!(reply.contains("\"optionId\": \"no\""), "{reply}");

    let plan = backend
        .start_session(mode_config(PermissionMode::Plan))
        .await
        .unwrap();
    let (asked, reply) = permission_outcome(&backend, &plan, "permission").await;
    assert!(!asked && reply.contains("\"optionId\": \"no\""), "{reply}");
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn plan_mode_and_effort_require_the_agent_to_advertise_them() {
    let backend = backend(&[]);
    let error = backend
        .start_session(mode_config(PermissionMode::Plan))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("does not advertise plan mode"),
        "{error}"
    );
    let mut effort = config();
    effort.reasoning_effort = Some("high".into());
    let error = backend.start_session(effort).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not advertise reasoning effort"),
        "{error}"
    );
}

#[tokio::test]
async fn model_catalog_lists_the_agent_default_then_its_models() {
    let backend = backend(&[("MODELS", "1")]);
    let models = backend.list_models(&std::env::temp_dir()).await.unwrap();
    let rows: Vec<_> = models
        .iter()
        .map(|m| (m.id.as_str(), m.name.as_str(), m.is_default))
        .collect();
    assert_eq!(
        rows,
        [
            ("mock", "Mock ACP", true),
            ("mock/m1", "Model One", false),
            ("mock/m2", "m2", false),
        ]
    );
    // A failing probe still offers the agent itself, with the reason.
    let failing = self::backend(&[("AUTH_REQUIRED", "1")]);
    let models = failing.list_models(&std::env::temp_dir()).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "mock");
    assert!(models[0].description.starts_with("Agent default · "));
}

#[tokio::test]
async fn unknown_updates_kinds_and_notifications_are_tolerated() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("unknown-update"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(events.iter().any(|e| matches!(e,
        AgentEvent::ToolCallStarted { id, name, .. } if id == "odd-1" && name == "Tool")));
    assert!(events.iter().any(|e| matches!(e,
        AgentEvent::ToolCallFinished { id, status: ToolStatus::Completed, output_summary }
            if id == "odd-1" && output_summary == "{\"ok\":true}")));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "after"))
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
    backend.shutdown().await.unwrap();
}

// Multi-threaded like the app, where a failed prompt waiter can run while the
// reader is still reporting the exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_exit_mid_turn_reports_one_error_and_fails_the_turn() {
    for _ in 0..5 {
        let backend = backend(&[]);
        let handle = backend.start_session(config()).await.unwrap();
        let mut rx = handle.subscribe();
        backend
            .send_prompt(&handle.id, UserInput::text("exit"))
            .await
            .unwrap();
        let mut events = completed(&mut rx).await;
        assert!(matches!(
            events.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Failed,
                ..
            })
        ));
        // Collect anything else the exit produced.
        while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await
        {
            events.push(event);
        }
        let errors: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Error {
                    message,
                    recoverable,
                } => Some((message.clone(), *recoverable)),
                _ => None,
            })
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(!errors[0].1, "process exit is not recoverable");
        assert!(errors[0].0.starts_with("ACP agent exited"), "{errors:?}");
        assert!(
            backend
                .send_prompt(&handle.id, UserInput::text("hello"))
                .await
                .is_err()
        );
        backend.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn response_with_a_null_error_member_is_a_success() {
    let backend = backend(&[]);
    let handle = backend.start_session(config()).await.unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("null-error"))
        .await
        .unwrap();
    let events = completed(&mut rx).await;
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
        "{events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
    backend.shutdown().await.unwrap();
}
