//! Streamable-HTTP MCP server: `POST /mcp` JSON-RPC, loopback only, one bearer
//! token per registered session.

use crate::host::*;
use crate::tools::{self, *};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use del_agent::SessionId;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

pub const SUPPORTED_PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_BODY_BYTES: usize = 1 << 20;

#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// How long `ask_user` waits for the user before failing the tool call.
    pub ask_timeout: Duration,
    /// Sessions one session may spawn.
    pub max_spawn_per_session: usize,
    /// Sessions the whole server will spawn.
    pub max_spawn_total: usize,
    /// Log one stderr line per JSON-RPC request (method, session). Never logs tokens or arguments.
    pub trace: bool,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            ask_timeout: Duration::from_secs(30 * 60),
            max_spawn_per_session: 8,
            max_spawn_total: 32,
            trace: false,
        }
    }
}

/// What a launcher needs to point an agent at the server. The token grants
/// access as exactly one session; keep it out of logs and argv.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub session_id: SessionId,
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("session_id", &self.session_id)
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Default)]
struct Scope {
    /// token -> session it authenticates as.
    tokens: HashMap<String, SessionId>,
    browser_only: HashSet<SessionId>,
    children: HashMap<SessionId, HashSet<SessionId>>,
    parent: HashMap<SessionId, SessionId>,
    spawned_total: usize,
    /// Spawn quota used per session (reserved before the host call).
    spawned_by: HashMap<SessionId, usize>,
}

struct Inner {
    host: Arc<dyn McpHost>,
    opts: ServerOptions,
    scope: Mutex<Scope>,
    port: u16,
}

pub struct McpServer {
    inner: Arc<Inner>,
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS randomness unavailable");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

impl McpServer {
    /// Bind `127.0.0.1:0` and start serving.
    pub async fn start(host: Arc<dyn McpHost>, opts: ServerOptions) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let inner = Arc::new(Inner {
            host,
            opts,
            scope: Mutex::new(Scope::default()),
            port: addr.port(),
        });
        let app = Router::new()
            .route(
                "/mcp",
                post(handle_post)
                    .get(method_not_allowed)
                    .delete(method_not_allowed),
            )
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .with_state(inner.clone());
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self {
            inner,
            addr,
            shutdown: Some(tx),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.addr.port())
    }

    /// Mint a fresh token for `session`. Existing tokens for it stay valid;
    /// call [`revoke_session`](Self::revoke_session) when the session ends.
    pub fn register_session(&self, session: &SessionId) -> Credentials {
        let token = random_token();
        self.inner
            .scope
            .lock()
            .unwrap()
            .tokens
            .insert(token.clone(), session.clone());
        Credentials {
            session_id: session.clone(),
            url: self.url(),
            token,
        }
    }

    /// Invalidate every token of `session`.
    pub fn register_browser_client(&self, session: &SessionId) -> Credentials {
        self.inner
            .scope
            .lock()
            .unwrap()
            .browser_only
            .insert(session.clone());
        self.register_session(session)
    }

    pub fn revoke_session(&self, session: &SessionId) {
        let mut s = self.inner.scope.lock().unwrap();
        s.tokens.retain(|_, v| v != session);
    }

    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn method_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

fn host_is_loopback(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Rejects DNS-rebinding and browser cross-origin calls.
fn origin_ok(h: &HeaderMap, port: u16) -> bool {
    match h.get(header::HOST).and_then(|v| v.to_str().ok()) {
        Some(host) if host_is_loopback(host) => {
            // Host must also carry our port when it carries one.
            if let Some((_, p)) = host.rsplit_once(':')
                && !host.ends_with(']')
                && p.parse::<u16>().ok() != Some(port)
            {
                return false;
            }
        }
        _ => return false,
    }
    match h.get(header::ORIGIN) {
        None => true,
        Some(o) => o
            .to_str()
            .ok()
            .and_then(|o| o.split_once("://").map(|(_, a)| a))
            .is_some_and(host_is_loopback),
    }
}

fn caller_of(inner: &Inner, h: &HeaderMap) -> Option<SessionId> {
    let tok = h
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let scope = inner.scope.lock().unwrap();
    // Compare against every token so timing does not reveal a prefix match.
    let mut found = None;
    for (t, s) in &scope.tokens {
        if ct_eq(t, tok) {
            found = Some(s.clone());
        }
    }
    found
}

async fn handle_post(State(inner): State<Arc<Inner>>, headers: HeaderMap, body: Bytes) -> Response {
    if !origin_ok(&headers, inner.port) {
        return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
    }
    let Some(caller) = caller_of(&inner, &headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or invalid bearer token",
        )
            .into_response();
    };
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return json_response(rpc_error(Value::Null, -32700, "parse error")),
    };
    match inner.dispatch(&caller, msg).await {
        Some(resp) => json_response(resp),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn json_response(v: Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_text(text: String, structured: Option<Value>, is_error: bool) -> Value {
    let mut v = json!({"content": [{"type": "text", "text": text}], "isError": is_error});
    if let Some(s) = structured {
        v["structuredContent"] = s;
    }
    v
}

fn tool_err(text: impl Into<String>) -> Value {
    tool_text(text.into(), None, true)
}

fn tool_json(v: Value) -> Value {
    tool_text(v.to_string(), Some(v), false)
}

impl Inner {
    /// Handle one JSON-RPC message. `None` means "no response" (notification).
    async fn dispatch(&self, caller: &SessionId, msg: Value) -> Option<Value> {
        if msg.is_array() {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "batch requests are not supported",
            ));
        }
        let id = msg.get("id").cloned();
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || id
                .as_ref()
                .is_some_and(|v| !v.is_string() && !v.is_number() && !v.is_null())
            || msg.get("method").and_then(Value::as_str).is_none()
            || msg.get("params").is_some_and(|v| !v.is_object())
        {
            return Some(rpc_error(Value::Null, -32600, "invalid request"));
        }
        let method = msg["method"].as_str().expect("validated method");
        let Some(id) = id else {
            return None; // notification (e.g. notifications/initialized)
        };
        if self.opts.trace {
            eprintln!("del-mcp: {caller} -> {method}");
        }
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let want = params.get("protocolVersion").and_then(Value::as_str);
                let version = want
                    .filter(|v| SUPPORTED_PROTOCOLS.contains(v))
                    .unwrap_or(SUPPORTED_PROTOCOLS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "del", "title": "del", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": format!(
                        "You are connected to Del as `{caller}`. Use only tools in tools/list. {}",
                        del_agent::backend::BROWSER_INSTRUCTIONS
                    )
                })
            }
            "ping" => json!({}),
            "tools/list" => {
                let mut catalogue = tools::tool_list();
                if self.scope.lock().unwrap().browser_only.contains(caller) {
                    catalogue
                        .as_array_mut()
                        .unwrap()
                        .retain(|tool| tool["name"] == "browser");
                }
                json!({"tools":catalogue})
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match self.call_tool(caller, name, args).await {
                    Ok(v) => v,
                    Err(ToolFail::UnknownTool) => {
                        return Some(rpc_error(id, -32602, &format!("unknown tool `{name}`")));
                    }
                    Err(ToolFail::Invalid(m)) => {
                        return Some(rpc_error(id, -32602, &m));
                    }
                }
            }
            _ => return Some(rpc_error(id, -32601, "method not found")),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    async fn call_tool(
        &self,
        caller: &SessionId,
        name: &str,
        args: Value,
    ) -> Result<Value, ToolFail> {
        fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolFail> {
            if !args.is_object() {
                return Err(ToolFail::Invalid("arguments must be an object".into()));
            }
            serde_json::from_value(args)
                .map_err(|e| ToolFail::Invalid(format!("invalid arguments: {e}")))
        }
        let host = &self.host;
        if name != "browser" && self.scope.lock().unwrap().browser_only.contains(caller) {
            return Ok(tool_err("This credential grants browser tools only"));
        }
        match name {
            "browser" => {
                let request: crate::BrowserRequest = parse(args)?;
                if let Err(error) = request.validate() {
                    return Ok(tool_err(error));
                }
                Ok(
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        host.browser(caller, request),
                    )
                    .await
                    {
                        Ok(Ok(value)) => {
                            if let Some(image) = value.get("image").and_then(Value::as_str) {
                                json!({"content":[{"type":"image","data":image,"mimeType":"image/png"}],"isError":false})
                            } else {
                                tool_text(value.to_string(), Some(value), false)
                            }
                        }
                        Ok(Err(error)) => tool_err(error.to_string()),
                        Err(_) => tool_err("Browser action timed out"),
                    },
                )
            }
            "report_status" => {
                let a: ReportStatusArgs = parse(args)?;
                if let Some(s) = &a.session_id
                    && s != caller.as_str()
                {
                    return Ok(tool_err("session_id must be your own session"));
                }
                if let Err(e) = non_empty_max("summary", &a.summary, MAX_SUMMARY) {
                    return Ok(tool_err(e));
                }
                Ok(
                    match host.report_status(caller, a.state, &a.summary).await {
                        Ok(()) => tool_text("status recorded".into(), None, false),
                        Err(e) => tool_err(e.to_string()),
                    },
                )
            }
            "ask_user" => {
                let a: AskUserArgs = parse(args)?;
                if let Err(e) = non_empty_max("question", &a.question, MAX_QUESTION) {
                    return Ok(tool_err(e));
                }
                if a.options.len() > MAX_OPTIONS
                    || a.options
                        .iter()
                        .any(|o| non_empty_max("option", o, MAX_OPTION).is_err())
                {
                    return Ok(tool_err(format!(
                        "options: at most {MAX_OPTIONS}, each 1-{MAX_OPTION} characters"
                    )));
                }
                let fut = host.ask_user(caller, &a.question, &a.options);
                Ok(
                    match tokio::time::timeout(self.opts.ask_timeout, fut).await {
                        Err(_) => tool_err("the user did not answer in time"),
                        Ok(Err(e)) => tool_err(e.to_string()),
                        Ok(Ok(UserAnswer::Dismissed)) => {
                            tool_json(json!({"answered": false, "dismissed": true}))
                        }
                        Ok(Ok(UserAnswer::Option { index, text })) => tool_json(
                            json!({"answered": true, "answer": text, "option_index": index}),
                        ),
                        Ok(Ok(UserAnswer::Text { text })) => {
                            tool_json(json!({"answered": true, "answer": text}))
                        }
                    },
                )
            }
            "list_sessions" => {
                let _: ListSessionsArgs = parse(args)?;
                Ok(match host.list_sessions(caller).await {
                    Ok(list) => {
                        let rows: Vec<Value> = list
                            .iter()
                            .map(|s| {
                                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                                v["is_self"] = json!(&s.id == caller);
                                v
                            })
                            .collect();
                        tool_json(json!({"sessions": rows}))
                    }
                    Err(e) => tool_err(e.to_string()),
                })
            }
            "get_session_summary" => {
                let a: SessionArg = parse(args)?;
                let target = SessionId::new(a.session_id);
                Ok(match host.session_summary(caller, &target).await {
                    Ok(s) => tool_json(serde_json::to_value(s).unwrap_or(Value::Null)),
                    Err(e) => tool_err(e.to_string()),
                })
            }
            "spawn_session" => {
                let a: SpawnArgs = parse(args)?;
                if let Err(e) = non_empty_max("prompt", &a.prompt, MAX_PROMPT) {
                    return Ok(tool_err(e));
                }
                let cwd = Path::new(&a.cwd);
                if !cwd.is_absolute() || !cwd.is_dir() {
                    return Ok(tool_err(
                        "cwd must be an absolute path of an existing directory",
                    ));
                }
                // Reserve the quota up front so concurrent calls cannot overshoot it.
                {
                    let mut s = self.scope.lock().unwrap();
                    let mine = s.spawned_by.get(caller).copied().unwrap_or(0);
                    if mine >= self.opts.max_spawn_per_session
                        || s.spawned_total >= self.opts.max_spawn_total
                    {
                        return Ok(tool_err("spawn limit reached"));
                    }
                    s.spawned_total += 1;
                    *s.spawned_by.entry(caller.clone()).or_default() += 1;
                }
                let req = SpawnRequest {
                    provider: a.provider,
                    cwd: cwd.to_path_buf(),
                    prompt: a.prompt,
                };
                match host.spawn_session(caller, req).await {
                    Ok(info) => {
                        let mut s = self.scope.lock().unwrap();
                        s.children
                            .entry(caller.clone())
                            .or_default()
                            .insert(info.id.clone());
                        s.parent.insert(info.id.clone(), caller.clone());
                        drop(s);
                        Ok(tool_json(serde_json::to_value(info).unwrap_or(Value::Null)))
                    }
                    Err(e) => {
                        let mut s = self.scope.lock().unwrap();
                        s.spawned_total -= 1;
                        if let Some(n) = s.spawned_by.get_mut(caller) {
                            *n -= 1;
                        }
                        drop(s);
                        Ok(tool_err(e.to_string()))
                    }
                }
            }
            "send_to_session" => {
                let a: SendArgs = parse(args)?;
                if let Err(e) = non_empty_max("text", &a.text, MAX_TEXT) {
                    return Ok(tool_err(e));
                }
                let target = SessionId::new(a.session_id);
                let allowed = {
                    let s = self.scope.lock().unwrap();
                    s.children.get(caller).is_some_and(|c| c.contains(&target))
                        || s.parent.get(caller) == Some(&target)
                };
                if !allowed {
                    return Ok(tool_err(
                        "you can only message sessions you spawned and the session that spawned you",
                    ));
                }
                Ok(match host.send_to_session(caller, &target, &a.text).await {
                    Ok(()) => tool_text("sent".into(), None, false),
                    Err(e) => tool_err(e.to_string()),
                })
            }
            "notify" => {
                let a: NotifyArgs = parse(args)?;
                if let Err(e) = non_empty_max("title", &a.title, MAX_TITLE) {
                    return Ok(tool_err(e));
                }
                if a.body.chars().count() > MAX_BODY {
                    return Ok(tool_err(format!(
                        "body is longer than {MAX_BODY} characters"
                    )));
                }
                Ok(match host.notify(caller, &a.title, &a.body).await {
                    Ok(()) => tool_text("notified".into(), None, false),
                    Err(e) => tool_err(e.to_string()),
                })
            }
            _ => Err(ToolFail::UnknownTool),
        }
    }
}

enum ToolFail {
    UnknownTool,
    Invalid(String),
}
