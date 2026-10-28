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
        reasoning_effort: None,
        service_tier: None,
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
    // Both were answered before the server reported them resolved.
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalResolved { .. }))
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
async fn terminal_display_title_uses_exact_conversation_and_provider_preview_fallback() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/read","params":{"threadId":"terminal-conversation","includeTurns":false}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"terminal-conversation","name":null,"preview":"uhuh"}}}"#,
        r#"> {"id":4,"method":"thread/read","params":{"threadId":"terminal-conversation","includeTurns":false}}"#,
        r#"< {"id":4,"result":{"thread":{"id":"terminal-conversation","name":"Enjoying life","preview":"uhuh"}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let id = SessionId::new("terminal-conversation");
    assert_eq!(
        backend
            .session_display_title(&id, std::path::Path::new("/work"))
            .await
            .unwrap()
            .as_deref(),
        Some("uhuh")
    );
    assert_eq!(
        backend
            .session_display_title(&id, std::path::Path::new("/work"))
            .await
            .unwrap()
            .as_deref(),
        Some("Enjoying life")
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn provider_names_refresh_and_persist_without_resuming_or_reordering_chats() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/resume","params":{"threadId":"saved"}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"saved","turns":[]}}}"#,
        r#"> {"id":30,"method":"thread/goal/get","params":{"threadId":"saved"}}"#,
        r#"< {"id":30,"result":{"goal":null}}"#,
        r#"> {"id":4,"method":"thread/read","params":{"threadId":"saved","includeTurns":false}}"#,
        r#"< {"id":4,"result":{"thread":{"id":"saved","name":"Build a smooth sidebar","preview":"hello"}}}"#,
        // New activity, after which the provider's name is worth asking for again.
        r#"> {"id":40,"method":"turn/start","params":{"threadId":"saved","input":[{"type":"text","text":"next"}]}}"#,
        r#"< {"id":40,"result":{"turn":{"id":"next"}}}"#,
        r#"< {"method":"turn/started","params":{"threadId":"saved","turn":{"id":"next"}}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"saved","turn":{"id":"next","status":"completed"}}}"#,
        r#"> {"id":5,"method":"thread/read","params":{"threadId":"saved","includeTurns":false}}"#,
        r#"< {"id":5,"result":{"thread":{"id":"saved","name":"Renamed in Codex","preview":"hello"}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = std::sync::Arc::new(CodexBackend::attach(opts(), client, rx).await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.jsonl");
    let id = SessionId::new("saved");
    let record = SessionRecord {
        id: id.clone(),
        provider: Provider::Codex,
        account_id: None,
        cwd: "/work".into(),
        model: None,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: PermissionMode::Ask,
        title: Some("hello".into()),
        title_turns: 0,
        created_at_ms: 1,
        updated_at_ms: 2,
        status: SessionStatus::Idle,
    };
    del_agent::index::SessionIndex::open(&path)
        .unwrap()
        .upsert(record.clone())
        .unwrap();
    let manager =
        SessionManager::new(&path, [backend as std::sync::Arc<dyn AgentBackend>]).unwrap();
    // Titles are only read for live sessions; never resume one just for its name.
    manager.resume(&id).await.unwrap();
    let (_, mut events) = manager.subscribe_with_history(&id).unwrap();
    let order = |m: &SessionManager| {
        m.list_sessions()
            .iter()
            .map(|r| (r.id.clone(), r.updated_at_ms))
            .collect::<Vec<_>>()
    };
    let before = order(&manager);
    assert!(manager.refresh_titles().await.unwrap());
    assert_eq!(
        manager.list_sessions()[0].title.as_deref(),
        Some("Build a smooth sidebar")
    );
    assert_eq!(order(&manager), before, "a rename must not reorder chats");
    // Without new activity the provider is not asked again.
    assert!(!manager.refresh_titles().await.unwrap());
    manager
        .send_prompt(&id, UserInput::text("next"))
        .await
        .unwrap();
    until_turn_end(&mut events).await;
    manager.flush_index().await;
    let before = order(&manager);
    assert!(manager.refresh_titles().await.unwrap());
    assert_eq!(order(&manager), before, "a rename must not reorder chats");
    assert!(!manager.refresh_titles().await.unwrap());
    manager.flush_index().await;
    let saved = del_agent::index::SessionIndex::open(&path)
        .unwrap()
        .list()
        .remove(0);
    assert_eq!(saved.title.as_deref(), Some("Renamed in Codex"));
    assert_eq!(saved.updated_at_ms, before[0].1);
    assert_eq!(saved.status, SessionStatus::Idle);
    manager.shutdown().await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn resumed_history_reaches_chat_subscribers() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/resume","params":{"threadId":"saved"}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"saved","turns":[{"id":"old-turn","status":"completed","items":[{"type":"userMessage","id":"u1","content":[{"type":"text","text":"hello again"}]},{"type":"agentMessage","id":"a1","text":"Welcome back"}]}]}}}"#,
        r#"> {"id":30,"method":"thread/goal/get","params":{"threadId":"saved"}}"#,
        r#"< {"id":30,"result":{"goal":null}}"#,
        r#"> {"id":4,"method":"turn/start","params":{"threadId":"saved","input":[{"type":"text","text":"first after reopening"}]}}"#,
        r#"< {"id":4,"result":{"turn":{"id":"new-turn"}}}"#,
        r#"< {"method":"turn/started","params":{"threadId":"saved","turn":{"id":"new-turn"}}}"#,
        r#"< {"method":"item/completed","params":{"threadId":"saved","turnId":"new-turn","item":{"type":"agentMessage","id":"a2","text":"First reply"}}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"saved","turn":{"id":"new-turn","status":"completed"}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = std::sync::Arc::new(CodexBackend::attach(opts(), client, rx).await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.jsonl");
    let id = SessionId::new("saved");
    del_agent::index::SessionIndex::open(&path)
        .unwrap()
        .upsert(SessionRecord {
            id: id.clone(),
            provider: Provider::Codex,
            account_id: None,
            cwd: "/work".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::Ask,
            title: Some("hello again".into()),
            title_turns: 0,
            created_at_ms: 1,
            updated_at_ms: 1,
            status: SessionStatus::Idle,
        })
        .unwrap();
    let manager =
        SessionManager::new(&path, [backend as std::sync::Arc<dyn AgentBackend>]).unwrap();
    // Sidebar selection and ChatView hydration can both request resume before
    // the first RPC completes. They must share one attachment.
    let (first, second) = tokio::time::timeout(Duration::from_millis(500), async {
        tokio::join!(manager.resume(&id), manager.resume(&id))
    })
    .await
    .expect("concurrent reopen must reuse one resume RPC");
    first.unwrap();
    second.unwrap();
    let (history, mut events) = manager.subscribe_with_history(&id).unwrap();
    assert!(
        history.iter().any(|event| matches!(event,
            AgentEvent::AssistantMessage { text, .. } if text == "Welcome back"
        )),
        "saved messages must reach a newly opened chat: {history:?}"
    );
    assert_eq!(
        history,
        vec![
            AgentEvent::SessionStarted {
                session_id: id.clone()
            },
            AgentEvent::TurnStarted {
                turn_id: "old-turn".into()
            },
            AgentEvent::UserMessage {
                text: "hello again".into()
            },
            AgentEvent::AssistantMessage {
                turn_id: "old-turn".into(),
                text: "Welcome back".into()
            },
            AgentEvent::TurnCompleted {
                turn_id: "old-turn".into(),
                status: TurnStatus::Completed
            },
            AgentEvent::GoalUpdated { goal: None },
        ]
    );
    // Reopening an already-live session must not append the snapshot again.
    manager.resume(&id).await.unwrap();
    assert_eq!(manager.subscribe_with_history(&id).unwrap().0, history);
    manager
        .send_prompt(&id, UserInput::text("first after reopening"))
        .await
        .unwrap();
    let live = until_turn_end(&mut events).await;
    assert!(
        live.iter().any(|event| matches!(event,
            AgentEvent::AssistantMessage { text, .. } if text == "First reply"
        )),
        "the original chat subscription must survive reopening and the first send"
    );
    manager.shutdown().await;
    server.await.unwrap().unwrap();
}

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
        r#"> {"id":5,"method":"turn/steer","params":{"threadId":"t1","expectedTurnId":"u1","input":[{"type":"text","text":"left please"},{"type":"localImage","path":"/work/picture.png"}]}}"#,
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
    backend
        .steer_input(
            &h.id,
            UserInput {
                command: None,
                skills: Vec::new(),
                text: "left please".into(),
                attachments: vec![Attachment {
                    path: "/work/picture.png".into(),
                    mime: Some("image/png".into()),
                }],
            },
        )
        .await
        .unwrap();
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

#[derive(Default)]
struct RecordedCredentials(std::sync::Mutex<Vec<String>>);

impl del_agent::backend::BrowserCredentials for RecordedCredentials {
    fn mint(&self, session: &SessionId) -> std::io::Result<String> {
        self.0.lock().unwrap().push(format!("mint {session}"));
        Ok(format!("token-{session}"))
    }
    fn rebind(&self, from: &SessionId, to: &SessionId) {
        self.0.lock().unwrap().push(format!("rebind {from} {to}"));
    }
    fn revoke(&self, session: &SessionId) {
        self.0.lock().unwrap().push(format!("revoke {session}"));
    }
    fn mint_shared(&self, _: &SessionId) -> std::io::Result<String> {
        unreachable!("Codex binds every credential to one thread")
    }
    fn set_shared_member(&self, _: &SessionId, _: &SessionId, _: bool) {}
}

#[tokio::test]
async fn browser_credentials_are_bound_to_each_thread() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"thread/start","params":{}}"#,
        r#"< {"id":4,"result":{"thread":{"id":"t2"}}}"#,
        r#"> {"id":5,"method":"thread/unsubscribe","params":{"threadId":"t1"}}"#,
        r#"< {"id":5,"result":{}}"#,
        r#"> {"id":6,"method":"thread/resume","params":{"threadId":"t1"}}"#,
        r#"< {"id":6,"result":{"thread":{"id":"t1","turns":[]}}}"#,
        r#"> {"id":7,"method":"thread/goal/get","params":{"threadId":"t1"}}"#,
        r#"< {"id":7,"result":{"goal":null}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let credentials = std::sync::Arc::new(RecordedCredentials::default());
    backend.set_browser_tools(del_agent::backend::BrowserTools {
        url: "http://127.0.0.1:1/mcp".into(),
        credentials: credentials.clone(),
        claude_session_config: None,
        question: None,
    });
    let first = backend.start_session(config()).await.unwrap();
    let second = backend.start_session(config()).await.unwrap();
    backend.close(&first.id).await.unwrap();
    // A resumed thread's id is known up front.
    backend.resume(config(), &first.id).await.unwrap();
    let log = credentials.0.lock().unwrap().clone();
    assert_eq!(log.len(), 6, "{log:?}");
    assert_eq!(log[5], "mint t1");
    // Each new thread gets its own provisional credential, rebound to its id.
    let provisional: Vec<_> = log[..5]
        .iter()
        .filter_map(|line| line.strip_prefix("mint "))
        .collect();
    assert_eq!(provisional.len(), 2);
    assert_ne!(provisional[0], provisional[1]);
    assert_eq!(log[1], format!("rebind {} {}", provisional[0], first.id));
    assert_eq!(log[3], format!("rebind {} {}", provisional[1], second.id));
    assert_eq!(log[4], "revoke t1");
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn server_resolved_approval_retires_the_card() {
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
        r#"< {"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"t1","turnId":"u1","itemId":"c1","command":"ls"}}"#,
        r#"< {"method":"serverRequest/resolved","params":{"threadId":"t1","requestId":7}}"#,
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
    let evs = until_turn_end(&mut ev).await;
    let requested = evs
        .iter()
        .position(|e| matches!(e, AgentEvent::ApprovalRequested { id, .. } if id == "7"))
        .expect("approval requested");
    let resolved = evs
        .iter()
        .position(|e| *e == AgentEvent::ApprovalResolved { id: "7".into() })
        .expect("approval resolved");
    assert!(requested < resolved);
    // The request is gone, so a late answer is refused instead of sent.
    assert!(
        backend
            .respond_approval(&h.id, "7", ApprovalResponse::Approve)
            .await
            .is_err()
    );
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
async fn rejected_turn_start_does_not_block_the_next_prompt() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"go"}]}}"#,
        r#"< {"id":4,"error":{"code":-32600,"message":"nope"}}"#,
        r#"> {"id":5,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"go"}]}}"#,
        r#"< {"id":5,"result":{"turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let h = backend.start_session(config()).await.unwrap();
    assert!(
        backend
            .send_prompt(&h.id, UserInput::text("go"))
            .await
            .is_err()
    );
    backend
        .send_prompt(&h.id, UserInput::text("go"))
        .await
        .unwrap();
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

#[tokio::test]
async fn model_effort_and_fast_changes_reach_next_turn_in_same_thread() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"go"}],"approvalPolicy":"never","sandboxPolicy":{"type":"dangerFullAccess"},"model":"model-b","effort":"high","serviceTier":"priority","serviceTierForTurn":"priority"}}"#,
        r#"< {"id":4,"result":{"turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"completed","items":[]}}}"#,
        r#"> {"id":5,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"again"}],"approvalPolicy":"untrusted","sandboxPolicy":{"type":"readOnly"},"model":"model-c","effort":"low","serviceTier":null,"serviceTierForTurn":"default"}}"#,
        r#"< {"id":5,"result":{"turn":{"id":"u2","status":"inProgress","items":[]}}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u2","status":"completed","items":[]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let h = backend.start_session(config()).await.unwrap();
    let mut events = h.subscribe();
    let mut settings = config();
    settings.model = Some("model-b".into());
    settings.permission_mode = PermissionMode::FullAuto;
    settings.reasoning_effort = Some("high".into());
    settings.service_tier = Some("priority".into());
    assert_eq!(
        backend
            .reconfigure(settings.clone(), &h.id)
            .await
            .unwrap()
            .id,
        h.id
    );
    backend
        .send_prompt(&h.id, UserInput::text("go"))
        .await
        .unwrap();
    until_turn_end(&mut events).await;
    settings.model = Some("model-c".into());
    settings.permission_mode = PermissionMode::Ask;
    settings.reasoning_effort = Some("low".into());
    settings.service_tier = None;
    backend.reconfigure(settings, &h.id).await.unwrap();
    backend
        .send_prompt(&h.id, UserInput::text("again"))
        .await
        .unwrap();
    until_turn_end(&mut events).await;
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn selected_skills_reach_native_input_and_local_goal_status_finishes() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
        r#"> {"id":4,"method":"skills/list","params":{"cwds":["/work"],"forceReload":true}}"#,
        r#"< {"id":4,"result":{"data":[{"skills":[{"name":"review","path":"/work/SKILL.md","description":"Review","enabled":true},{"name":"check","path":"/work/check/SKILL.md","description":"Check","enabled":true}]}]}}"#,
        r#"> {"id":5,"method":"turn/start","params":{"threadId":"t1","input":[{"type":"text","text":"/review /check check this"},{"type":"skill","name":"review","path":"/work/SKILL.md"},{"type":"skill","name":"check","path":"/work/check/SKILL.md"}]}}"#,
        r#"< {"id":5,"result":{"turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"< {"method":"turn/started","params":{"threadId":"t1","turn":{"id":"u1","status":"inProgress","items":[]}}}"#,
        r#"< {"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"completed","items":[]}}}"#,
        r#"> {"id":6,"method":"thread/goal/get","params":{"threadId":"t1"}}"#,
        r#"< {"id":6,"result":{"goal":null}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let handle = backend.start_session(config()).await.unwrap();
    let mut events = handle.subscribe();
    let commands = backend
        .list_commands(std::path::Path::new("/work"))
        .await
        .unwrap();
    backend
        .invoke_command(
            &handle.id,
            commands[0].clone(),
            UserInput {
                skills: commands
                    .iter()
                    .filter(|c| c.kind == CommandKind::Skill)
                    .cloned()
                    .collect(),
                ..UserInput::text("/review /check check this")
            },
        )
        .await
        .unwrap();
    until_turn_end(&mut events).await;
    let goal = commands
        .into_iter()
        .find(|command| command.kind == CommandKind::Goal)
        .unwrap();
    backend
        .invoke_command(&handle.id, goal, UserInput::text("/goal"))
        .await
        .unwrap();
    let events = until_turn_end(&mut events).await;
    assert!(events.iter().any(|event| matches!(event, AgentEvent::AssistantMessage { text, .. } if text.contains("No goal set"))));
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn goal_controls_use_native_rpcs_and_preserve_literal_objectives() {
    use serde_json::json;
    let mut lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        THREAD[0],
        THREAD[1],
    ]);
    for (id, method, params, status) in [
        (
            4,
            "thread/goal/set",
            json!({"threadId":"t1","objective":"clear","status":"active"}),
            Some("active"),
        ),
        (
            5,
            "thread/goal/set",
            json!({"threadId":"t1","status":"paused"}),
            Some("paused"),
        ),
        (
            6,
            "thread/goal/set",
            json!({"threadId":"t1","status":"active"}),
            Some("active"),
        ),
        (7, "thread/goal/clear", json!({"threadId":"t1"}), None),
    ] {
        lines.push(format!(
            "> {}",
            json!({"id":id,"method":method,"params":params})
        ));
        let goal = status.map(|status| json!({"objective":"clear","status":status,"timeUsedSeconds":4,"tokensUsed":50}));
        lines.push(format!("< {}", json!({"id":id,"result":{"goal":goal}})));
    }
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let handle = backend.start_session(config()).await.unwrap();
    let mut events = handle.subscribe();
    for (action, status) in [
        (GoalAction::Set("clear".into()), Some(GoalStatus::Active)),
        (GoalAction::Pause, Some(GoalStatus::Paused)),
        (GoalAction::Resume, Some(GoalStatus::Active)),
        (GoalAction::Clear, None),
    ] {
        backend.control_goal(&handle.id, action).await.unwrap();
        let AgentEvent::GoalUpdated { goal } = next(&mut events).await else {
            panic!("expected goal state")
        };
        assert_eq!(goal.as_ref().map(|g| g.status), status);
        if let Some(goal) = goal {
            assert_eq!(goal.objective, "clear");
            assert_eq!(goal.time_used_seconds, 4);
        }
    }
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn account_usage_reads_the_selected_connection() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"account/rateLimits/read","params":{}}"#,
        r#"< {"id":3,"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300}}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    assert_eq!(
        backend.account_rate_limits().await.unwrap()["rateLimits"]["primary"]["usedPercent"],
        25
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn account_transfer_resumes_the_copied_native_transcript() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/resume","params":{"threadId":"saved","path":"/profile/sessions/copied.jsonl"}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"saved","turns":[]}}}"#,
        r#"> {"id":4,"method":"thread/goal/get","params":{"threadId":"saved"}}"#,
        r#"< {"id":4,"result":{"goal":null}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let handle = backend
        .resume_transferred(
            config(),
            &SessionId::new("saved"),
            Some(std::path::Path::new("/profile/sessions/copied.jsonl")),
        )
        .await
        .unwrap();
    assert_eq!(handle.id.as_str(), "saved");
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}
