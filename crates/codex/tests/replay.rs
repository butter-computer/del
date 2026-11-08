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
            AgentEvent::UsageSnapshot {
                context_tokens: Some(27500),
                context_window: Some(475000),
                cost_scope: None,
                cost_usd: None
            },
            AgentEvent::UsageSnapshot {
                context_tokens: None,
                context_window: None,
                cost_scope: Some(del_agent::pricing::cost_scope(
                    Some("01a16315-ed3f-7d80-a7ff-c4d455713fdf"),
                    "estimate:codex:01a16315-ed3f-7d80-a7ff-c4d455713fdf".into()
                )),
                cost_usd: Some(0.0299904),
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
        segments: Vec::new(),
        fork: None,
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
        spawned_by: None,
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
            segments: Vec::new(),
            fork: None,
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
            spawned_by: None,
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
        r#"< {"id":5,"result":{"status":"unsubscribed"}}"#,
        r#"< {"method":"thread/closed","params":{"threadId":"t1"}}"#,
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
async fn closing_last_chat_does_not_interrupt_model_loading() {
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (client_io, server_io) = tokio::io::duplex(8192);
    let (cr, cw) = tokio::io::split(client_io);
    let (sr, mut sw) = tokio::io::split(server_io);
    let (client, incoming) = del_codex::CodexClient::from_io(cr, cw);
    let requested = Arc::new(tokio::sync::Notify::new());
    let ready = requested.clone();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(sr).lines();
        let mut model_request = None;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({"userAgent":"test"}),
                "initialized" => continue,
                "account/read" => json!({"requiresOpenaiAuth":false}),
                "thread/start" => json!({"thread":{"id":"t1"}}),
                "model/list" => {
                    model_request = Some(request["id"].clone());
                    ready.notify_one();
                    continue;
                }
                "thread/unsubscribe" => json!({"status":"unsubscribed"}),
                "config/read" => json!({"config":{}}),
                method => panic!("unexpected request {method}"),
            };
            let reply = json!({"id":request["id"],"result":result});
            sw.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            if request["method"] == "thread/unsubscribe" {
                sw.write_all(b"{\"method\":\"thread/closed\",\"params\":{\"threadId\":\"t1\"}}\n")
                    .await
                    .unwrap();
                let reply = json!({"id":model_request.take().unwrap(),"result":{
                    "data":[{"model":"available-model"}],"nextCursor":null
                }});
                sw.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let backend = Arc::new(
        CodexBackend::attach(opts(), client, incoming)
            .await
            .unwrap(),
    );
    let session = backend.start_session(config()).await.unwrap();
    let listing = {
        let backend = backend.clone();
        tokio::spawn(async move { backend.list_models(std::path::Path::new("/work")).await })
    };
    tokio::time::timeout(Duration::from_secs(2), requested.notified())
        .await
        .unwrap();
    backend.close(&session.id).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), listing)
        .await
        .unwrap()
        .unwrap();
    backend.shutdown().await.unwrap();
    server.await.unwrap();
    let models = result.expect("closing the last chat must not shut down an in-flight model list");
    assert_eq!(models[0].id, "available-model");
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

#[tokio::test]
async fn native_fork_requests_ephemeral_context_without_replaying_parent_turns() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/start","params":{"cwd":"/work","approvalPolicy":"untrusted","sandbox":"read-only","serviceName":"del","config":{"thread_unload_delay_secs":0,"tools.update_plan.enabled":true}}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"parent","turns":[]}}}"#,
        r#"> {"id":4,"method":"thread/fork","params":{"threadId":"parent","cwd":"/work","approvalPolicy":"untrusted","sandbox":"read-only","serviceName":"del","ephemeral":true,"excludeTurns":true,"config":{"thread_unload_delay_secs":0,"tools.update_plan.enabled":true}}}"#,
        r#"< {"id":4,"result":{"thread":{"id":"child","turns":[{"id":"old","status":"completed","items":[{"type":"userMessage","id":"u","content":[{"type":"text","text":"inherited"}]}]}]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    backend.start_session(config()).await.unwrap();
    let child = backend
        .fork_session(config(), &SessionId::new("parent"), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.id.as_str(), "child");
    assert!(child.history().is_empty());
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn closing_one_of_two_chats_waits_for_native_lock_release() {
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client_io, server_io) = tokio::io::duplex(8192);
    let (cr, cw) = tokio::io::split(client_io);
    let (sr, mut sw) = tokio::io::split(server_io);
    let (client, incoming) = del_codex::CodexClient::from_io(cr, cw);
    let unsubscribed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let immediate = Arc::new(AtomicBool::new(false));
    let (ready, proceed, configured) = (unsubscribed.clone(), release.clone(), immediate.clone());
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(sr).lines();
        let mut count = 0;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({"userAgent":"test"}),
                "initialized" => continue,
                "account/read" => json!({"requiresOpenaiAuth":false}),
                "thread/start" => {
                    configured.store(
                        request["params"]["config"]["thread_unload_delay_secs"] == 0,
                        Ordering::SeqCst,
                    );
                    count += 1;
                    json!({"thread":{"id":format!("t{count}")}})
                }
                "thread/unsubscribe" => json!({"status":"unsubscribed"}),
                method => panic!("unexpected request {method}"),
            };
            sw.write_all(format!("{}\n", json!({"id":request["id"],"result":result})).as_bytes())
                .await
                .unwrap();
            if request["method"] == "thread/unsubscribe" {
                ready.notify_one();
                proceed.notified().await;
                sw.write_all(b"{\"method\":\"thread/closed\",\"params\":{\"threadId\":\"t1\"}}\n")
                    .await
                    .unwrap();
            }
        }
    });
    let backend = Arc::new(
        CodexBackend::attach(opts(), client, incoming)
            .await
            .unwrap(),
    );
    let first = backend.start_session(config()).await.unwrap();
    let second = backend.start_session(config()).await.unwrap();
    let closing = {
        let backend = backend.clone();
        tokio::spawn(async move { backend.close(&first.id).await })
    };
    unsubscribed.notified().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let waited = !closing.is_finished();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(backend.interrupt(&second.id).await.is_ok());
    backend.shutdown().await.unwrap();
    server.await.unwrap();
    assert!(
        waited,
        "unsubscribe acknowledgement did not release the native conversation lock"
    );
    assert!(
        immediate.load(Ordering::SeqCst),
        "server must not retain idle ownership after unsubscribe"
    );
}

#[tokio::test]
async fn model_catalog_follows_pages_and_rejects_a_repeated_cursor() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"model/list","params":{"cursor":null}}"#,
        r#"< {"id":3,"result":{"data":[{"model":"m1","displayName":"One","isDefault":true,"futureField":{}},{"model":"secret","hidden":true},{"displayName":"no id"}],"nextCursor":"p2"}}"#,
        r#"> {"id":4,"method":"model/list","params":{"cursor":"p2"}}"#,
        r#"< {"id":4,"result":{"data":[{"model":"m2","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"bogus":1}]}],"nextCursor":null}}"#,
        r#"> {"id":5,"method":"config/read","params":{"cwd":"/work","includeLayers":false}}"#,
        r#"< {"id":5,"result":{"config":{"model":"m2"}}}"#,
        r#"> {"id":6,"method":"model/list","params":{"cursor":null}}"#,
        r#"< {"id":6,"result":{"data":[],"nextCursor":"same"}}"#,
        r#"> {"id":7,"method":"model/list","params":{"cursor":"same"}}"#,
        r#"< {"id":7,"result":{"data":[],"nextCursor":"same"}}"#,
        r#"> {"id":8,"method":"model/list","params":{"cursor":null}}"#,
        r#"< {"id":8,"result":{"models":[]}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let cwd = std::path::Path::new("/work");
    let models = backend.list_models(cwd).await.unwrap();
    let rows: Vec<_> = models
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                m.name.as_str(),
                m.is_default,
                m.efforts.len(),
            )
        })
        .collect();
    assert_eq!(rows, [("m1", "One", false, 0), ("m2", "m2", true, 1)]);
    let repeated = backend.list_models(cwd).await.unwrap_err();
    assert!(
        repeated.to_string().contains("repeated its cursor"),
        "{repeated}"
    );
    let malformed = backend.list_models(cwd).await.unwrap_err();
    assert!(
        malformed.to_string().contains("no model list"),
        "{malformed}"
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn saved_history_read_rebuilds_turns_without_touching_live_state() {
    let lines = script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
        r#"> {"id":3,"method":"thread/read","params":{"threadId":"saved","includeTurns":true}}"#,
        r#"< {"id":3,"result":{"thread":{"id":"saved","turns":[{"id":"t1","status":"inProgress","items":[{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]},{"type":"agentMessage","id":"a","text":"partial"}]}]}}}"#,
    ]);
    let (client, rx, server) = replay(lines);
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    let history = backend
        .read_session_history(&config(), &SessionId::new("saved"))
        .await
        .unwrap();
    assert_eq!(
        history,
        vec![
            AgentEvent::TurnStarted {
                turn_id: "t1".into()
            },
            AgentEvent::UserMessage { text: "hi".into() },
            AgentEvent::AssistantMessage {
                turn_id: "t1".into(),
                text: "partial".into()
            },
            AgentEvent::TurnCompleted {
                turn_id: "t1".into(),
                status: TurnStatus::Interrupted
            },
        ]
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn in_flight_request_fails_promptly_when_the_server_exits() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (cr, cw) = tokio::io::split(client_io);
    let (client, mut rx) = del_codex::CodexClient::from_io(cr, cw);
    let request = {
        let client = client.clone();
        tokio::spawn(async move { client.request("model/list", serde_json::json!({})).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(server_io);
    let result = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .expect("a pending request must not wait for its timeout")
        .unwrap();
    assert!(
        matches!(result, Err(del_codex::ClientError::Disconnected(_))),
        "{result:?}"
    );
    assert!(client.is_closed());
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap(),
        Some(del_codex::Incoming::Closed { .. })
    ));
}

#[tokio::test]
async fn noise_partial_writes_and_null_error_fields_are_tolerated() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (cr, cw) = tokio::io::split(client_io);
    let (sr, mut sw) = tokio::io::split(server_io);
    let (client, mut rx) = del_codex::CodexClient::from_io(cr, cw);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(sr).lines();
        let request: serde_json::Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let id = &request["id"];
        sw.write_all(b"warning: not json\n\n[1,2]\n{\"id\":999,\"result\":{}}\n")
            .await
            .unwrap();
        // One response split across writes, with an explicit null error.
        let reply = format!("{{\"id\":{id},\"error\":null,\"result\":{{\"ok\":true}}}}\r\n");
        let (a, b) = reply.split_at(7);
        sw.write_all(a.as_bytes()).await.unwrap();
        sw.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        sw.write_all(b.as_bytes()).await.unwrap();
        sw.write_all(b"{\"method\":\"future/notification\",\"params\":{\"x\":1}}\n")
            .await
            .unwrap();
        sw.flush().await.unwrap();
        while let Ok(Some(_)) = lines.next_line().await {}
    });
    let result = client.request("thread/list", serde_json::json!({})).await;
    assert_eq!(result.unwrap(), serde_json::json!({"ok":true}));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap(),
        Some(del_codex::Incoming::Notification {
            method: "future/notification".into(),
            params: serde_json::json!({"x":1})
        })
    );
    client.shutdown(Duration::from_millis(100)).await;
    server.await.unwrap();
}

#[tokio::test]
async fn spawned_app_server_gets_goal_and_unload_settings_and_account_home() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("codex-fixture");
    let capture = dir.path().join("launch.json");
    std::fs::write(
        &script,
        r#"#!/usr/bin/python3
import json, os, sys
with open(os.environ['DEL_TEST_CAPTURE'], 'w') as f:
    json.dump({'argv': sys.argv[1:], 'home': os.environ.get('CODEX_HOME'),
        'profile': os.environ.get('DEL_ACCOUNT_PROFILE')}, f)
for line in sys.stdin:
    msg = json.loads(line)
    if 'id' not in msg:
        continue
    result = {'account/read': {'account': {'type': 'apiKey'}, 'requiresOpenaiAuth': True}}.get(msg['method'], {})
    print(json.dumps({'id': msg['id'], 'result': result}), flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let home = dir.path().join("codex-home");
    let backend = CodexBackend::new(CodexOptions {
        spawn: del_codex::SpawnOptions {
            program: script,
            global_args: vec!["-c".into(), "user=1".into()],
            env: vec![
                ("CODEX_HOME".into(), home.display().to_string()),
                ("DEL_ACCOUNT_PROFILE".into(), "1".into()),
                ("DEL_TEST_CAPTURE".into(), capture.display().to_string()),
            ],
        },
        api_key: None,
        shutdown_grace: Duration::from_millis(500),
        ..Default::default()
    });
    assert_eq!(
        backend.check_auth().await.unwrap(),
        del_codex::AuthStatus::ApiKey
    );
    let launch: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&capture).unwrap()).unwrap();
    assert_eq!(
        launch["argv"],
        serde_json::json!([
            "-c",
            "user=1",
            "-c",
            "features.goals=true",
            "-c",
            "thread_unload_delay_secs=0",
            "app-server"
        ])
    );
    assert_eq!(launch["home"], home.display().to_string());
    assert_eq!(launch["profile"], "1");
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_native_fork_baseline_uses_transcript_fallback() {
    let (client, rx, server) = replay(script(&[
        HANDSHAKE[0],
        HANDSHAKE[1],
        HANDSHAKE[2],
        ACCOUNT_OK[0],
        ACCOUNT_OK[1],
    ]));
    let backend = CodexBackend::attach(opts(), client, rx).await.unwrap();
    assert!(
        backend
            .fork_session(config(), &SessionId::new("unloaded"), true)
            .await
            .unwrap()
            .is_none()
    );
    backend.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}
