//! Compile the app's GPUI-independent bridge in the MCP test target as well,
//! keeping its transport/lifecycle contract testable without a desktop window.
#[path = "../../app/src/mcp_bridge.rs"]
mod mcp_bridge;

use del_agent::{AgentBackend, MockBackend, Provider, SessionConfig, SessionId, SessionManager};
use del_mcp::{ServerOptions, SessionInfo, SessionMode, UserAnswer, launch::LaunchConfig};
use mcp_bridge::{BridgeEvent, BridgeHandle, McpBridge};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn terminal(id: &str, provider: Option<Provider>) -> SessionInfo {
    SessionInfo {
        id: SessionId::new(id),
        provider,
        mode: SessionMode::Terminal,
        command: Some(
            match provider {
                Some(Provider::Codex) => "codex",
                Some(Provider::Claude) => "claude",
                Some(Provider::Acp) => "acp",
                None => "/bin/zsh",
            }
            .into(),
        ),
        cwd: std::env::temp_dir(),
        title: Some(format!("Terminal {id}")),
        status: "running".into(),
        reported_state: None,
    }
}

fn env(config: &LaunchConfig, key: &str) -> String {
    config
        .env
        .iter()
        .find(|(name, _)| name == key)
        .unwrap()
        .1
        .clone()
}

struct Fixture {
    _dir: tempfile::TempDir,
    manager: SessionManager,
    bridge: McpBridge,
    handle: BridgeHandle,
    events: UnboundedReceiver<BridgeEvent>,
    config: LaunchConfig,
}

impl Fixture {
    async fn new(options: ServerOptions) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(
            dir.path().join("index.jsonl"),
            [Arc::new(MockBackend::new(Provider::Codex)) as Arc<dyn AgentBackend>],
        )
        .unwrap();
        let (tx, events) = unbounded_channel();
        let bridge =
            McpBridge::start_with_options(manager.clone(), tx, dir.path().join("mcp"), options)
                .await
                .unwrap();
        let handle = bridge.handle();
        handle.upsert_terminal(
            terminal("parent", Some(Provider::Codex)),
            Some("terminal output".into()),
        );
        let config = handle
            .launch_config(&SessionId::new("parent"), Provider::Codex)
            .unwrap();
        Self {
            _dir: dir,
            manager,
            bridge,
            handle,
            events,
            config,
        }
    }

    async fn event(&mut self) -> BridgeEvent {
        tokio::time::timeout(Duration::from_secs(3), self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

async fn post(config: &LaunchConfig, method: &str, params: Value) -> (u16, Value) {
    let url = env(config, "DEL_MCP_URL");
    let token = env(config, "DEL_MCP_TOKEN");
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent
            .post(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .send(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string())
            .unwrap();
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    })
    .await
    .unwrap()
}

async fn call(config: &LaunchConfig, tool: &str, args: Value) -> Value {
    let (status, body) = post(config, "tools/call", json!({"name":tool,"arguments":args})).await;
    assert_eq!(status, 200);
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

#[tokio::test]
async fn real_manager_and_terminal_context_status_notify_and_summary() {
    let mut f = Fixture::new(Default::default()).await;
    let chat = f
        .manager
        .start_session(SessionConfig {
            cwd: f._dir.path().into(),
            model: None,
            provider: Provider::Codex,
            permission_mode: Default::default(),
        })
        .await
        .unwrap();
    f.handle.upsert_terminal(terminal("shell", None), None);
    let list = call(&f.config, "list_sessions", json!({})).await;
    let rows = list["structuredContent"]["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let chat_row = rows.iter().find(|r| r["id"] == chat.id.as_str()).unwrap();
    assert_eq!(chat_row["mode"], "chat");
    assert_eq!(chat_row["provider"], "codex");
    let shell = rows.iter().find(|r| r["id"] == "shell").unwrap();
    assert!(shell["provider"].is_null());
    assert_eq!(shell["command"], "/bin/zsh");
    assert_eq!(shell["mode"], "terminal");
    assert!(
        f.handle
            .launch_config(&SessionId::new("shell"), Provider::Codex)
            .is_err()
    );

    let result = call(
        &f.config,
        "report_status",
        json!({"state":"working","summary":"Compiling"}),
    )
    .await;
    assert_eq!(result["isError"], false);
    match f.event().await {
        BridgeEvent::Status {
            caller,
            state,
            summary,
        } => {
            assert_eq!(caller.as_str(), "parent");
            assert_eq!(state, del_mcp::ReportedState::Working);
            assert_eq!(summary, "Compiling");
        }
        _ => panic!("expected status"),
    }
    f.handle
        .update_terminal_output(&SessionId::new("parent"), "🦀".repeat(2_001));
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"parent"}),
    )
    .await;
    assert_eq!(
        summary["structuredContent"]["reported_summary"],
        "Compiling"
    );
    assert_eq!(
        summary["structuredContent"]["info"]["reported_state"],
        "working"
    );
    assert_eq!(
        summary["structuredContent"]["last_output"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        2_000
    );
    let result = call(
        &f.config,
        "notify",
        json!({"title":"Done","body":"Check output"}),
    )
    .await;
    assert_eq!(result["isError"], false);
    match f.event().await {
        BridgeEvent::Notify {
            caller,
            title,
            body,
        } => {
            assert_eq!(caller.as_str(), "parent");
            assert_eq!(title, "Done");
            assert_eq!(body, "Check output");
        }
        _ => panic!("expected notification"),
    }
    f.manager.shutdown().await;
}

#[tokio::test]
async fn visible_question_response_does_not_block_other_requests() {
    let mut f = Fixture::new(Default::default()).await;
    let config = f.config.clone();
    let task = tokio::spawn(async move {
        call(
            &config,
            "ask_user",
            json!({"question":"Continue?","options":["Yes","No"]}),
        )
        .await
    });
    let (id, reply) = match f.event().await {
        BridgeEvent::AskUser {
            request_id,
            caller,
            question,
            options,
            reply,
        } => {
            assert_eq!(caller.as_str(), "parent");
            assert_eq!(question, "Continue?");
            assert_eq!(options, ["Yes", "No"]);
            (request_id, reply)
        }
        _ => panic!("expected visible question event"),
    };
    assert!(!task.is_finished());
    assert_eq!(post(&f.config, "ping", json!({})).await.0, 200);
    reply
        .send(UserAnswer::Option {
            index: 1,
            text: "No".into(),
        })
        .unwrap();
    let answer = task.await.unwrap();
    assert_eq!(answer["structuredContent"]["answer"], "No");
    assert_eq!(answer["structuredContent"]["option_index"], 1);
    assert!(
        matches!(f.event().await, BridgeEvent::QuestionClosed { request_id } if request_id == id)
    );
}

#[tokio::test]
async fn spawn_and_send_dispatch_real_terminal_operations_with_family_scope() {
    let mut f = Fixture::new(Default::default()).await;
    let config = f.config.clone();
    let cwd = f._dir.path().to_path_buf();
    let task = tokio::spawn(async move {
        call(
            &config,
            "spawn_session",
            json!({"provider":"codex","cwd":cwd,"prompt":"Build it"}),
        )
        .await
    });
    match f.event().await {
        BridgeEvent::SpawnTerminal {
            caller,
            request,
            reply,
        } => {
            assert_eq!(caller.as_str(), "parent");
            assert_eq!(request.prompt, "Build it");
            assert_eq!(request.provider, Provider::Codex);
            assert_eq!(request.cwd, f._dir.path());
            let mut child = terminal("child", Some(Provider::Codex));
            child.cwd = request.cwd;
            f.handle
                .upsert_terminal(child.clone(), Some("initial child output".into()));
            reply.send(Ok(child)).unwrap();
        }
        _ => panic!("expected terminal launch event"),
    }
    let result = task.await.unwrap();
    assert_eq!(result["structuredContent"]["id"], "child");
    let child = f
        .handle
        .launch_config(&SessionId::new("child"), Provider::Codex)
        .unwrap();
    for (config, target) in [(f.config.clone(), "child"), (child, "parent")] {
        let task = tokio::spawn(async move {
            call(
                &config,
                "send_to_session",
                json!({"session_id":target,"text":"next step"}),
            )
            .await
        });
        match f.event().await {
            BridgeEvent::SendTerminal {
                caller,
                target,
                text,
                reply,
            } => {
                assert_ne!(caller, target);
                assert_eq!(text, "next step");
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("expected terminal send event"),
        }
        assert_eq!(task.await.unwrap()["isError"], false);
    }
    f.handle.upsert_terminal(terminal("stranger", None), None);
    assert_eq!(
        call(
            &f.config,
            "send_to_session",
            json!({"session_id":"stranger","text":"no"})
        )
        .await["isError"],
        true
    );
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"child"}),
    )
    .await;
    assert_eq!(
        summary["structuredContent"]["last_output"],
        "initial child output"
    );
}

#[tokio::test]
async fn revocation_rotation_and_shutdown_cancel_questions_and_clean_files() {
    let mut f = Fixture::new(Default::default()).await;
    let previous = f.config.clone();
    f.config = f
        .handle
        .launch_config(&SessionId::new("parent"), Provider::Codex)
        .unwrap();
    assert_eq!(post(&previous, "ping", json!({})).await.0, 401);
    assert_eq!(post(&f.config, "ping", json!({})).await.0, 200);
    let config = f.config.clone();
    let task =
        tokio::spawn(async move { call(&config, "ask_user", json!({"question":"Wait"})).await });
    let (id, reply) = match f.event().await {
        BridgeEvent::AskUser {
            request_id, reply, ..
        } => (request_id, reply),
        _ => panic!("expected question"),
    };
    f.handle.revoke(&SessionId::new("parent"));
    assert_eq!(task.await.unwrap()["isError"], true);
    assert!(reply.is_closed());
    assert!(
        matches!(f.event().await, BridgeEvent::QuestionClosed { request_id } if request_id == id)
    );
    assert_eq!(post(&f.config, "ping", json!({})).await.0, 401);
    f.handle
        .upsert_terminal(terminal("claude", Some(Provider::Claude)), None);
    let claude = f
        .handle
        .launch_config(&SessionId::new("claude"), Provider::Claude)
        .unwrap();
    assert!(claude.files[0].exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&claude.files[0])
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    f.handle.remove_terminal(&SessionId::new("claude"));
    assert!(!claude.files[0].exists());
    f.handle
        .upsert_terminal(terminal("claude", Some(Provider::Claude)), None);
    let claude = f
        .handle
        .launch_config(&SessionId::new("claude"), Provider::Claude)
        .unwrap();
    f.config = f
        .handle
        .launch_config(&SessionId::new("parent"), Provider::Codex)
        .unwrap();
    let config = f.config.clone();
    let task =
        tokio::spawn(async move { call(&config, "ask_user", json!({"question":"Quit?"})).await });
    let reply = match f.event().await {
        BridgeEvent::AskUser { reply, .. } => reply,
        _ => panic!("expected question"),
    };
    // Drop the owner while its clone survives; the listener still shuts down.
    let endpoint = f.handle.endpoint().unwrap();
    drop(f.bridge);
    assert!(f.handle.endpoint().is_none());
    assert!(!claude.files[0].exists());
    assert_eq!(task.await.unwrap()["isError"], true);
    assert!(reply.is_closed());
    assert!(
        f.handle
            .launch_config(&SessionId::new("parent"), Provider::Codex)
            .is_err()
    );
    let authority = endpoint
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/mcp")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while tokio::net::TcpStream::connect(authority).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn timed_out_question_is_removed_from_ui_and_dropped_responder_errors() {
    let mut f = Fixture::new(ServerOptions {
        ask_timeout: Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let config = f.config.clone();
    let task =
        tokio::spawn(
            async move { call(&config, "ask_user", json!({"question":"Timeout?"})).await },
        );
    let (id, reply) = match f.event().await {
        BridgeEvent::AskUser {
            request_id, reply, ..
        } => (request_id, reply),
        _ => panic!("expected question"),
    };
    assert_eq!(task.await.unwrap()["isError"], true);
    assert!(reply.is_closed());
    assert!(
        matches!(f.event().await, BridgeEvent::QuestionClosed { request_id } if request_id == id)
    );
    let config = f.config.clone();
    let task =
        tokio::spawn(
            async move { call(&config, "ask_user", json!({"question":"Dismiss?"})).await },
        );
    match f.event().await {
        BridgeEvent::AskUser { reply, .. } => drop(reply),
        _ => panic!("expected question"),
    }
    assert_eq!(task.await.unwrap()["isError"], true);
}

#[tokio::test]
async fn production_start_api_serves_and_shuts_down() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SessionManager::new(dir.path().join("index"), []).unwrap();
    let (tx, _rx) = unbounded_channel();
    let bridge = McpBridge::start(manager, tx, dir.path().join("mcp"))
        .await
        .unwrap();
    assert!(
        bridge
            .handle()
            .endpoint()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    bridge.shutdown();
    assert!(bridge.handle().endpoint().is_none());
}

#[tokio::test]
async fn opaque_terminal_lease_is_generation_safe_and_cancels_pending_questions() {
    let mut f = Fixture::new(Default::default()).await;
    let first = f
        .handle
        .register_terminal(terminal("leased", Some(Provider::Codex)))
        .unwrap()
        .unwrap();
    let first_config = first.config().clone();
    assert_eq!(post(&first_config, "ping", json!({})).await.0, 200);
    let replacement = f
        .handle
        .register_terminal(terminal("leased", Some(Provider::Codex)))
        .unwrap()
        .unwrap();
    let replacement_config = replacement.config().clone();
    assert_eq!(post(&first_config, "ping", json!({})).await.0, 401);
    drop(first);
    assert_eq!(post(&replacement_config, "ping", json!({})).await.0, 200);
    let config = replacement_config.clone();
    let task = tokio::spawn(async move {
        call(&config, "ask_user", json!({"question":"Still running?"})).await
    });
    let (request_id, reply) = match f.event().await {
        BridgeEvent::AskUser {
            request_id, reply, ..
        } => (request_id, reply),
        _ => panic!("expected question"),
    };
    // Exact opaque type accepted by the terminal host, no bridge coupling.
    let opaque: Box<dyn std::any::Any> = Box::new(replacement);
    drop(opaque);
    assert_eq!(task.await.unwrap()["isError"], true);
    assert!(reply.is_closed());
    assert!(
        matches!(f.event().await, BridgeEvent::QuestionClosed { request_id: id } if id == request_id)
    );
    assert_eq!(post(&replacement_config, "ping", json!({})).await.0, 401);
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"leased"}),
    )
    .await;
    assert_eq!(summary["structuredContent"]["info"]["id"], "leased");

    let claude = f
        .handle
        .register_terminal(terminal("claude-leased", Some(Provider::Claude)))
        .unwrap()
        .unwrap();
    let file = claude.config().files[0].clone();
    assert!(file.exists());
    drop(claude);
    assert!(!file.exists());
}

#[tokio::test]
async fn shell_registration_has_context_without_agent_lease() {
    let f = Fixture::new(Default::default()).await;
    assert!(
        f.handle
            .register_terminal(terminal("plain-shell", None))
            .unwrap()
            .is_none()
    );
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"plain-shell"}),
    )
    .await;
    assert!(summary["structuredContent"]["info"]["provider"].is_null());
    assert_eq!(summary["structuredContent"]["info"]["command"], "/bin/zsh");
    assert!(
        f.handle
            .register_terminal(terminal("acp", Some(Provider::Acp)))
            .is_err()
    );
    let mut chat = terminal("wrong-mode", Some(Provider::Codex));
    chat.mode = SessionMode::Chat;
    assert!(f.handle.register_terminal(chat).is_err());
}
