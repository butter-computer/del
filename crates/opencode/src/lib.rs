//! Native OpenCode adapter. The CLI owns its agent loop, skills, and transcripts.
mod forms;
pub mod history;
mod mapping;
mod server;
use async_trait::async_trait;
use del_agent::*;
use reqwest::Method;
use serde_json::{Value, json};
use server::Server;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

#[derive(Default)]
pub struct OpenCodeBackend {
    environment: Vec<(String, String)>,
    servers: AsyncMutex<HashMap<PathBuf, Arc<Server>>>,
    sessions: Mutex<HashMap<SessionId, Arc<Live>>>,
    tools: Mutex<Option<backend::BrowserTools>>,
}
struct Live {
    server: Arc<Server>,
    config: Mutex<SessionConfig>,
    events: broadcast::Sender<AgentEvent>,
    busy: AtomicBool,
    connected: AtomicBool,
    generation: AtomicU64,
    sequence: Mutex<Option<u64>>,
    advanced: tokio::sync::Notify,
    mapping: Mutex<mapping::Stream>,
    forms: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
/// Stream work queued for in-order delivery to subscribers.
enum Queued {
    /// A turn's file diff, fetched off the reader; delivered before the turn ends.
    Diffs(tokio::task::JoinHandle<Vec<AgentEvent>>),
    /// Mapped events and the durable sequence they advance to.
    Events(Vec<AgentEvent>, Option<u64>),
    Closed,
}
impl Live {
    fn stop_tasks(&self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        for (_, task) in self.forms.lock().unwrap().drain() {
            task.abort();
        }
    }
    fn emit(&self, events: impl IntoIterator<Item = AgentEvent>) {
        for event in events {
            let _ = self.events.send(event);
        }
    }
}
/// Percent-encode one URL path segment (provider ids are opaque strings).
pub(crate) fn seg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn err(message: impl std::fmt::Display) -> AgentError {
    AgentError::Backend(format!("OpenCode: {message}"))
}
impl OpenCodeBackend {
    pub fn with_environment(environment: Vec<(String, String)>) -> Self { { let mut backend = Self::default(); backend.environment = environment; backend } }
    async fn server(&self, cwd: &Path) -> Result<Arc<Server>> {
        let cwd = cwd.canonicalize()?;
        let mut servers = self.servers.lock().await;
        if let Some(server) = servers.get(&cwd) {
            if server.is_alive() {
                return Ok(server.clone());
            }
            // Crashed: start a fresh server rather than reuse a dead address.
            if let Some(server) = servers.remove(&cwd) {
                self.retire_browser(&server);
            }
        }
        // Browser tools are installed per session by `install_tools`.
        let server = Arc::new(Server::start(&cwd, &self.environment).await?);
        servers.insert(cwd, server.clone());
        Ok(server)
    }
    /// Revoke the browser credential of a server that is going away.
    fn retire_browser(&self, server: &Server) {
        let group = server.browser.lock().unwrap().take();
        if let (Some((group, _)), Some(tools)) = (group, self.tools.lock().unwrap().as_ref()) {
            tools.credentials.revoke(&group);
        }
    }
    async fn install_tools(&self, server: &Server) -> Result<()> {
        let tools = self.tools.lock().unwrap().clone();
        if let Some(tools) = tools {
            // OpenCode configures MCP per workspace server, not per session, so
            // its chats share one credential that reaches exactly those chats.
            let token = {
                let mut browser = server.browser.lock().unwrap();
                match browser.as_ref() {
                    Some((_, token)) => token.clone(),
                    None => {
                        let group = SessionId::new(format!(
                            "opencode-workspace-{}-{}",
                            std::process::id(),
                            uuid::Uuid::new_v4()
                        ));
                        let token = tools.credentials.mint_shared(&group).map_err(err)?;
                        *browser = Some((group, token.clone()));
                        token
                    }
                }
            };
            server.json(Method::PUT,"/api/experimental/mcp/del",Some(&json!({"config":{"type":"remote","url":tools.url,"headers":{"Authorization":format!("Bearer {token}")},"oauth":false}}))).await?;
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    let servers = server.json(Method::GET, "/api/mcp", None).await?;
                    let status = servers["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|s| s["name"] == "del")
                        .map(|s| &s["status"]);
                    match status.and_then(|s| s["status"].as_str()) {
                        Some("connected") => return Ok(()),
                        Some("pending") => {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await
                        }
                        _ => {
                            return Err(err(format!(
                                "Del MCP connection failed: {}",
                                status.unwrap_or(&Value::Null)
                            )));
                        }
                    }
                }
            })
            .await
            .map_err(|_| err("Del MCP connection timed out"))??;
        }
        Ok(())
    }
    /// Let (or stop letting) the server's shared browser credential reach `id`.
    fn set_browser_member(&self, server: &Server, id: &SessionId, member: bool) {
        let group = server.browser.lock().unwrap().as_ref().map(|(group, _)| group.clone());
        if let (Some(group), Some(tools)) = (group, self.tools.lock().unwrap().as_ref()) {
            tools.credentials.set_shared_member(&group, id, member);
        }
    }
    fn live(&self, id: &SessionId) -> Result<Arc<Live>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }
    async fn attach(
        &self,
        config: SessionConfig,
        id: SessionId,
        server: Arc<Server>,
        history: Vec<AgentEvent>,
    ) -> Result<SessionHandle> {
        let has_tools = self.tools.lock().unwrap().is_some();
        if has_tools {
            install_browser_instructions(&server, &id).await?;
        }
        let mut response = server.events().await?;
        // Establish the subscription before callers can send the first prompt.
        let first = tokio::time::timeout(std::time::Duration::from_secs(10), response.chunk())
            .await
            .map_err(|_| err("event subscription timed out"))?
            .map_err(err)?;
        let (events, _) = broadcast::channel(backend::EVENT_CHANNEL_CAPACITY);
        let live = Arc::new(Live {
            server,
            config: Mutex::new(config),
            events: events.clone(),
            busy: AtomicBool::new(false),
            connected: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            sequence: Mutex::new(None),
            advanced: tokio::sync::Notify::new(),
            mapping: Mutex::new(Default::default()),
            forms: Mutex::new(HashMap::new()),
            tasks: Mutex::new(vec![]),
        });
        let question = self
            .tools
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|t| t.question.clone());
        // The reader never awaits HTTP: it parses SSE lines and queues their
        // events; the emitter applies them in order, awaiting a turn's file
        // diff before that turn's completion, so ordering is unchanged.
        let (queue, mut queued) = tokio::sync::mpsc::unbounded_channel::<Queued>();
        let emit_live = live.clone();
        let emitter = tokio::spawn(async move {
            while let Some(item) = queued.recv().await {
                match item {
                    Queued::Diffs(diffs) => {
                        if let Ok(events) = diffs.await {
                            emit_live.emit(events);
                        }
                    }
                    Queued::Events(mapped, seq) => {
                        for event in &mapped {
                            if matches!(event, AgentEvent::TurnStarted { .. }) {
                                emit_live.busy.store(true, Ordering::SeqCst);
                            }
                            if matches!(event, AgentEvent::TurnCompleted { .. }) {
                                emit_live.busy.store(false, Ordering::SeqCst);
                            }
                        }
                        emit_live.emit(mapped);
                        if let Some(seq) = seq {
                            *emit_live.sequence.lock().unwrap() = Some(seq);
                            emit_live.advanced.notify_one();
                        }
                    }
                    Queued::Closed => {
                        emit_live.busy.store(false, Ordering::SeqCst);
                        emit_live.emit([AgentEvent::Error { message: "OpenCode event connection closed. Unload and reopen this chat to reconnect.".into(), recoverable: false }]);
                        break;
                    }
                }
            }
        });
        live.tasks.lock().unwrap().push(emitter);
        let stream_live = live.clone();
        let session_id = id.clone();
        let task = tokio::spawn(async move {
            let mut buffer = first.map(|b| b.to_vec()).unwrap_or_default();
            // Bytes before `scanned` hold no newline; never rescan them.
            let mut scanned = 0;
            let mut files_changed = false;
            loop {
                let mut start = 0;
                while let Some(offset) = buffer[scanned..].iter().position(|b| *b == b'\n') {
                    let end = scanned + offset;
                    let line = &buffer[start..=end];
                    start = end + 1;
                    scanned = start;
                    if let Some(data) = line.strip_prefix(b"data:")
                        && let Ok(event) = serde_json::from_slice::<Value>(data)
                    {
                        let p = &event["data"];
                        if event["type"] == "form.created"
                            && p["form"]["sessionID"] == session_id.as_str()
                        {
                            let form = p["form"].clone();
                            let form_id = mapping::string(&form, "id");
                            let sid = session_id.clone();
                            let ask = question.clone();
                            let server = stream_live.server.clone();
                            let form_events = stream_live.events.clone();
                            let task = tokio::spawn(async move {
                                let answer = match ask {
                                    Some(ask) => forms::answer(sid.clone(), &form, &ask).await,
                                    None => Ok(None),
                                };
                                let path = format!(
                                    "/api/session/{}/form/{}",
                                    seg(sid.as_str()),
                                    seg(&mapping::string(&form, "id"))
                                );
                                let result = match answer {
                                    Ok(Some(answer)) => {
                                        server
                                            .json(
                                                Method::POST,
                                                &format!("{path}/reply"),
                                                Some(&json!({"answer":answer})),
                                            )
                                            .await
                                    }
                                    Ok(None) => server.json(Method::DELETE, &path, None).await,
                                    Err(error) => Err(error),
                                };
                                if let Err(error) = result {
                                    let _ = form_events.send(AgentEvent::Error {
                                        message: format!("OpenCode question failed: {error}"),
                                        recoverable: true,
                                    });
                                    // Release the native tool's wait if presentation or validation failed.
                                    if let Err(cancel_error) =
                                        server.json(Method::DELETE, &path, None).await
                                    {
                                        let _ = form_events.send(AgentEvent::Error {
                                                message: format!("OpenCode could not cancel the question: {cancel_error}. Stop this turn to release it."),
                                                recoverable: true,
                                            });
                                    }
                                }
                            });
                            let mut forms = stream_live.forms.lock().unwrap();
                            forms.retain(|_, task| !task.is_finished());
                            if let Some(previous) = forms.insert(form_id, task) {
                                previous.abort();
                            }
                        }
                        let belongs = p["sessionID"]
                            .as_str()
                            .or(p["part"]["sessionID"].as_str())
                            .or(p["info"]["sessionID"].as_str())
                            == Some(session_id.as_str());
                        if belongs {
                            if matches!(
                                event["type"].as_str(),
                                Some("form.cancelled" | "form.replied")
                            ) && let Some(task) = stream_live
                                .forms
                                .lock()
                                .unwrap()
                                .remove(&mapping::string(p, "id"))
                            {
                                task.abort();
                            }
                            if event["type"] == "session.step.ended"
                                && p["files"].as_array().is_some_and(|files| !files.is_empty())
                            {
                                files_changed = true;
                            }
                            let mapped = stream_live.mapping.lock().unwrap().event(&event);
                            if files_changed
                                && mapped
                                    .iter()
                                    .any(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
                            {
                                let cwd = stream_live.config.lock().unwrap().cwd.clone();
                                let server = stream_live.server.clone();
                                let path = format!("/api/session/{}/diff", seg(session_id.as_str()));
                                let diffs = tokio::spawn(async move {
                                    match server.json(Method::GET, &path, None).await {
                                        Ok(diffs) => mapping::diffs(&cwd, &diffs),
                                        Err(error) => vec![AgentEvent::Error {
                                            message: format!(
                                                "Could not load OpenCode file changes: {error}"
                                            ),
                                            recoverable: true,
                                        }],
                                    }
                                });
                                let _ = queue.send(Queued::Diffs(diffs));
                                files_changed = false;
                            }
                            let _ = queue.send(Queued::Events(mapped, event["durable"]["seq"].as_u64()));
                        }
                    }
                }
                buffer.drain(..start);
                scanned = buffer.len();
                match response.chunk().await {
                    Ok(Some(bytes)) => buffer.extend_from_slice(&bytes),
                    _ => {
                        stream_live.connected.store(false, Ordering::SeqCst);
                        let _ = queue.send(Queued::Closed);
                        break;
                    }
                }
            }
        });
        live.tasks.lock().unwrap().push(task);
        self.set_browser_member(&live.server, &id, true);
        let previous = self.sessions.lock().unwrap().insert(id.clone(), live);
        if let Some(previous) = previous {
            previous.stop_tasks();
        }
        Ok(SessionHandle::new(id, Provider::OpenCode, events).with_history(history))
    }
    async fn submit(
        &self,
        id: &SessionId,
        input: UserInput,
        command: Option<ComposerCommand>,
        steer: bool,
    ) -> Result<()> {
        let live = self.live(id)?;
        if !live.connected.load(Ordering::SeqCst) {
            return Err(err(
                "event connection closed; unload and reopen this chat before sending",
            ));
        }
        if !steer
            && live
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return Err(AgentError::Busy(id.to_string()));
        }
        let generation = live.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let result = async {
            // A native command can complete locally without starting an execution.
            // Capture its boundary so completion can be reconciled with the event stream.
            let before = if command.is_some() {
                let previous = *live.sequence.lock().unwrap();
                live.server.watermark(id, previous).await?
            } else { None };
            let body = prompt_body(&input, command.as_ref(), steer)?;
            let path = format!(
                "/api/session/{}/{}",
                seg(id.as_str()),
                if command.is_some() {
                    "command"
                } else {
                    "prompt"
                }
            );
            live.server.json(Method::POST, &path, Some(&body)).await?;
            if command.is_some() {
                let command_live = live.clone();
                let sid = id.clone();
                let task = tokio::spawn(async move {
                    let result = async {
                        // The native idle barrier owns command execution duration.
                        let response = command_live.server.request(Method::POST,
                            &format!("/api/experimental/session/{}/wait", seg(sid.as_str()))).send().await.map_err(err)?;
                        server::decode(response).await?;
                        let target = command_live.server.watermark(&sid, before).await?;
                        if target != before {
                            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                                loop {
                                    let advanced = command_live.advanced.notified();
                                    if *command_live.sequence.lock().unwrap() >= target { break; }
                                    advanced.await;
                                }
                            }).await.map_err(|_| err("command completed but its events did not arrive; unload and reopen this chat"))?;
                        }
                        Ok::<(), AgentError>(())
                    }.await;
                    if command_live.generation.load(Ordering::SeqCst) == generation {
                        match result {
                            Ok(()) => {
                                if command_live.busy.swap(false, Ordering::SeqCst) {
                                    command_live.emit([AgentEvent::TurnCompleted {
                                        turn_id: format!("command-{generation}"),
                                        status: TurnStatus::Completed,
                                    }]);
                                }
                            }
                            Err(error) => {
                                command_live.busy.store(false, Ordering::SeqCst);
                                command_live.emit([AgentEvent::Error { message: error.to_string(), recoverable: false }]);
                            }
                        }
                    }
                });
                let mut tasks = live.tasks.lock().unwrap();
                tasks.retain(|task| !task.is_finished());
                tasks.push(task);
            }
            Ok(())
        }
        .await;
        if result.is_err() && !steer {
            live.busy.store(false, Ordering::SeqCst);
        }
        result
    }
}
fn prompt_body(input: &UserInput, command: Option<&ComposerCommand>, steer: bool) -> Result<Value> {
    let mut text = input.text.trim().to_owned();
    for skill in &input.skills {
        if let Some(rest) = text
            .strip_prefix(&format!("/{}", skill.name))
            .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
        {
            text = rest.trim_start().to_owned();
        }
    }
    let mut body = json!({"text":text,"delivery":if steer{"steer"}else{"queue"}});
    if !input.skills.is_empty() {
        body["skills"] = input
            .skills
            .iter()
            .map(|s| {
                s.target
                    .as_ref()
                    .map(|id| json!({"id":id}))
                    .ok_or_else(|| err("skill is missing its native ID"))
            })
            .collect::<Result<Vec<_>>>()?
            .into();
    }
    body["files"]=input.attachments.iter().map(|a|{
        let path=a.path.canonicalize()?;
        let uri=reqwest::Url::from_file_path(&path).map_err(|_|err("invalid attachment path"))?;
        Ok(json!({"uri":uri.as_str(),"name":path.file_name().unwrap_or_default().to_string_lossy()}))
    }).collect::<Result<Vec<_>>>()?.into();
    if let Some(c) = command {
        body["name"] = c.name.clone().into();
        body["text"] = split_command(&input.text)
            .filter(|(name, _)| *name == c.name)
            .map(|(_, args)| args)
            .unwrap_or(&input.text)
            .into();
    }
    Ok(body)
}
async fn install_browser_instructions(server: &Server, id: &SessionId) -> Result<()> {
    let instructions = format!(
        "{} OpenCode exposes this MCP server as del. Call tools.del.browser through execute (or the advertised del_browser tool). The built-in browser.tabs tools target OpenCode's desktop browser, not Del's embedded browser.",
        backend::BROWSER_INSTRUCTIONS
    );
    server
        .json(
            Method::PUT,
            &format!(
                "/api/experimental/session/{}/instructions/entries/del-browser",
                seg(id.as_str())
            ),
            Some(&json!({"value":instructions})),
        )
        .await?;
    Ok(())
}

fn model_ref(config: &SessionConfig) -> Result<Option<Value>> {
    config
        .model
        .as_ref()
        .map(|model| {
            let (provider, id) = model
                .split_once('/')
                .ok_or_else(|| err("model must be provider/model"))?;
            let mut value = json!({"providerID":provider,"id":id});
            if let Some(variant) = &config.reasoning_effort {
                value["variant"] = variant.clone().into();
            }
            Ok(value)
        })
        .transpose()
}
fn permissions(mode: PermissionMode) -> Value {
    let mut rules = vec![
        json!({"action":"*","resource":"*","effect":if mode==PermissionMode::FullAuto {"allow"}else{"ask"}}),
    ];
    if mode == PermissionMode::AutoEdit {
        rules.push(json!({"action":"edit","resource":"*","effect":"allow"}));
    }
    rules.into()
}
async fn configure(server: &Server, id: &SessionId, config: &SessionConfig) -> Result<()> {
    server
        .json(
            Method::PATCH,
            &format!("/api/session/{}", seg(id.as_str())),
            Some(&json!({"permissions":permissions(config.permission_mode)})),
        )
        .await?;
    let model = match model_ref(config)? {
        Some(model) => model,
        None => {
            let default = server.json(Method::GET, "/api/model/default", None).await?;
            let model = &default["data"];
            if !model["id"].is_string() || !model["providerID"].is_string() {
                return Err(err("default model is unavailable"));
            }
            json!({"id":model["id"],"providerID":model["providerID"]})
        }
    };
    server
        .json(
            Method::POST,
            &format!("/api/session/{}/model", seg(id.as_str())),
            Some(&json!({"model":model})),
        )
        .await?;
    Ok(())
}

#[async_trait]
impl AgentBackend for OpenCodeBackend {
    async fn session_process(&self, id: &SessionId) -> Option<(u32, bool)> {
        Some((self.live(id).ok()?.server.pid, true))
    }
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }
    fn set_browser_tools(&self, tools: backend::BrowserTools) {
        *self.tools.lock().unwrap() = Some(tools);
    }
    async fn list_models(&self, cwd: &Path) -> Result<Vec<ModelInfo>> {
        let server = self.server(cwd).await?;
        let models = server.json(Method::GET, "/api/model", None).await?;
        let default = server.json(Method::GET, "/api/model/default", None).await?;
        Ok(mapping::models(&models, &default))
    }
    async fn list_commands(&self, cwd: &Path) -> Result<Vec<ComposerCommand>> {
        let server = self.server(cwd).await?;
        // Unlike model discovery, v2.1's command/skill routes do not await
        // plugin activation. Use the native model-default route as that barrier.
        server.json(Method::GET, "/api/model/default", None).await?;
        let commands = server.json(Method::GET, "/api/command", None).await?;
        let skills = server.json(Method::GET, "/api/skill", None).await?;
        Ok(mapping::commands(&commands, &skills))
    }
    async fn start_session(&self, mut config: SessionConfig) -> Result<SessionHandle> {
        config.cwd = config.cwd.canonicalize()?;
        let server = self.server(&config.cwd).await?;
        self.install_tools(&server).await?;
        let mut body = json!({"location":{"directory":config.cwd},"permissions":permissions(config.permission_mode)});
        if let Some(model) = model_ref(&config)? {
            body["model"] = model;
        }
        if config.permission_mode == PermissionMode::Plan {
            body["agent"] = "plan".into();
        }
        let session = server
            .json(Method::POST, "/api/session", Some(&body))
            .await?;
        let id = session["data"]["id"]
            .as_str()
            .ok_or_else(|| err("missing session identity"))?;
        self.attach(config, SessionId::new(id), server, vec![])
            .await
    }
    async fn resume(&self, config: SessionConfig, id: &SessionId) -> Result<SessionHandle> {
        if let Ok(live) = self.live(id) {
            if live.connected.load(Ordering::SeqCst) && live.server.is_alive() {
                return Ok(SessionHandle::new(
                    id.clone(),
                    Provider::OpenCode,
                    live.events.clone(),
                ));
            }
            // Its event stream or server is gone: stop it before re-attaching.
            self.close(id).await?;
        }
        let server = self.server(&config.cwd).await?;
        self.install_tools(&server).await?;
        configure(&server, id, &config).await?;
        let mut mapping = mapping::Stream::default();
        let mut history = vec![];
        let mut first_user: Option<String> = None;
        let mut last_user: Option<String> = None;
        let mut cursor: Option<String> = None;
        loop {
            let mut request = server
                .request(Method::GET, &format!("/api/session/{}/message", seg(id.as_str())))
                .query(&[("limit", "200")])
                .timeout(std::time::Duration::from_secs(30));
            request = if let Some(cursor) = &cursor {
                request.query(&[("cursor", cursor)])
            } else {
                request.query(&[("order", "asc")])
            };
            let page = server::decode(request.send().await.map_err(err)?).await?;
            for message in page["data"].as_array().into_iter().flatten() {
                if message["type"] == "user"
                    && let Some(id) = message["id"].as_str()
                {
                    first_user.get_or_insert_with(|| id.to_owned());
                    last_user = Some(id.to_owned());
                }
                history.extend(mapping.message(message));
            }
            cursor = page["cursor"]["next"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if let (Some(first), Some(last)) = (first_user, last_user) {
            let response = server
                .request(Method::GET, &format!("/api/session/{}/diff", seg(id.as_str())))
                .query(&[("from", first), ("to", last)])
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .map_err(err)?;
            let diffs = server::decode(response).await?;
            history.extend(mapping::diffs(&config.cwd, &diffs));
        }
        self.attach(config, id.clone(), server, history).await
    }
    async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        self.submit(id, input, None, false).await
    }
    async fn invoke_command(
        &self,
        id: &SessionId,
        command: ComposerCommand,
        mut input: UserInput,
    ) -> Result<()> {
        if command.kind == CommandKind::Skill {
            if !input.skills.contains(&command) {
                input.skills.push(command);
            }
            self.submit(id, input, None, false).await
        } else {
            self.submit(id, input, Some(command), false).await
        }
    }
    async fn steer(&self, id: &SessionId, text: String) -> Result<()> {
        self.steer_input(id, UserInput::text(text)).await
    }
    async fn steer_input(&self, id: &SessionId, input: UserInput) -> Result<()> {
        self.submit(id, input, None, true).await
    }
    async fn interrupt(&self, id: &SessionId) -> Result<()> {
        self.live(id)?
            .server
            .json(Method::POST, &format!("/api/session/{}/interrupt", seg(id.as_str())), None)
            .await?;
        Ok(())
    }
    async fn reconfigure(&self, config: SessionConfig, id: &SessionId) -> Result<SessionHandle> {
        let live = self.live(id)?;
        if live.busy.load(Ordering::SeqCst) {
            return Err(AgentError::Busy(id.to_string()));
        }
        configure(&live.server, id, &config).await?;
        let previous_mode = live.config.lock().unwrap().permission_mode;
        if (previous_mode == PermissionMode::Plan)
            != (config.permission_mode == PermissionMode::Plan)
        {
            live.server.json(Method::POST, &format!("/api/session/{}/agent", seg(id.as_str())),
                Some(&json!({"agent":if config.permission_mode == PermissionMode::Plan {"plan"} else {"build"}}))).await?;
        }
        *live.config.lock().unwrap() = config;
        Ok(SessionHandle::new(
            id.clone(),
            Provider::OpenCode,
            live.events.clone(),
        ))
    }
    async fn respond_approval(
        &self,
        id: &SessionId,
        approval: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        self.live(id)?.server.json(Method::POST,&format!("/api/session/{}/permission/{}/reply", seg(id.as_str()), seg(approval)),Some(&json!({"decision":if response == ApprovalResponse::Approve {"once"} else {"reject"}}))).await?;
        Ok(())
    }
    async fn generate_session_title(
        &self,
        id: &SessionId,
        context: &str,
    ) -> Result<Option<String>> {
        let live = self.live(id)?;
        let value = live
            .server
            .json(
                Method::POST,
                &format!("/api/session/{}/generate", seg(id.as_str())),
                Some(&json!({"prompt": format!("{}\n\nConversation to name:\n{context}", del_agent::titles::INSTRUCTIONS)})),
            )
            .await?;
        Ok(value["data"]["text"].as_str().map(str::to_owned))
    }

    async fn session_title(&self, id: &SessionId, cwd: &Path) -> Result<Option<String>> {
        let value = self
            .server(cwd)
            .await?
            .json(Method::GET, &format!("/api/session/{}", seg(id.as_str())), None)
            .await?;
        Ok(value["data"]["title"].as_str().map(str::to_owned))
    }
    async fn close(&self, id: &SessionId) -> Result<()> {
        let live = self.sessions.lock().unwrap().remove(id);
        if let Some(live) = live {
            live.stop_tasks();
            self.set_browser_member(&live.server, id, false);
            if live.busy.load(Ordering::SeqCst) {
                let _ = live
                    .server
                    .json(
                        Method::POST,
                        &format!("/api/session/{}/interrupt", seg(id.as_str())),
                        Some(&json!({})),
                    )
                    .await;
            }
        }
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        let ids: Vec<_> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.close(&id).await?;
        }
        for (_, server) in self.servers.lock().await.drain() {
            self.retire_browser(&server);
        }
        Ok(())
    }
}
impl Drop for OpenCodeBackend {
    fn drop(&mut self) {
        for live in self.sessions.get_mut().unwrap().values() {
            live.stop_tasks();
        }
    }
}

/// Allocate a native conversation before starting the TUI so terminal history
/// always records its exact identity, including before the first prompt.
pub async fn create_terminal_session(
    cwd: &Path,
    model: Option<&str>,
    variant: Option<&str>,
) -> Result<String> {
    create_terminal_session_with_environment(cwd, model, variant, &[]).await
}

pub async fn create_terminal_session_with_environment(cwd: &Path, model: Option<&str>, variant: Option<&str>, env: &[(String, String)]) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    let server = Server::start(&cwd, env).await?;
    let mut body = json!({"location":{"directory":cwd}});
    if let Some(model) = model {
        let (provider, id) = model.split_once('/').ok_or_else(|| err("invalid model"))?;
        body["model"] = json!({"id":id,"providerID":provider});
        if let Some(variant) = variant {
            body["model"]["variant"] = variant.into();
        }
    }
    let value = server
        .json(Method::POST, "/api/session", Some(&body))
        .await?;
    let id = value["data"]["id"]
        .as_str()
        .ok_or_else(|| err("missing terminal identity"))?;
    install_browser_instructions(&server, &SessionId::new(id)).await?;
    Ok(id.to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct NoCredentials;
    impl backend::BrowserCredentials for NoCredentials {
        fn mint(&self, _: &SessionId) -> std::io::Result<String> {
            Err(std::io::Error::other("no browser in this fixture"))
        }
        fn rebind(&self, _: &SessionId, _: &SessionId) {}
        fn revoke(&self, _: &SessionId) {}
        fn mint_shared(&self, _: &SessionId) -> std::io::Result<String> {
            Err(std::io::Error::other("no browser in this fixture"))
        }
        fn set_shared_member(&self, _: &SessionId, _: &SessionId, _: bool) {}
    }
    #[tokio::test]
    #[ignore = "requires installed OpenCode 2.x; use isolated XDG data directories"]
    async fn real_native_form_answer_and_cancellation() {
        struct Closed(Arc<tokio::sync::Notify>);
        impl Drop for Closed {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let backend = OpenCodeBackend::default();
        let server = backend.server(dir.path()).await.unwrap();
        let created = server.json(Method::POST, "/api/session",
            Some(&json!({"location":{"directory":dir.path().canonicalize().unwrap()},"model":{"providerID":"opencode","id":"big-pickle"}}))).await.unwrap();
        let id = SessionId::new(created["data"]["id"].as_str().unwrap());
        let started = Arc::new(tokio::sync::Notify::new());
        let closed = Arc::new(tokio::sync::Notify::new());
        let ask_started = started.clone();
        let ask_closed = closed.clone();
        *backend.tools.lock().unwrap() = Some(backend::BrowserTools {
            url: String::new(),
            credentials: Arc::new(NoCredentials),
            claude_session_config: None,
            question: Some(Arc::new(move |_, prompt, _| {
                let started = ask_started.clone();
                let closed = ask_closed.clone();
                Box::pin(async move {
                    if prompt.contains("Cancel fixture") {
                        let _closed = Closed(closed);
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok(Some(backend::QuestionAnswer::Option(0)))
                })
            })),
        });
        backend
            .attach(
                SessionConfig {
                    provider: Provider::OpenCode,
                    cwd: dir.path().into(),
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    permission_mode: PermissionMode::FullAuto,
                },
                id.clone(),
                server.clone(),
                vec![],
            )
            .await
            .unwrap();
        let form_path = format!("/api/session/{id}/form");
        let form = server.json(Method::POST, &form_path, Some(&json!({
            "title":"Answer fixture","fields":[{"key":"approved","type":"boolean","required":true}]
        }))).await.unwrap();
        let path = format!("{form_path}/{}", form["data"]["id"].as_str().unwrap());
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let state = server.json(Method::GET, &path, None).await.unwrap();
                if state["data"]["state"]["status"] == "answered" {
                    assert_eq!(state["data"]["state"]["answer"]["approved"], true);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("native form was not answered");
        let form = server
            .json(
                Method::POST,
                &form_path,
                Some(&json!({
                    "title":"Cancel fixture","fields":[{"key":"answer","type":"string"}]
                })),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        server
            .json(
                Method::DELETE,
                &format!("{form_path}/{}", form["data"]["id"].as_str().unwrap()),
                None,
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), closed.notified())
            .await
            .expect("cancelled native form left the question callback open");
        backend.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires installed OpenCode 2.x; use isolated XDG data directories"]
    async fn real_local_command_finishes_without_a_model_turn() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join(".opencode/plugins/del-test.js");
        std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        let marker = dir.path().join("command-ran");
        let source = format!(
            r#"
import {{ Plugin }} from "@opencode/plugin";
import {{ writeFile }} from "node:fs/promises";
export default Plugin.define({{
  id: "del-test",
  setup: async (ctx) => {{
    await ctx.command.transform((editor) => editor.add({{
      name: "del-local-test",
      description: "Write a local test marker without a model turn",
      execute: async () => {{ await writeFile({}, "ok"); }}
    }}));
  }}
}});
"#,
            serde_json::to_string(&marker).unwrap()
        );
        std::fs::write(&plugin, source).unwrap();
        std::fs::write(dir.path().join("opencode.json"),
            json!({"commands":{"del-model-test":{"template":"Reply with exactly DEL_COMMAND_OK. Do not use tools."}}}).to_string()).unwrap();
        let backend = OpenCodeBackend::default();
        let commands = backend.list_commands(dir.path()).await.unwrap();
        if !commands.iter().any(|c| c.name == "del-local-test") {
            let server = backend.server(dir.path()).await.unwrap();
            let plugins = server.json(Method::GET, "/api/plugin", None).await.unwrap();
            let config = server.json(Method::GET, "/api/config", None).await.unwrap();
            let documents: Vec<_> = config
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| json!({"type":d["type"],"path":d["path"],"plugins":d["info"]["plugins"]}))
                .collect();
            panic!("native plugin command was not discovered: {plugins}; documents={documents:?}");
        }
        let command = commands
            .iter()
            .find(|c| c.name == "del-local-test")
            .expect("native plugin command was not discovered")
            .clone();
        let config = SessionConfig {
            provider: Provider::OpenCode,
            cwd: dir.path().into(),
            model: Some("opencode/big-pickle".into()),
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::FullAuto,
        };
        let session = backend.start_session(config).await.unwrap();
        let mut events = session.subscribe();
        let model_command = commands
            .iter()
            .find(|c| c.name == "del-model-test")
            .unwrap()
            .clone();
        backend
            .invoke_command(
                &session.id,
                model_command,
                UserInput::text("/del-model-test"),
            )
            .await
            .unwrap();
        let text = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let mut text = String::new();
            loop {
                match events.recv().await.unwrap() {
                    AgentEvent::AssistantTextDelta { text: delta, .. } => text.push_str(&delta),
                    AgentEvent::TurnCompleted { status, .. } => {
                        assert_eq!(status, TurnStatus::Completed);
                        break text;
                    }
                    AgentEvent::Error { message, .. } => panic!("{message}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("model command did not finish");
        assert!(text.contains("DEL_COMMAND_OK"), "command reply: {text}");
        for _ in 0..2 {
            backend
                .invoke_command(
                    &session.id,
                    command.clone(),
                    UserInput::text("/del-local-test"),
                )
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    match events.recv().await.unwrap() {
                        AgentEvent::TurnCompleted { status, .. } => {
                            assert_eq!(status, TurnStatus::Completed);
                            break;
                        }
                        AgentEvent::Error { message, .. } => panic!("{message}"),
                        AgentEvent::AssistantTextDelta { .. } => {
                            panic!("local command invoked a model")
                        }
                        _ => {}
                    }
                }
            })
            .await
            .expect("local command remained busy");
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ok");
        }
        backend.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires installed OpenCode 2.x; use isolated XDG data directories"]
    async fn real_title_generation_preserves_history() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let observed = called.clone();
        let router = axum::Router::new().route("/chat/completions", axum::routing::post(
            move |axum::Json(body): axum::Json<Value>| {
                let observed = observed.clone();
                async move {
                    assert_eq!(body["model"], "title-test");
                    let messages = body["messages"].to_string();
                    assert!(messages.contains("chess board"));
                    assert!(messages.contains("Return only the title"));
                    observed.store(true, Ordering::SeqCst);
                    let chunk = json!({"id":"test-title", "object":"chat.completion.chunk",
                        "created":1, "model":"title-test",
                        "choices":[{"index":0,"delta":{"role":"assistant","content":"Chess Board"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}});
                    ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
                }
            }));
        let endpoint = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        std::fs::write(dir.path().join("opencode.json"), json!({
            "providers":{"del-test":{"package":"@opencode/ai/providers/openai-compatible",
                "settings":{"baseURL":format!("http://{address}"),"apiKey":"local-test"},
                "models":{"title-test":{"name":"Title Test","limit":{"context":32000,"output":1000}}}}}
        }).to_string()).unwrap();
        let backend = OpenCodeBackend::default();
        let session = backend
            .start_session(SessionConfig {
                provider: Provider::OpenCode,
                cwd: dir.path().to_owned(),
                model: Some("del-test/title-test".into()),
                reasoning_effort: None,
                service_tier: None,
                permission_mode: PermissionMode::FullAuto,
            })
            .await
            .unwrap();
        let server = backend.server(dir.path()).await.unwrap();
        let path = format!("/api/session/{}/message", session.id);
        let before = server.json(Method::GET, &path, None).await.unwrap();
        let title = backend
            .generate_session_title(
                &session.id,
                "User: Build a chess board with legal move highlighting.",
            )
            .await
            .unwrap();
        assert_eq!(title.as_deref(), Some("Chess Board"));
        assert!(called.load(Ordering::SeqCst));
        let after = server.json(Method::GET, &path, None).await.unwrap();
        assert_eq!(before, after, "title request changed conversation history");
        assert!(
            !backend
                .live(&session.id)
                .unwrap()
                .busy
                .load(Ordering::SeqCst)
        );
        backend.shutdown().await.unwrap();
        endpoint.abort();
    }

    #[tokio::test]
    #[ignore = "requires installed OpenCode 2.x; use isolated XDG data directories"]
    async fn real_model_reset_and_plan_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let backend = OpenCodeBackend::default();
        let models = backend.list_models(dir.path()).await.unwrap();
        let explicit = models.iter().find(|m| !m.is_default).unwrap();
        let mut config = SessionConfig {
            provider: Provider::OpenCode,
            cwd: dir.path().into(),
            model: Some(explicit.id.clone()),
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::FullAuto,
        };
        let session = backend.start_session(config.clone()).await.unwrap();
        let server = backend.server(dir.path()).await.unwrap();
        // Keep the request path alive across the async request.
        let path = format!("/api/session/{}", session.id);
        let before = server.json(Method::GET, &path, None).await.unwrap();
        let expected = model_ref(&config).unwrap().unwrap();
        assert_eq!(before["data"]["model"]["id"], expected["id"]);
        assert_eq!(
            before["data"]["model"]["providerID"],
            expected["providerID"]
        );
        config.model = None;
        config.permission_mode = PermissionMode::Plan;
        backend
            .reconfigure(config.clone(), &session.id)
            .await
            .unwrap();
        let default = server
            .json(Method::GET, "/api/model/default", None)
            .await
            .unwrap();
        let after = server.json(Method::GET, &path, None).await.unwrap();
        assert_eq!(after["data"]["model"]["id"], default["data"]["id"]);
        assert_eq!(
            after["data"]["model"]["providerID"],
            default["data"]["providerID"]
        );
        assert_eq!(after["data"]["agent"], "plan");
        config.permission_mode = PermissionMode::FullAuto;
        backend.reconfigure(config, &session.id).await.unwrap();
        let after = server.json(Method::GET, &path, None).await.unwrap();
        assert_eq!(after["data"]["agent"], "build");
        backend.shutdown().await.unwrap();
    }

    #[test]
    fn path_segments_are_percent_encoded() {
        assert_eq!(seg("ses_ABC-1.x~"), "ses_ABC-1.x~");
        assert_eq!(seg("../a b/?#"), "..%2Fa%20b%2F%3F%23");
    }

    #[test]
    fn native_skills_and_steering_are_structured() {
        let mut input = UserInput::text("/review Inspect changes");
        input.skills.push(ComposerCommand {
            name: "review".into(),
            description: String::new(),
            argument_hint: String::new(),
            kind: CommandKind::Skill,
            source: "OpenCode".into(),
            target: Some("opaque-id".into()),
        });
        let body = prompt_body(&input, None, true).unwrap();
        assert_eq!(body["text"], "Inspect changes");
        assert_eq!(body["skills"], json!([{"id":"opaque-id"}]));
        assert_eq!(body["delivery"], "steer");
    }
    #[test]
    fn model_reference_preserves_variant() {
        let c = SessionConfig {
            provider: Provider::OpenCode,
            cwd: "/tmp".into(),
            model: Some("provider/org/model".into()),
            reasoning_effort: Some("high".into()),
            service_tier: None,
            permission_mode: PermissionMode::FullAuto,
        };
        assert_eq!(
            model_ref(&c).unwrap(),
            Some(json!({"id":"org/model","providerID":"provider","variant":"high"}))
        );
    }
}
