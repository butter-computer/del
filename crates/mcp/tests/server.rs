use del_agent::{MockBackend, Provider, SessionId, SessionManager};
use del_mcp::launch::{self, Transport};
use del_mcp::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Env {
    host: Arc<MockHost>,
    server: McpServer,
    me: Credentials,
}

async fn env_with(opts: ServerOptions) -> Env {
    let host = Arc::new(MockHost::new());
    host.add_session("me", Provider::Claude);
    let server = McpServer::start(host.clone(), opts).await.unwrap();
    let me = server.register_session(&SessionId::new("me"));
    Env { host, server, me }
}

async fn env() -> Env {
    env_with(ServerOptions::default()).await
}

/// Raw POST; returns (status, body).
async fn post(url: &str, headers: Vec<(String, String)>, body: Value) -> (u16, String) {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent.post(&url);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        let mut resp = req.send(body.to_string()).unwrap();
        (
            resp.status().as_u16(),
            resp.body_mut().read_to_string().unwrap(),
        )
    })
    .await
    .unwrap()
}

fn auth(c: &Credentials) -> Vec<(String, String)> {
    vec![("Authorization".into(), format!("Bearer {}", c.token))]
}

async fn rpc(c: &Credentials, method: &str, params: Value) -> Value {
    let (s, b) = post(
        &c.url,
        auth(c),
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
    )
    .await;
    assert_eq!(s, 200, "{b}");
    serde_json::from_str(&b).unwrap()
}

async fn call(c: &Credentials, tool: &str, args: Value) -> Value {
    rpc(c, "tools/call", json!({"name": tool, "arguments": args})).await
}

fn is_error(v: &Value) -> bool {
    v["result"]["isError"].as_bool().unwrap_or(false)
}

#[tokio::test]
async fn rejects_missing_wrong_and_revoked_tokens() {
    let e = env().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    assert_eq!(post(&e.me.url, vec![], body.clone()).await.0, 401);
    let bad = vec![("Authorization".to_string(), "Bearer nope".to_string())];
    assert_eq!(post(&e.me.url, bad, body.clone()).await.0, 401);
    assert_eq!(post(&e.me.url, auth(&e.me), body.clone()).await.0, 200);
    e.server.revoke_session(&SessionId::new("me"));
    assert_eq!(post(&e.me.url, auth(&e.me), body).await.0, 401);
}

#[tokio::test]
async fn binds_loopback_and_rejects_foreign_origin_and_host() {
    let e = env().await;
    assert!(e.server.addr().ip().is_loopback());
    let body = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    let mut h = auth(&e.me);
    h.push(("Origin".into(), "https://evil.example".into()));
    assert_eq!(post(&e.me.url, h, body.clone()).await.0, 403);
    // Another loopback port (a local dev server page) is a foreign origin.
    let mut h = auth(&e.me);
    h.push(("Origin".into(), "http://localhost:3000".into()));
    assert_eq!(post(&e.me.url, h, body.clone()).await.0, 403);
    let mut h = auth(&e.me);
    h.push(("Origin".into(), "http://localhost".into()));
    assert_eq!(post(&e.me.url, h, body.clone()).await.0, 403);
    let port = e.server.addr().port();
    for origin in [
        format!("http://localhost:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ] {
        let mut h = auth(&e.me);
        h.push(("Origin".into(), origin.clone()));
        assert_eq!(post(&e.me.url, h, body.clone()).await.0, 200, "{origin}");
    }
    let mut h = auth(&e.me);
    h.push(("Host".into(), "evil.example".into()));
    assert_eq!(post(&e.me.url, h, body).await.0, 403);
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let e = env().await;
    let r = rpc(&e.me, "initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}})).await;
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert!(
        r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("`me`")
    );
    assert!(
        r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains(del_agent::backend::BROWSER_INSTRUCTIONS)
    );
    let r = rpc(&e.me, "initialize", json!({"protocolVersion":"1999-01-01"})).await;
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");

    let r = rpc(&e.me, "tools/list", json!({})).await;
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "browser",
            "report_status",
            "ask_user",
            "list_sessions",
            "get_session_summary",
            "spawn_session",
            "send_to_session",
            "notify"
        ]
    );
    for t in tools {
        assert!(!t["description"].as_str().unwrap().is_empty());
        assert_eq!(t["inputSchema"]["type"], "object");
    }
    // Notifications get 202 and no body.
    let (s, b) = post(
        &e.me.url,
        auth(&e.me),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    assert_eq!((s, b.as_str()), (202, ""));
    let r = rpc(&e.me, "nope/nope", json!({})).await;
    assert_eq!(r["error"]["code"], -32601);
}

#[tokio::test]
async fn report_status_and_scoping() {
    let e = env().await;
    let r = call(
        &e.me,
        "report_status",
        json!({"state":"working","summary":"parsing"}),
    )
    .await;
    assert!(!is_error(&r));
    let r = call(
        &e.me,
        "report_status",
        json!({"session_id":"me","state":"done","summary":"ok"}),
    )
    .await;
    assert!(!is_error(&r));
    let r = call(
        &e.me,
        "report_status",
        json!({"session_id":"other","state":"done","summary":"ok"}),
    )
    .await;
    assert!(is_error(&r));
    // bad enum is a protocol-level invalid-params error
    let r = call(
        &e.me,
        "report_status",
        json!({"state":"dancing","summary":"x"}),
    )
    .await;
    assert_eq!(r["error"]["code"], -32602);
    let reports = e.host.reports();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].0.as_str(), "me");
}

#[tokio::test]
async fn ask_user_blocks_until_answered() {
    let e = env().await;
    let c = e.me.clone();
    let h = tokio::spawn(async move {
        call(
            &c,
            "ask_user",
            json!({"question":"Deploy?","options":["yes","no"]}),
        )
        .await
    });
    let q = e.host.take_question().await;
    assert_eq!(q.question, "Deploy?");
    assert_eq!(q.options, ["yes", "no"]);
    assert_eq!(q.caller.as_str(), "me");
    assert!(!h.is_finished());
    q.answer(UserAnswer::Option {
        index: 1,
        text: "no".into(),
    });
    let r = h.await.unwrap();
    assert_eq!(r["result"]["structuredContent"]["answer"], "no");
    assert_eq!(r["result"]["structuredContent"]["option_index"], 1);
}

#[tokio::test]
async fn ask_user_times_out() {
    let e = env_with(ServerOptions {
        ask_timeout: Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let r = call(&e.me, "ask_user", json!({"question":"hello?"})).await;
    assert!(is_error(&r));
}

#[tokio::test]
async fn ask_user_validates_input() {
    let e = env().await;
    assert!(is_error(
        &call(&e.me, "ask_user", json!({"question":"  "})).await
    ));
    let many: Vec<_> = (0..9).map(|i| i.to_string()).collect();
    assert!(is_error(
        &call(&e.me, "ask_user", json!({"question":"q","options":many})).await
    ));
}

#[tokio::test]
async fn list_and_summary() {
    let e = env().await;
    e.host.add_session("other", Provider::Codex);
    let r = call(&e.me, "list_sessions", json!({})).await;
    // Sessions outside the caller's spawn family are neither listed nor readable.
    let rows = r["result"]["structuredContent"]["sessions"]
        .as_array()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["is_self"], true);
    assert!(is_error(
        &call(&e.me, "get_session_summary", json!({"session_id":"other"})).await
    ));
    assert!(is_error(
        &call(&e.me, "get_session_summary", json!({"session_id":"ghost"})).await
    ));
    let r = call(&e.me, "get_session_summary", json!({"session_id":"me"})).await;
    assert_eq!(r["result"]["structuredContent"]["last_output"], "output of me");
}

#[tokio::test]
async fn spawn_then_send_is_scoped_to_family() {
    let e = env().await;
    e.host.add_session("stranger", Provider::Codex);
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();

    // Not spawned by me -> refused, and nothing reaches the host.
    let r = call(
        &e.me,
        "send_to_session",
        json!({"session_id":"stranger","text":"hi"}),
    )
    .await;
    assert!(is_error(&r));
    assert!(e.host.sent().is_empty());

    assert!(is_error(
        &call(
            &e.me,
            "spawn_session",
            json!({"provider":"codex","cwd":"relative","prompt":"x"})
        )
        .await
    ));
    assert!(is_error(
        &call(
            &e.me,
            "spawn_session",
            json!({"provider":"codex","cwd":"/definitely/not/here","prompt":"x"})
        )
        .await
    ));

    let r = call(
        &e.me,
        "spawn_session",
        json!({"provider":"codex","cwd":cwd,"prompt":"do it"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let child = r["result"]["structuredContent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(e.host.spawned()[0].prompt, "do it");

    let r = call(
        &e.me,
        "send_to_session",
        json!({"session_id":child,"text":"more"}),
    )
    .await;
    assert!(!is_error(&r));
    assert_eq!(e.host.sent()[0].2, "more");

    // The child may talk back to its parent, but not to the stranger.
    let child_creds = e.server.register_session(&SessionId::new(child));
    assert!(!is_error(
        &call(
            &child_creds,
            "send_to_session",
            json!({"session_id":"me","text":"done"})
        )
        .await
    ));
    assert!(is_error(
        &call(
            &child_creds,
            "send_to_session",
            json!({"session_id":"stranger","text":"x"})
        )
        .await
    ));
}

#[tokio::test]
async fn spawn_limit_enforced() {
    let e = env_with(ServerOptions {
        max_spawn_per_session: 2,
        ..Default::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let args = json!({"provider":"claude","cwd":dir.path(),"prompt":"x"});
    let mut children = Vec::new();
    for _ in 0..2 {
        let r = call(&e.me, "spawn_session", args.clone()).await;
        assert!(!is_error(&r));
        // The launcher registers each live child.
        let child = SessionId::new(r["result"]["structuredContent"]["id"].as_str().unwrap());
        e.server.register_session(&child);
        children.push(child);
    }
    assert!(is_error(&call(&e.me, "spawn_session", args.clone()).await));
    // An exited (revoked) child frees its slot; quotas count live children.
    e.server.revoke_session(&children[0]);
    let r = call(&e.me, "spawn_session", args.clone()).await;
    assert!(!is_error(&r));
    let third = SessionId::new(r["result"]["structuredContent"]["id"].as_str().unwrap());
    e.server.register_session(&third);
    assert!(is_error(&call(&e.me, "spawn_session", args).await));
}

#[tokio::test]
async fn forgotten_sessions_lose_family_access() {
    let e = env().await;
    let dir = tempfile::tempdir().unwrap();
    let r = call(
        &e.me,
        "spawn_session",
        json!({"provider":"codex","cwd":dir.path(),"prompt":"x"}),
    )
    .await;
    let child = SessionId::new(r["result"]["structuredContent"]["id"].as_str().unwrap());
    let child_creds = e.server.register_session(&child);
    let rows = call(&e.me, "list_sessions", json!({})).await["result"]["structuredContent"]
        ["sessions"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(rows, 2);
    assert!(!is_error(
        &call(&child_creds, "get_session_summary", json!({"session_id":"me"})).await
    ));
    e.server.forget_session(&child);
    let rows = call(&e.me, "list_sessions", json!({})).await["result"]["structuredContent"]
        ["sessions"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(rows, 1);
    assert!(is_error(
        &call(
            &e.me,
            "send_to_session",
            json!({"session_id":child.as_str(),"text":"x"})
        )
        .await
    ));
}

#[tokio::test]
async fn browser_targets_are_limited_to_the_spawn_family() {
    let e = env().await;
    e.host.add_session("other", Provider::Codex);
    let r = call(
        &e.me,
        "browser",
        json!({"action":"snapshot","session_id":"other"}),
    )
    .await;
    assert!(is_error(&r));
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("session_id must be your own session")
    );
    // Own session passes the server check and reaches the host.
    let r = call(&e.me, "browser", json!({"action":"snapshot","session_id":"me"})).await;
    assert_eq!(
        r["result"]["content"][0]["text"],
        "This host has no browser"
    );
}

#[tokio::test]
async fn notify_reaches_host() {
    let e = env().await;
    assert!(!is_error(
        &call(&e.me, "notify", json!({"title":"Done","body":"all green"})).await
    ));
    assert_eq!(e.host.notifications()[0].1, "Done");
    assert_eq!(
        call(&e.me, "nope", json!({})).await["error"]["code"],
        -32602
    );
}

#[tokio::test]
async fn stdio_proxy_round_trip() {
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let e = env().await;
    let out = Buf::default();
    let (url, token, o) = (e.me.url.clone(), e.me.token.clone(), out.clone());
    let input = "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/list\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
    tokio::task::spawn_blocking(move || del_mcp::stdio::run(&url, &token, input.as_bytes(), o))
        .await
        .unwrap();
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    let v: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 8);

    // Bad token surfaces as a JSON-RPC error for requests.
    let out = Buf::default();
    let (url, o) = (e.me.url.clone(), out.clone());
    tokio::task::spawn_blocking(move || {
        del_mcp::stdio::run(
            &url,
            "bad",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".as_bytes(),
            o,
        )
    })
    .await
    .unwrap();
    let v: Value = serde_json::from_slice(&out.0.lock().unwrap()).unwrap();
    assert!(v["error"]["message"].as_str().unwrap().contains("401"));
}

#[test]
fn launch_configs() {
    let c = Credentials {
        session_id: SessionId::new("s/1"),
        url: "http://127.0.0.1:1234/mcp".into(),
        token: "secret".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let l = launch::claude(&c, &Transport::Http, dir.path()).unwrap();
    assert_eq!(l.args[0], "--mcp-config");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&l.args[1]).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["del"]["url"], "http://127.0.0.1:1234/mcp");
    assert_eq!(
        v["mcpServers"]["del"]["headers"]["Authorization"],
        "Bearer secret"
    );
    assert!(
        l.files[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("s_1")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&l.files[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let l = launch::codex(&c, &Transport::Http);
    assert_eq!(
        l.args,
        [
            "-c",
            "mcp_servers.del.url=\"http://127.0.0.1:1234/mcp\"",
            "-c",
            "mcp_servers.del.bearer_token_env_var=\"DEL_MCP_TOKEN\"",
            "-c",
            "mcp_servers.del.tool_timeout_sec=1860"
        ]
    );
    assert!(l.env.contains(&("DEL_MCP_TOKEN".into(), "secret".into())));
    assert!(!l.args.iter().any(|a| a.contains("secret")));
}

struct AutoUi;
#[async_trait::async_trait]
impl McpUi for AutoUi {
    async fn ask_user(&self, _: &SessionId, _: &str, _: &[String]) -> UserAnswer {
        UserAnswer::Text { text: "42".into() }
    }
    fn status_reported(&self, _: &SessionId, _: ReportedState, _: &str) {}
    fn notify(&self, _: &SessionId, _: &str, _: &str) {}
}

#[tokio::test]
async fn manager_host_drives_real_session_manager() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(
        dir.path().join("s.jsonl"),
        [Arc::new(MockBackend::new(Provider::Claude)) as Arc<dyn del_agent::AgentBackend>],
    )
    .unwrap();
    let me = mgr
        .start_session(del_agent::SessionConfig {
            cwd: dir.path().into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Claude,
            permission_mode: Default::default(),
        })
        .await
        .unwrap();
    let server = McpServer::start(
        Arc::new(ManagerHost::new(mgr.clone(), Arc::new(AutoUi))),
        Default::default(),
    )
    .await
    .unwrap();
    let c = server.register_session(&me.id);

    let r = call(
        &c,
        "spawn_session",
        json!({"provider":"claude","cwd":dir.path(),"prompt":"hello there"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let child = r["result"]["structuredContent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(mgr.is_live(&SessionId::new(child.clone())));

    let r = call(&c, "list_sessions", json!({})).await;
    assert_eq!(
        r["result"]["structuredContent"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    call(
        &c,
        "report_status",
        json!({"state":"working","summary":"checking"}),
    )
    .await;
    let r = call(
        &c,
        "get_session_summary",
        json!({"session_id": me.id.as_str()}),
    )
    .await;
    assert_eq!(
        r["result"]["structuredContent"]["reported_summary"],
        "checking"
    );
    assert_eq!(
        r["result"]["structuredContent"]["info"]["reported_state"],
        "working"
    );

    let r = call(&c, "ask_user", json!({"question":"?"})).await;
    assert_eq!(r["result"]["structuredContent"]["answer"], "42");
    let r = call(
        &c,
        "send_to_session",
        json!({"session_id": child, "text": "again"}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    mgr.shutdown().await;
}

#[tokio::test]
async fn rejects_malformed_requests_and_unknown_tool_fields() {
    let e = env().await;
    for body in [
        json!({}),
        json!(42),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":{},"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"method":"ping","params":false}),
    ] {
        let (status, body) = post(&e.me.url, auth(&e.me), body).await;
        assert_eq!(status, 200);
        let response: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["error"]["code"], -32600);
    }
    for (tool, args) in [
        ("list_sessions", json!({"extra":true})),
        ("list_sessions", json!([])),
        ("notify", json!({"title":"hi","body":"there","extra":true})),
    ] {
        let response = call(&e.me, tool, args).await;
        assert_eq!(response["error"]["code"], -32602);
    }
    assert!(e.host.notifications().is_empty());
}

#[test]
fn launch_files_are_unique_and_debug_hides_tokens() {
    let c = Credentials {
        session_id: SessionId::new("same"),
        url: "http://127.0.0.1:1234/mcp".into(),
        token: "private-bearer-value".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let first = launch::claude(&c, &Transport::Http, dir.path()).unwrap();
    let mut rotated = c.clone();
    rotated.token = "rotated-private-bearer".into();
    let second = launch::claude(&rotated, &Transport::Http, dir.path()).unwrap();
    assert_ne!(first.files, second.files);
    let original = std::fs::read_to_string(&first.files[0]).unwrap();
    assert!(original.contains(&c.token));
    assert!(!original.contains(&rotated.token));
    for transport in [
        Transport::Http,
        Transport::Stdio {
            binary: "/tmp/del-mcp-stdio".into(),
        },
    ] {
        let config = launch::codex(&c, &transport);
        assert!(!format!("{config:?}").contains(&c.token));
    }
    assert!(!format!("{c:?}").contains(&c.token));
}

#[test]
fn launch_timeouts_cover_the_user_wait_without_process_wide_overrides() {
    let credentials = Credentials {
        session_id: SessionId::new("waiter"),
        url: "http://127.0.0.1:1234/mcp".into(),
        token: "test-only-token".into(),
    };
    let timeout_secs = ServerOptions::default().ask_timeout.as_secs() + 60;
    assert_eq!(launch::TOOL_TIMEOUT_SECS, timeout_secs);
    let dir = tempfile::tempdir().unwrap();
    for transport in [
        Transport::Http,
        Transport::Stdio {
            binary: "/tmp/del-mcp-stdio".into(),
        },
    ] {
        let claude = launch::claude(&credentials, &transport, dir.path()).unwrap();
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&claude.files[0]).unwrap()).unwrap();
        assert_eq!(
            document["mcpServers"]["del"]["timeout"],
            timeout_secs * 1_000
        );
        assert_eq!(document["mcpServers"].as_object().unwrap().len(), 1);
        assert!(
            claude.env.is_empty(),
            "do not change timeouts for other configured servers"
        );
        assert!(!claude.args.iter().any(|arg| arg == "--strict-mcp-config"));
        let codex = launch::codex(&credentials, &transport);
        assert!(codex.args.windows(2).any(|args| args
            == [
                "-c",
                &format!("mcp_servers.del.tool_timeout_sec={timeout_secs}")
            ]));
        assert!(
            codex
                .env
                .iter()
                .all(|(name, _)| name == launch::ENV_URL || name == launch::ENV_TOKEN)
        );
    }
}

#[tokio::test]
async fn chat_browser_credentials_cannot_call_session_or_notification_tools() {
    let e = env().await;
    let id = SessionId::new("browser-chat");
    let credential = e.server.register_browser_client(&id);
    let list = rpc(&credential, "tools/list", json!({})).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(list["result"]["tools"][0]["name"], "browser");
    assert!(is_error(
        &call(&credential, "list_sessions", json!({})).await
    ));
    assert!(is_error(
        &call(&credential, "notify", json!({"title":"no","body":"no"})).await
    ));
    assert!(is_error(
        &call(&credential, "browser", json!({"action":"not-real"})).await
    ));
    let invalid = call(
        &credential,
        "browser",
        json!({"action":"list_tabs","unexpected":true}),
    )
    .await;
    assert_eq!(invalid["error"]["code"], -32602);
    e.server.revoke_session(&id);
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    assert_eq!(post(&credential.url, auth(&credential), body).await.0, 401);
}

#[tokio::test]
async fn chat_credentials_expose_only_browser_and_own_goal() {
    let e = env().await;
    let chat = e.server.register_chat_client(&SessionId::new("chat"));
    let list = rpc(&chat, "tools/list", json!({})).await;
    let names: Vec<_> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["goal", "browser"]);
    assert!(is_error(
        &call(
            &chat,
            "spawn_session",
            json!({"provider":"claude","cwd":"/tmp","prompt":"hi"})
        )
        .await
    ));
    let target = call(
        &chat,
        "goal",
        json!({"action":"clear","session_id":"other"}),
    )
    .await;
    assert!(target.get("error").is_some());
    assert!(is_error(
        &call(&chat, "goal", json!({"action":"set","objective":" "})).await
    ));
    assert!(is_error(
        &call(&chat, "goal", json!({"action":"set","objective":"clear"})).await
    ));
    let browser = e.server.register_browser_client(&SessionId::new("browser"));
    assert!(is_error(
        &call(&browser, "goal", json!({"action":"get"})).await
    ));
}
