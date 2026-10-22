mod common;

use common::*;
use del_agent::*;
use del_codex::{CodexBackend, CodexError, CodexOptions};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

fn opts() -> CodexOptions {
    CodexOptions {
        api_key: None,
        ..Default::default()
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        cwd: "/work".into(),
        model: None,
        provider: Provider::Codex,
        permission_mode: PermissionMode::Ask,
    }
}

async fn next(rx: &mut Receiver<AgentEvent>) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event stream closed")
}

async fn until_turn_end(rx: &mut Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    loop {
        let e = next(rx).await;
        let done = matches!(e, AgentEvent::TurnCompleted { .. });
        out.push(e);
        if done {
            return out;
        }
    }
}

#[tokio::test]
async fn replays_plain_turn() {
    let (client, rx, server) = replay(fixture("pong.jsonl"));
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let handle = backend.start_session(config()).await.unwrap();
    let mut events = handle.subscribe();
    backend
        .send_prompt(
            &handle.id,
            UserInput::text("Reply with exactly the word: pong. Do not run any tools."),
        )
        .await
        .unwrap();
    let evs = until_turn_end(&mut events).await;
    let turn = "01a16315-f8f9-7292-86dc-7feacb0d382e".to_string();
    assert_eq!(
        evs,
        vec![
            AgentEvent::TurnStarted {
                turn_id: turn.clone()
            },
            AgentEvent::AssistantTextDelta {
                turn_id: turn.clone(),
                text: "pong".into()
            },
            AgentEvent::AssistantMessage {
                turn_id: turn.clone(),
                text: "pong".into()
            },
            AgentEvent::TokenUsage {
                input: 27495,
                output: 5,
                cached: 13184
            },
            AgentEvent::TurnCompleted {
                turn_id: turn,
                status: TurnStatus::Completed
            },
        ]
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn replays_command_and_file_change_with_approvals() {
    let (client, rx, server) = replay(fixture("tools_approval.jsonl"));
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let handle = backend.start_session(config()).await.unwrap();
    let mut events = handle.subscribe();
    let prompt = "Run the shell command `echo hi` and then create a file hello.txt containing \"hi\" using apply_patch. Then say done.";
    backend
        .send_prompt(&handle.id, UserInput::text(prompt))
        .await
        .unwrap();

    let mut seen = Vec::new();
    loop {
        let e = next(&mut events).await;
        if let AgentEvent::ApprovalRequested { id, .. } = &e {
            backend
                .respond_approval(&handle.id, id, ApprovalResponse::Approve)
                .await
                .unwrap();
        }
        let done = matches!(e, AgentEvent::TurnCompleted { .. });
        seen.push(e);
        if done {
            break;
        }
    }

    let find = |f: &dyn Fn(&AgentEvent) -> bool| seen.iter().position(f).expect("event missing");
    let shell_start =
        find(&|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "shell"));
    let cmd_approval = find(&|e| {
        matches!(
            e,
            AgentEvent::ApprovalRequested {
                kind: ApprovalKind::Command,
                ..
            }
        )
    });
    let shell_end = find(
        &|e| matches!(e, AgentEvent::ToolCallFinished { output_summary, .. } if output_summary == "hi\n"),
    );
    let patch_approval = find(&|e| {
        matches!(
            e,
            AgentEvent::ApprovalRequested {
                kind: ApprovalKind::FileChange,
                ..
            }
        )
    });
    let file_change = find(&|e| {
        matches!(
            e,
            AgentEvent::FileChange {
                kind: FileChangeKind::Created,
                ..
            }
        )
    });
    assert!(
        shell_start < cmd_approval
            && cmd_approval < shell_end
            && shell_end < patch_approval
            && patch_approval < file_change
    );

    match &seen[cmd_approval] {
        AgentEvent::ApprovalRequested { id, detail, .. } => {
            assert_eq!(id, "0");
            assert!(detail.contains("echo hi"), "{detail}");
        }
        _ => unreachable!(),
    }
    match &seen[patch_approval] {
        AgentEvent::ApprovalRequested { detail, .. } => {
            assert!(detail.contains("add /work/hello.txt"), "{detail}")
        }
        _ => unreachable!(),
    }
    match &seen[file_change] {
        AgentEvent::FileChange { path, diff, .. } => {
            assert_eq!(path.to_str(), Some("/work/hello.txt"));
            assert_eq!(diff.as_deref(), Some("hi\n"));
        }
        _ => unreachable!(),
    }
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::AssistantMessage { text, .. } if text == "done"))
    );
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Completed,
            ..
        })
    ));
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

fn script(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| s.to_string()).collect()
}

const HANDSHAKE: [&str; 3] = [
    r#"> {"id":1,"method":"initialize","params":{}}"#,
    r#"< {"id":1,"result":{"userAgent":"x","codexHome":"/h","platformFamily":"unix","platformOs":"macos"}}"#,
    r#"> {"method":"initialized"}"#,
];
const ACCOUNT_OK: [&str; 2] = [
    r#"> {"id":2,"method":"account/read","params":{"refreshToken":false}}"#,
    r#"< {"id":2,"result":{"account":{"type":"chatgpt","email":null,"planType":"plus"},"requiresOpenaiAuth":true}}"#,
];
const THREAD: [&str; 2] = [
    r#"> {"id":3,"method":"thread/start","params":{}}"#,
    r#"< {"id":3,"result":{"thread":{"id":"t1"}}}"#,
];

#[tokio::test]
async fn not_logged_in_is_a_clear_error() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        r#"< {"id":2,"result":{"account":null,"requiresOpenaiAuth":true}}"#,
    ]);
    let (client, rx, _server) = replay(lines);
    let err = CodexBackend::attach(opts(), client, rx)
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, CodexError::NotLoggedIn), "{err:?}");
    assert!(err.to_string().contains("codex login"));
}

#[tokio::test]
async fn api_key_mode_logs_in_through_codex() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        r#"< {"id":2,"result":{"account":null,"requiresOpenaiAuth":true}}"#,
        r#"> {"id":3,"method":"account/login/start","params":{}}"#,
        r#"< {"id":3,"result":{"type":"apiKey"}}"#,
        r#"> {"id":4,"method":"account/read","params":{"refreshToken":false}}"#,
        r#"< {"id":4,"result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let o = CodexOptions {
        api_key: Some("sk-test".into()),
        ..Default::default()
    };
    let backend = CodexBackend::attach(o, client, rx).await.unwrap();
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn steer_and_interrupt_use_the_active_turn_id() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"go"}]}}"#,
        r#"< {"id":4,"result":{"turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"< {"method":"turn/started","params":{"threadId":"t1","turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"> {"id":5,"method":"turn/steer","params":{"threadId":"t1","expectedTurnId":"u1","input":[{"type":"text","text":"left please"}]}}"#,
        r#"< {"id":5,"result":{"turnId":"u1"}}"#,
        r#"> {"id":6,"method":"turn/interrupt","params":{"threadId":"t1","turnId":"u1"}}"#,
        r#"< {"id":6,"result":{}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"interrupted","items":[]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let h = backend.start_session(config()).await.unwrap();
    let mut ev = h.subscribe();
    backend
        .send_prompt(&h.id, UserInput::text("go"))
        .await
        .unwrap();
    assert!(matches!(
        backend.send_prompt(&h.id, UserInput::text("again")).await,
        Err(AgentError::Busy(_))
    ));
    backend.steer(&h.id, "left please".into()).await.unwrap();
    backend.interrupt(&h.id).await.unwrap();
    let evs = until_turn_end(&mut ev).await;
    assert!(matches!(
        evs.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Interrupted,
            ..
        })
    ));
    // No turn left: steer errors, interrupt is a no-op.
    assert!(backend.steer(&h.id, "x".into()).await.is_err());
    backend.interrupt(&h.id).await.unwrap();
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn failed_turn_and_unsupported_request() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"go"}]}}"#,
        r#"< {"id":4,"result":{"turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"< {"id":"srv-1","method":"item/tool/requestUserInput","params":{"threadId":"t1","turnId":"u1","itemId":"i","isBlocking":true,"questions":[]}}"#,
        r#"> {"id":"srv-1","error":{"code":-32601,"message":"del does not support server request `item/tool/requestUserInput`"}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"failed","error":{"message":"boom"},"items":[]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let h = backend.start_session(config()).await.unwrap();
    let mut ev = h.subscribe();
    backend
        .send_prompt(&h.id, UserInput::text("go"))
        .await
        .unwrap();
    let evs = until_turn_end(&mut ev).await;
    assert!(matches!(
        &evs[0],
        AgentEvent::Error {
            recoverable: true,
            ..
        }
    ));
    assert_eq!(
        evs[1],
        AgentEvent::Error {
            message: "boom".into(),
            recoverable: false
        }
    );
    assert!(matches!(
        evs[2],
        AgentEvent::TurnCompleted {
            status: TurnStatus::Failed,
            ..
        }
    ));
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn overloaded_requests_are_retried_with_backoff() {
    use del_codex::rpc::BackoffPolicy;
    let lines = script(&[
        r#"> {"id":1,"method":"thread/list","params":{}}"#,
        r#"< {"id":1,"error":{"code":-32001,"message":"Server overloaded; retry later."}}"#,
        r#"> {"id":2,"method":"thread/list","params":{}}"#,
        r#"< {"id":2,"error":{"code":-32001,"message":"Server overloaded; retry later."}}"#,
        r#"> {"id":3,"method":"thread/list","params":{}}"#,
        r#"< {"id":3,"result":{"data":[]}}"#,
    ]);
    let b = BackoffPolicy {
        max_retries: 4,
        initial: Duration::from_millis(5),
        max: Duration::from_millis(20),
    };
    let (client, _rx, server) = replay_with(lines, b);
    let v = client
        .request("thread/list", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(v["data"], serde_json::json!([]));
    client.shutdown(Duration::from_millis(100)).await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn overload_gives_up_after_max_retries() {
    use del_codex::{ClientError, rpc::BackoffPolicy};
    let mut lines = Vec::new();
    for i in 1..=3 {
        lines.push(format!(
            r#"> {{"id":{i},"method":"thread/list","params":{{}}}}"#
        ));
        lines.push(format!(
            r#"< {{"id":{i},"error":{{"code":-32001,"message":"busy"}}}}"#
        ));
    }
    let b = BackoffPolicy {
        max_retries: 2,
        initial: Duration::from_millis(1),
        max: Duration::from_millis(2),
    };
    let (client, _rx, server) = replay_with(lines, b);
    let err = client
        .request("thread/list", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Rpc(e) if e.code == -32001));
    client.shutdown(Duration::from_millis(100)).await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn server_exit_surfaces_error_event_and_fails_requests() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (cr, cw) = tokio::io::split(client_io);
    let (client, rx) = del_codex::CodexClient::from_io(cr, cw);
    let _ = (client.clone(), rx);
    drop(server_io); // EOF
    tokio::time::sleep(Duration::from_millis(50)).await;
    let err = client
        .request("thread/list", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, del_codex::ClientError::Disconnected(_)),
        "{err:?}"
    );
}
