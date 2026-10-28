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
