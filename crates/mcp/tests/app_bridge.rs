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
                Some(Provider::OpenCode) => "opencode",
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
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Codex,
            permission_mode: Default::default(),
        })
        .await
        .unwrap();
    f.handle.upsert_terminal(terminal("shell", None), None);
    // Unrelated projects and plain shells stay outside the peer scope.
    let list = call(&f.config, "list_sessions", json!({})).await;
    let rows = list["structuredContent"]["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "parent");
    assert_eq!(rows[0]["mode"], "terminal");
    for other in [chat.id.as_str(), "shell"] {
        let summary = call(
            &f.config,
            "get_session_summary",
            json!({"session_id": other}),
        )
        .await;
        assert_eq!(summary["isError"], true);
    }
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
async fn terminal_peers_queue_until_safe_boundary_and_can_pull_without_interrupting() {
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
    // Project membership is host-owned, independent of the spawn family.
    f.manager
        .set_peer_project(&SessionId::new("child"), &std::env::temp_dir())
        .unwrap();
    let receipt = call(
        &f.config,
        "send_to_session",
        json!({"session_id":"child","text":"next step","idempotency_key":"first"}),
    )
    .await;
    assert_eq!(receipt["structuredContent"]["receipt"]["state"], "queued");
    // Unknown/working terminal status cannot cause typing into a CLI.
    assert!(
        tokio::time::timeout(Duration::from_millis(400), f.events.recv())
            .await
            .is_err()
    );
    let pulled = call(&child, "session_work", json!({"action":"receive"})).await;
    assert_eq!(pulled["structuredContent"]["message"]["text"], "next step");
    let request = pulled["structuredContent"]["message"]["id"]
        .as_str()
        .unwrap();
    let replied = call(
        &child,
        "reply_to_session",
        json!({"request_id":request,"text":"done","idempotency_key":"answer"}),
    )
    .await;
    assert_eq!(replied["isError"], false);
    let _ = call(
        &f.config,
        "report_status",
        json!({"state":"idle","summary":"Ready for peer input"}),
    )
    .await;
    assert!(matches!(f.event().await, BridgeEvent::Status { .. }));
    match f.event().await {
        BridgeEvent::SendTerminal {
            caller,
            target,
            text,
            reply,
        } => {
            assert_eq!(caller.as_str(), "child");
            assert_eq!(target.as_str(), "parent");
            assert!(text.contains("del_peer_message") && text.contains("done"));
            reply.send(Ok(())).unwrap();
        }
        _ => panic!("expected idle-terminal wakeup"),
    }
    // Repeated tool retries return the same receipt and never replay a send.
    let again = call(
        &f.config,
        "send_to_session",
        json!({"session_id":"child","text":"next step","idempotency_key":"first"}),
    )
    .await;
    assert_eq!(
        again["structuredContent"]["receipt"]["id"],
        receipt["structuredContent"]["receipt"]["id"]
    );
    let context = call(
        &f.config,
        "read_session",
        json!({"session_id":"child","max_characters":1000}),
    )
    .await;
    assert_eq!(context["isError"], false);
    assert!(context.to_string().contains("initial child output"));
    f.handle.upsert_terminal(terminal("stranger", None), None);
    assert_eq!(
        call(
            &f.config,
            "send_to_session",
            json!({"session_id":"stranger","text":"no","idempotency_key":"denied"})
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
    // Metadata outlives the lease: a relaunch finds it and reads its own summary.
    let relaunched = f
        .handle
        .launch_config(&SessionId::new("leased"), Provider::Codex)
        .unwrap();
    let summary = call(
        &relaunched,
        "get_session_summary",
        json!({"session_id":"leased"}),
    )
    .await;
    assert_eq!(summary["structuredContent"]["info"]["id"], "leased");
    // Other agents in the same project can read it.
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"leased"}),
    )
    .await;
    assert_eq!(summary["isError"], false);

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
    // Plain shells are not agent peers.
    let summary = call(
        &f.config,
        "get_session_summary",
        json!({"session_id":"plain-shell"}),
    )
    .await;
    assert_eq!(summary["isError"], true);
    assert!(
        f.handle
            .register_terminal(terminal("acp", Some(Provider::Acp)))
            .is_err()
    );
    let mut chat = terminal("wrong-mode", Some(Provider::Codex));
    chat.mode = SessionMode::Chat;
    assert!(f.handle.register_terminal(chat).is_err());
}

#[tokio::test]
async fn http_disconnect_removes_its_pending_question() {
    use tokio::io::AsyncWriteExt;
    let mut f = Fixture::new(Default::default()).await;
    let endpoint = f.handle.endpoint().unwrap();
    let authority = endpoint
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/mcp")
        .unwrap();
    let mut connection = tokio::net::TcpStream::connect(authority).await.unwrap();
    let body = json!({"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"ask_user","arguments":{"question":"Disconnect me"}}}).to_string();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        env(&f.config, "DEL_MCP_TOKEN"),
        body.len()
    );
    connection.write_all(request.as_bytes()).await.unwrap();
    let (id, reply) = match f.event().await {
        BridgeEvent::AskUser {
            request_id, reply, ..
        } => (request_id, reply),
        _ => panic!("expected question"),
    };
    drop(connection);
    let closed = tokio::time::timeout(Duration::from_secs(2), f.events.recv())
        .await
        .expect("disconnected HTTP request must remove its UI question")
        .unwrap();
    assert!(matches!(closed, BridgeEvent::QuestionClosed { request_id } if request_id == id));
    assert!(reply.is_closed());
}

#[tokio::test]
async fn browser_commands_reach_ui_and_chat_credentials_are_restricted() {
    let mut f = Fixture::new(Default::default()).await;
    let config = f.config.clone();
    let pending = tokio::spawn(async move {
        call(
            &config,
            "browser",
            json!({"action":"snapshot","session_id":"parent","tab_id":"tab-a"}),
        )
        .await
    });
    match f.event().await {
        BridgeEvent::Browser {
            caller,
            allowed,
            request,
            reply,
        } => {
            assert_eq!(caller.as_str(), "parent");
            assert_eq!(allowed, [SessionId::new("parent")]);
            assert_eq!(request.action, "snapshot");
            assert_eq!(request.tab_id.as_deref(), Some("tab-a"));
            reply.send(Ok(json!({"title":"Fixture"}))).unwrap();
        }
        _ => panic!("expected browser command"),
    }
    let result = pending.await.unwrap();
    assert!(result.get("structuredContent").is_none());
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["title"], "Fixture");
    // Terminal agents cannot reach browsers outside their spawn family.
    f.handle.upsert_terminal(terminal("stranger", None), None);
    let denied = call(
        &f.config,
        "browser",
        json!({"action":"snapshot","session_id":"stranger"}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    // Codex/OpenCode chats get browser-only credentials bound to their own chat.
    let chat_config = || SessionConfig {
        cwd: std::env::temp_dir(),
        model: None,
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Codex,
        permission_mode: Default::default(),
    };
    let a = f.manager.start_session(chat_config()).await.unwrap().id;
    let b = f.manager.start_session(chat_config()).await.unwrap().id;
    let tools = f.handle.chat_browser_tools().unwrap();
    let credential = |token: String| LaunchConfig {
        args: vec![],
        env: vec![
            ("DEL_MCP_URL".into(), tools.url.clone()),
            ("DEL_MCP_TOKEN".into(), token),
        ],
        files: vec![],
    };
    let chat_a = credential(tools.credentials.mint(&a).unwrap());
    let (_, listing) = post(&chat_a, "tools/list", json!({})).await;
    let names: Vec<_> = listing["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"browser")
            && names.contains(&"send_to_session")
            && names.contains(&"read_session")
    );
    assert!(names.contains(&"spawn_session"));
    let peers = call(&chat_a, "list_sessions", json!({})).await;
    assert_eq!(peers["isError"], false);
    // Chat A cannot reach chat B's browser.
    let denied = call(
        &chat_a,
        "browser",
        json!({"action":"snapshot","session_id":b.as_str()}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    // Without session_id it reaches only its own browser.
    let own = |config: LaunchConfig| {
        tokio::spawn(async move { call(&config, "browser", json!({"action":"list_tabs"})).await })
    };
    let pending = own(chat_a.clone());
    match f.event().await {
        BridgeEvent::Browser {
            caller,
            allowed,
            reply,
            ..
        } => {
            assert_eq!(caller, a);
            assert_eq!(allowed, vec![a.clone()]);
            reply.send(Ok(json!({"sessions":[]}))).unwrap();
        }
        _ => panic!("expected browser command"),
    }
    assert_eq!(pending.await.unwrap()["isError"], false);
    // A new thread's provisional credential follows it to its real id.
    let provisional = SessionId::new("codex-pending-test");
    let chat_b = credential(tools.credentials.mint(&provisional).unwrap());
    tools.credentials.rebind(&provisional, &b);
    let pending = own(chat_b.clone());
    match f.event().await {
        BridgeEvent::Browser {
            caller,
            allowed,
            reply,
            ..
        } => {
            assert_eq!(caller, b);
            assert_eq!(allowed, vec![b.clone()]);
            reply.send(Ok(json!({"sessions":[]}))).unwrap();
        }
        _ => panic!("expected browser command"),
    }
    assert_eq!(pending.await.unwrap()["isError"], false);
    tools.credentials.revoke(&b);
    assert_eq!(post(&chat_b, "tools/list", json!({})).await.0, 401);
    assert_eq!(post(&chat_a, "tools/list", json!({})).await.0, 200);
    // A workspace-shared credential reaches only its member chats, and of
    // those only the ones running a turn.
    let group = SessionId::new("opencode-workspace-test");
    let shared = credential(tools.credentials.mint_shared(&group).unwrap());
    tools.credentials.set_shared_member(&group, &a, true);
    let denied = call(
        &shared,
        "browser",
        json!({"action":"snapshot","session_id":b.as_str()}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    let idle = call(&shared, "browser", json!({"action":"list_tabs"})).await;
    assert_eq!(idle["isError"], true);
    f.bridge.shutdown();
}

#[tokio::test]
#[ignore = "requires OpenCode 2.x and a network-accessible model; use isolated XDG data"]
async fn real_opencode_warmed_server_uses_chat_browser_bridge() {
    use del_agent::{AgentEvent, PermissionMode, TurnStatus, UserInput};
    let mut f = Fixture::new(Default::default()).await;
    let backend = Arc::new(del_opencode::OpenCodeBackend::default());
    // Catalogs can warm a workspace before the app supplies its MCP bridge.
    backend.list_models(f._dir.path()).await.unwrap();
    backend.set_browser_tools(f.handle.chat_browser_tools().unwrap());
    // The bridge authorizes running chats through SessionManager, just as
    // production does; a backend-only session is intentionally invisible.
    f.manager
        .register_account("opencode-live-test".into(), backend.clone());
    let session = f
        .manager
        .start_account_session(
            SessionConfig {
                provider: Provider::OpenCode,
                cwd: f._dir.path().into(),
                model: Some("opencode/big-pickle".into()),
                reasoning_effort: None,
                service_tier: None,
                permission_mode: PermissionMode::FullAuto,
            },
            Some("opencode-live-test".into()),
        )
        .await
        .unwrap();
    let mut events = session.subscribe();
    f.manager.send_prompt(&session.id, UserInput::text(
        "Use the Del MCP browser tool with action list_tabs. Report the verification marker from its response. Do not use other tools or navigate anywhere."
    )).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let mut called = false;
        let mut text = String::new();
        loop {
            tokio::select! {
                event = f.events.recv() => match event.unwrap() {
                    BridgeEvent::Browser {allowed,request,reply,..} => {
                        assert_eq!(allowed, vec![session.id.clone()]);
                        assert_eq!(request.action, "list_tabs");
                        called = true;
                        reply.send(Ok(json!({"tabs":[],"verification_marker":"DEL_MCP_BROWSER_4826_OK"}))).unwrap();
                    },
                    _ => panic!("unexpected UI operation"),
                },
                event = events.recv() => match event.unwrap() {
                    AgentEvent::AssistantTextDelta {text:delta,..} => text.push_str(&delta),
                    AgentEvent::TurnCompleted {status,..} => {
                        assert_eq!(status, TurnStatus::Completed);
                        assert!(called, "native provider did not call the browser bridge: {text}");
                        assert!(text.contains("DEL_MCP_BROWSER_4826_OK"), "reply: {text}");
                        break;
                    },
                    AgentEvent::Error {message,..} => panic!("{message}"),
                    _ => {}
                }
            }
        }
    }).await;
    f.manager.shutdown().await;
    f.bridge.shutdown();
    result.expect("native browser integration timed out");
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.x; use isolated XDG data"]
async fn real_opencode_cold_server_connects_before_chat_start() {
    let f = Fixture::new(Default::default()).await;
    let backend = del_opencode::OpenCodeBackend::default();
    backend.set_browser_tools(f.handle.chat_browser_tools().unwrap());
    let config = SessionConfig {
        provider: Provider::OpenCode,
        cwd: f._dir.path().into(),
        model: Some("opencode/big-pickle".into()),
        reasoning_effort: None,
        service_tier: None,
        permission_mode: del_agent::PermissionMode::FullAuto,
    };
    let first = backend.start_session(config.clone()).await.unwrap();
    let second = backend.start_session(config).await.unwrap();
    assert_ne!(first.id, second.id);
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn chat_spawn_creates_four_project_peers_with_tracked_first_requests() {
    let f = Fixture::new(Default::default()).await;
    f.manager.register_account(
        "selected".into(),
        Arc::new(MockBackend::new(Provider::Codex)),
    );
    let parent = f
        .manager
        .start_account_session(
            SessionConfig {
                cwd: f._dir.path().into(),
                model: Some("chosen-model".into()),
                reasoning_effort: Some("high".into()),
                service_tier: None,
                provider: Provider::Codex,
                permission_mode: del_agent::PermissionMode::Plan,
            },
            Some("selected".into()),
        )
        .await
        .unwrap()
        .id;
    let tools = f.handle.chat_browser_tools().unwrap();
    let config = LaunchConfig {
        args: vec![],
        env: vec![
            ("DEL_MCP_URL".into(), tools.url.clone()),
            (
                "DEL_MCP_TOKEN".into(),
                tools.credentials.mint(&parent).unwrap(),
            ),
        ],
        files: vec![],
    };
    let outside = tempfile::tempdir().unwrap();
    let before = f.manager.list_sessions().len();
    let denied = call(
        &config,
        "spawn_session",
        json!({"provider":"codex", "cwd":outside.path(), "prompt":"outside"}),
    )
    .await;
    assert_eq!(denied["isError"], true);
    assert_eq!(f.manager.list_sessions().len(), before);
    #[cfg(unix)]
    {
        let escape = f._dir.path().join("outside-link");
        std::os::unix::fs::symlink(outside.path(), &escape).unwrap();
        let denied = call(
            &config,
            "spawn_session",
            json!({"provider":"codex", "cwd":escape, "prompt":"outside"}),
        )
        .await;
        assert_eq!(denied["isError"], true);
        assert_eq!(f.manager.list_sessions().len(), before);
    }
    let mut ids = std::collections::HashSet::new();
    for n in 0..4 {
        let result = call(
            &config,
            "spawn_session",
            json!({"provider":"codex", "cwd":f._dir.path(), "prompt":format!("Task {n}")}),
        )
        .await;
        assert_eq!(result["isError"], false, "{result}");
        let info = &result["structuredContent"];
        assert_eq!(info["mode"], "chat");
        let id = SessionId::new(info["id"].as_str().unwrap());
        assert!(ids.insert(id.clone()));
        let record = f.manager.session_record(&id).unwrap();
        assert_eq!(record.title, Some(format!("Task {n}")));
        assert_eq!(record.account_id.as_deref(), Some("selected"));
        assert_eq!(record.model.as_deref(), Some("chosen-model"));
        assert_eq!(record.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(record.permission_mode, del_agent::PermissionMode::Plan);
        f.manager.authorize_peer(&parent, &id).unwrap();
        let messages = f.manager.peer_messages(&id);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from, parent);
        assert_eq!(messages[0].text, format!("Task {n}"));
        assert!(messages[0].reply_to.is_none());
        assert!(messages[0].prompt().contains("Reply with reply_to_session"));
    }
    assert_eq!(f.manager.peer_messages(&parent).len(), 4);
    assert!(f.manager.control_peer_work(&parent, "complete").is_err());
    f.manager.control_peer_work(&parent, "wait").unwrap();
    f.bridge.shutdown();
}

#[tokio::test]
async fn chat_spawn_cross_provider_keeps_permissions_and_project_without_model_leak() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("worktree");
    std::fs::create_dir(&workspace).unwrap();
    let manager = SessionManager::new(
        dir.path().join("index.jsonl"),
        [
            Arc::new(MockBackend::new(Provider::Codex)) as Arc<dyn AgentBackend>,
            Arc::new(MockBackend::new(Provider::Claude)) as Arc<dyn AgentBackend>,
        ],
    )
    .unwrap();
    let parent = manager
        .start_session(SessionConfig {
            cwd: workspace.clone(),
            provider: Provider::Codex,
            model: Some("codex-only-model".into()),
            reasoning_effort: Some("high".into()),
            service_tier: Some("priority".into()),
            permission_mode: del_agent::PermissionMode::Plan,
        })
        .await
        .unwrap()
        .id;
    manager.set_peer_project(&parent, dir.path()).unwrap();
    let child = manager
        .spawn_peer_session(&parent, Provider::Claude, &workspace, "Answer back")
        .await
        .unwrap();
    let record = manager.session_record(&child.id).unwrap();
    assert_eq!(record.provider, Provider::Claude);
    assert_eq!(record.model, None);
    assert_eq!(record.reasoning_effort, None);
    assert_eq!(record.service_tier, None);
    assert_eq!(record.account_id, None);
    assert_eq!(record.permission_mode, del_agent::PermissionMode::Plan);
    manager.authorize_peer(&parent, &child.id).unwrap();
    let count = manager.list_sessions().len();
    assert!(
        manager
            .spawn_peer_session(
                &parent,
                Provider::OpenCode,
                &workspace,
                "Unavailable provider"
            )
            .await
            .is_err()
    );
    assert_eq!(manager.list_sessions().len(), count);
    let request = manager.peer_messages(&child.id).pop().unwrap();
    // The first prompt uses the ordinary mailbox: a real reply wakes the caller,
    // and the receipt plus inherited project survive a manager restart.
    manager.control_peer_work(&parent, "wait").unwrap();
    manager
        .reply_to_peer(&child.id, &request.id, "answer", "done")
        .unwrap();
    drop(manager);
    let restored = SessionManager::new(
        dir.path().join("index.jsonl"),
        [
            Arc::new(MockBackend::new(Provider::Codex)) as Arc<dyn AgentBackend>,
            Arc::new(MockBackend::new(Provider::Claude)) as Arc<dyn AgentBackend>,
        ],
    )
    .unwrap();
    restored.authorize_peer(&parent, &child.id).unwrap();
    assert_eq!(restored.peer_messages(&parent).len(), 2);
    assert_eq!(
        restored.peer_messages(&parent)[1].reply_to.as_deref(),
        Some(request.id.as_str())
    );
}
