use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v1::{
    PermissionOptionKind, RequestPermissionRequest, SessionNotification,
};
use async_trait::async_trait;
use del_agent::*;
use serde_json::{Value, json};
use tokio::sync::{broadcast, oneshot};

use crate::registry::{self, AcpAgentConfig};
use crate::rpc::{Handler, Rpc, RpcError};
use crate::translate::{Translator, approval_kind, input_summary};

const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_GRACE: Duration = Duration::from_secs(2);

struct PendingApproval {
    answer: oneshot::Sender<ApprovalResponse>,
}

#[derive(Default)]
struct State {
    remote: Option<String>,
    translator: Translator,
    approvals: HashMap<String, PendingApproval>,
    cancelled: bool,
    closed: bool,
    closing: bool,
    /// While `session/load` runs: replayed updates rebuilt as history.
    replay: Option<(Translator, Vec<AgentEvent>)>,
}

struct Client {
    state: Mutex<State>,
    events: broadcast::Sender<AgentEvent>,
    permission_mode: PermissionMode,
}

impl Client {
    fn emit(&self, events: Vec<AgentEvent>) {
        for event in events {
            let _ = self.events.send(event);
        }
    }

    fn finish(&self, result: std::result::Result<Value, RpcError>) {
        let result = result.and_then(|v| match v["stopReason"].as_str() {
            Some("end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled") => {
                Ok(v)
            }
            reason => Err(RpcError::new(
                -32603,
                format!("ACP agent returned an invalid or failed stop reason: {reason:?}"),
            )),
        });
        let mut state = self.state.lock().unwrap();
        let status = if state.cancelled
            || result
                .as_ref()
                .is_ok_and(|v| v["stopReason"] == "cancelled")
        {
            TurnStatus::Interrupted
        } else if result.is_err() {
            TurnStatus::Failed
        } else {
            TurnStatus::Completed
        };
        if let Err(e) = result
            && state.translator.turn_id().is_some()
            && !state.cancelled
        {
            self.emit(vec![AgentEvent::Error {
                message: e.to_string(),
                recoverable: true,
            }]);
        }
        state.approvals.clear();
        self.emit(state.translator.end_turn(status));
    }
}

fn cancelled() -> Value {
    json!({"outcome":{"outcome":"cancelled"}})
}

#[async_trait]
impl Handler for Client {
    fn notification(&self, method: &str, params: Value) {
        if method != "session/update" {
            return;
        }
        let Ok(update) = serde_json::from_value::<SessionNotification>(params) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        let state = &mut *state;
        if state.remote.as_deref() != Some(update.session_id.to_string().as_str()) {
            return;
        }
        match state.replay.as_mut() {
            Some((translator, history)) => history.extend(translator.replay(&update.update)),
            None => self.emit(state.translator.update(&update.update)),
        }
    }

    async fn request(&self, method: &str, params: Value) -> std::result::Result<Value, RpcError> {
        if method != "session/request_permission" {
            return Err(RpcError::method_not_found(method));
        }
        let request: RequestPermissionRequest =
            serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))?;
        // A binary approval never grants a persistent permission. If the agent
        // offers only allow_always, an approval cancels instead of widening it.
        let allow = request
            .options
            .iter()
            .find(|o| o.kind == PermissionOptionKind::AllowOnce)
            .map(|o| o.option_id.to_string());
        let reject = request
            .options
            .iter()
            .find(|o| o.kind == PermissionOptionKind::RejectOnce)
            .map(|o| o.option_id.to_string());
        let kind = request.tool_call.fields.kind.unwrap_or_default();
        let answer = {
            let mut state = self.state.lock().unwrap();
            if state.closed
                || state.cancelled
                || state.translator.turn_id().is_none()
                || state.remote.as_deref() != Some(request.session_id.to_string().as_str())
            {
                return Ok(cancelled());
            }
            let auto = match self.permission_mode {
                PermissionMode::FullAuto => Some(ApprovalResponse::Approve),
                PermissionMode::AutoEdit if approval_kind(kind) == ApprovalKind::FileChange => {
                    Some(ApprovalResponse::Approve)
                }
                PermissionMode::Plan => Some(ApprovalResponse::Deny),
                _ => None,
            };
            if let Some(answer) = auto {
                return Ok(permission_result(answer, allow, reject));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            state
                .approvals
                .insert(id.clone(), PendingApproval { answer: tx });
            let fields = &request.tool_call.fields;
            self.emit(vec![AgentEvent::ApprovalRequested {
                id,
                kind: approval_kind(kind),
                detail: input_summary(
                    fields.title.as_deref().unwrap_or("Allow tool call?"),
                    fields.raw_input.as_ref(),
                ),
            }]);
            rx
        };
        Ok(match answer.await {
            Ok(answer) => permission_result(answer, allow, reject),
            Err(_) => cancelled(),
        })
    }

    fn closed(&self, exit: Option<i32>, stderr_tail: String) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.approvals.clear();
        self.emit(state.translator.end_turn(TurnStatus::Failed));
        if !state.closing {
            self.emit(vec![AgentEvent::Error {
                message: format!("ACP agent exited ({exit:?}): {stderr_tail}"),
                recoverable: false,
            }]);
        }
    }
}

fn permission_result(
    answer: ApprovalResponse,
    allow: Option<String>,
    reject: Option<String>,
) -> Value {
    let option = match answer {
        ApprovalResponse::Approve => allow,
        ApprovalResponse::Deny => reject,
    };
    option.map_or_else(
        cancelled,
        |id| json!({"outcome":{"outcome":"selected","optionId":id}}),
    )
}

struct Live {
    models: Vec<ModelInfo>,
    rpc: Arc<Rpc>,
    client: Arc<Client>,
    remote: String,
}

struct SetupGuard(Option<Arc<Rpc>>);
impl Drop for SetupGuard {
    fn drop(&mut self) {
        if let Some(rpc) = &self.0 {
            rpc.kill_now();
        }
    }
}

impl Live {
    fn alive(&self) -> bool {
        let state = self.client.state.lock().unwrap();
        !state.closed && !state.closing
    }

    /// End any turn and terminate the agent.
    async fn retire(&self) {
        {
            let mut state = self.client.state.lock().unwrap();
            state.closing = true;
            state.approvals.clear();
            self.client
                .emit(state.translator.end_turn(TurnStatus::Interrupted));
        }
        self.rpc.kill(CLOSE_GRACE).await;
    }
}

/// Launch `agent` and open (or load) one session on it.
async fn spawn_live(
    agent: &AcpAgentConfig,
    config: &SessionConfig,
    saved: Option<&(String, String)>,
    model: Option<&str>,
) -> Result<(SessionId, Arc<Live>, Vec<AgentEvent>)> {
    let program = agent
        .resolve()
        .ok_or_else(|| AgentError::Backend(format!("{} is not installed", agent.name)))?;
    let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
    let client = Arc::new(Client {
        state: Mutex::default(),
        events: events.clone(),
        permission_mode: config.permission_mode,
    });
    let env = agent
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    let rpc = Rpc::spawn(&program, &agent.args, &env, &config.cwd, client.clone())?;
    let mut guard = SetupGuard(Some(rpc.clone()));
    let setup = async {
        let init = rpc.request("initialize", json!({
            "protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"del","version":env!("CARGO_PKG_VERSION")}
        }), Some(SETUP_TIMEOUT)).await?;
        if init["protocolVersion"] != 1 { return Err(RpcError::new(-32603, "Agent did not negotiate ACP v1")); }
        let mut params = json!({"cwd":config.cwd,"mcpServers":[]});
        let mut history = Vec::new();
        let mut response = if let Some((_, remote)) = saved {
            if init["agentCapabilities"]["loadSession"] != true {
                return Err(RpcError::new(-32601, "This ACP agent does not support session/load"));
            }
            params["sessionId"] = json!(remote);
            // The agent replays the conversation as updates before answering;
            // collect them as the handle's history.
            {
                let mut state = client.state.lock().unwrap();
                state.remote = Some(remote.clone());
                state.replay = Some(Default::default());
            }
            let response = rpc.request("session/load", params, Some(SETUP_TIMEOUT)).await?;
            if let Some((mut translator, events)) = client.state.lock().unwrap().replay.take() {
                history = events;
                history.extend(translator.finish_replay());
            }
            response
        } else {
            rpc.request("session/new", params, Some(SETUP_TIMEOUT)).await?
        };
        let remote = saved.map(|s| s.1.clone()).or_else(|| response["sessionId"].as_str().map(str::to_owned))
            .ok_or_else(|| RpcError::invalid_params("session/new returned no sessionId"))?;
        client.state.lock().unwrap().remote = Some(remote.clone());
        if config.permission_mode == PermissionMode::Plan {
            let plan = response["modes"]["availableModes"].as_array()
                .and_then(|m| m.iter().find(|m| m["id"] == "plan"));
            if plan.is_none() { return Err(RpcError::new(-32601, "This ACP agent does not advertise plan mode")); }
            rpc.request("session/set_mode", json!({"sessionId":remote,"modeId":"plan"}), Some(SETUP_TIMEOUT)).await?;
        }
        if let Some(model) = model {
            if let Some(option) = crate::models::option(&response, "model") {
                let updated = rpc.request("session/set_config_option", json!({"sessionId":remote,"configId":option["id"],"value":model}), Some(SETUP_TIMEOUT)).await?;
                if updated["configOptions"].is_array() { response["configOptions"] = updated["configOptions"].clone(); }
            } else {
                rpc.request("session/set_model", json!({"sessionId":remote,"modelId":model}), Some(SETUP_TIMEOUT)).await?;
            }
        }
        if let Some(effort) = &config.reasoning_effort {
            let option = crate::models::option(&response, "thought_level")
                .ok_or_else(|| RpcError::invalid_params("This agent does not advertise reasoning effort"))?;
            if !crate::models::choices(option).iter().any(|o| o["value"].as_str() == Some(effort)) {
                return Err(RpcError::invalid_params("Unsupported reasoning effort for this agent/model"));
            }
            rpc.request("session/set_config_option", json!({"sessionId":remote,"configId":option["id"],"value":effort}), Some(SETUP_TIMEOUT)).await?;
        }
        Ok::<_, RpcError>((remote, crate::models::catalog(&agent.id, &agent.name, &response), history))
    }.await;
    let (remote, models, history) = match setup {
        Ok(result) => result,
        Err(e) => {
            client.state.lock().unwrap().closing = true;
            rpc.kill(CLOSE_GRACE).await;
            let hint = if e.is_auth_required() {
                " Sign in using the agent's own CLI, then retry."
            } else {
                ""
            };
            return Err(AgentError::Backend(format!("{}: {e}.{hint}", agent.name)));
        }
    };
    let id = SessionId::new(format!(
        "acp:{}",
        serde_json::to_string(&(&agent.id, &remote)).unwrap()
    ));
    guard.0 = None;
    Ok((
        id,
        Arc::new(Live {
            models,
            rpc,
            client,
            remote,
        }),
        history,
    ))
}

/// Start a throwaway session on `agent` just to read its model catalog.
async fn probe_models(agent: AcpAgentConfig, cwd: std::path::PathBuf) -> Result<Vec<ModelInfo>> {
    let config = SessionConfig {
        cwd,
        model: Some(agent.id.clone()),
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Acp,
        permission_mode: PermissionMode::Ask,
    };
    let (_, live, _) = spawn_live(&agent, &config, None, None).await?;
    live.retire().await;
    Ok(live.models.clone())
}

#[derive(Default)]
pub struct AcpBackend {
    agents: Option<Vec<AcpAgentConfig>>,
    sessions: Mutex<HashMap<SessionId, Arc<Live>>>,
    opening: tokio::sync::Mutex<()>,
    /// Successful model probes for this backend's lifetime, keyed by the
    /// agent's config and resolved executable so edits or reinstalls re-probe.
    /// Failures are not cached, so signing in later is picked up.
    model_cache: Mutex<HashMap<String, Vec<ModelInfo>>>,
}

impl AcpBackend {
    pub fn new(agents: Vec<AcpAgentConfig>) -> Self {
        Self {
            agents: Some(agents),
            sessions: Mutex::default(),
            opening: tokio::sync::Mutex::default(),
            model_cache: Mutex::default(),
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

    async fn open(
        &self,
        config: SessionConfig,
        resume: Option<&SessionId>,
    ) -> Result<SessionHandle> {
        let _opening = self.opening.lock().await;
        if let Some(id) = resume
            && let Ok(live) = self.live(id)
        {
            if live.alive() {
                return Ok(SessionHandle::new(
                    id.clone(),
                    Provider::Acp,
                    live.client.events.clone(),
                ));
            }
            // The agent exited (or is being retired): load the session afresh.
            {
                let mut sessions = self.sessions.lock().unwrap();
                if sessions.get(id).is_some_and(|l| Arc::ptr_eq(l, &live)) {
                    sessions.remove(id);
                }
            }
            live.rpc.kill(CLOSE_GRACE).await;
        }
        let saved: Option<(String, String)> = resume
            .map(|id| {
                serde_json::from_str(id.as_str().strip_prefix("acp:").unwrap_or(""))
                    .map_err(|_| AgentError::Backend("Invalid ACP session id".into()))
            })
            .transpose()?;
        let (selected, model) = registry::split_model(config.model.as_deref());
        let selected = saved.as_ref().map(|s| s.0.as_str()).or(selected);
        let agents = self.agents.clone().unwrap_or_else(|| registry::load().0);
        let agent = match selected {
            Some(id) => agents.iter().find(|a| a.id == id),
            None => agents.iter().find(|a| a.resolve().is_some()),
        }
        .ok_or_else(|| {
            AgentError::Backend("No matching installed ACP agent; configure acp-agents.json".into())
        })?;
        let (id, live, history) = spawn_live(agent, &config, saved.as_ref(), model).await?;
        let events = live.client.events.clone();
        self.sessions.lock().unwrap().insert(id.clone(), live);
        Ok(SessionHandle::new(id, Provider::Acp, events).with_history(history))
    }
}

#[async_trait]
impl AgentBackend for AcpBackend {
    async fn session_process(&self, id: &SessionId) -> Option<(u32, bool)> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)?
            .rpc
            .process_id()
            .map(|pid| (pid, false))
    }
    fn provider(&self) -> Provider {
        Provider::Acp
    }
    async fn list_models(&self, cwd: &std::path::Path) -> Result<Vec<ModelInfo>> {
        let agents = self.agents.clone().unwrap_or_else(|| registry::load().0);
        let mut slots: Vec<Option<Vec<ModelInfo>>> = Vec::new();
        // Probes run in parallel and do not hold `opening`, so a slow agent
        // never delays opening a chat.
        let mut probes = tokio::task::JoinSet::new();
        for agent in agents {
            let Some(program) = agent.resolve() else { continue };
            let key = format!(
                "{}\0{}",
                serde_json::to_string(&agent).unwrap_or_default(),
                program.display()
            );
            let index = slots.len();
            slots.push(self.model_cache.lock().unwrap().get(&key).cloned());
            if slots[index].is_none() {
                let cwd = cwd.to_path_buf();
                probes.spawn(async move {
                    let result = probe_models(agent.clone(), cwd).await;
                    (index, key, agent, result)
                });
            }
        }
        while let Some(joined) = probes.join_next().await {
            let (index, key, agent, result) = joined.map_err(|e| AgentError::Backend(e.to_string()))?;
            slots[index] = Some(match result {
                Ok(models) => {
                    self.model_cache.lock().unwrap().insert(key, models.clone());
                    models
                }
                Err(error) => vec![ModelInfo {
                    id: agent.id,
                    name: agent.name,
                    description: format!("Agent default · {error}"),
                    efforts: vec![],
                    default_effort: None,
                    is_default: false,
                    fast_tier: None,
                }],
            });
        }
        let mut models: Vec<ModelInfo> = slots.into_iter().flatten().flatten().collect();
        if let Some(first) = models.first_mut() {
            first.is_default = true;
        }
        Ok(models)
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.open(config, None).await
    }
    async fn resume(&self, config: SessionConfig, id: &SessionId) -> Result<SessionHandle> {
        self.open(config, Some(id)).await
    }

    async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        use base64::Engine as _;
        let mut content = vec![json!({"type":"text","text":input.text})];
        for attachment in &input.attachments {
            if let Some(mime) = attachment
                .mime
                .as_deref()
                .filter(|mime| mime.starts_with("image/"))
            {
                let bytes = tokio::fs::read(&attachment.path).await?;
                content.push(json!({"type":"image","mimeType":mime,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}));
            } else {
                content.push(json!({"type":"text","text":format!("[Attached file: {}]", attachment.path.display())}));
            }
        }
        let live = self.live(id)?;
        let rpc = live.rpc.clone();
        let response = {
            let mut state = live.client.state.lock().unwrap();
            if state.closed || state.closing {
                return Err(AgentError::Backend("ACP agent is closed".into()));
            }
            if state.translator.turn_id().is_some() {
                return Err(AgentError::Busy(id.to_string()));
            }
            state.cancelled = false;
            live.client.emit(
                state
                    .translator
                    .begin_turn(uuid::Uuid::new_v4().to_string()),
            );
            rpc.request(
                "session/prompt",
                json!({"sessionId":live.remote,"prompt":content}),
                None,
            )
        };
        tokio::spawn(async move {
            let result = response.await;
            live.client.finish(result);
        });
        Ok(())
    }
    async fn steer(&self, _id: &SessionId, _text: String) -> Result<()> {
        Err(AgentError::Backend(
            "ACP v1 has no standard steer method; stop the turn and send a new prompt".into(),
        ))
    }
    async fn interrupt(&self, id: &SessionId) -> Result<()> {
        let live = self.live(id)?;
        let turn = {
            let mut state = live.client.state.lock().unwrap();
            let Some(turn) = state.translator.turn_id().map(str::to_owned) else {
                return Ok(());
            };
            state.cancelled = true;
            state.approvals.clear();
            live.rpc
                .notify("session/cancel", json!({"sessionId":live.remote}));
            turn
        };
        // ACP requires the outstanding prompt to settle after cancellation.
        // Retire agents that do not comply rather than allow overlapping turns.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let stuck = {
                let mut state = live.client.state.lock().unwrap();
                let stuck = state.cancelled && state.translator.turn_id() == Some(turn.as_str());
                if stuck {
                    state.closing = true;
                    live.client
                        .emit(state.translator.end_turn(TurnStatus::Interrupted));
                }
                stuck
            };
            if stuck {
                live.rpc.kill(CLOSE_GRACE).await;
            }
        });
        Ok(())
    }
    async fn respond_approval(
        &self,
        id: &SessionId,
        approval_id: &str,
        answer: ApprovalResponse,
    ) -> Result<()> {
        let live = self.live(id)?;
        let pending = live
            .client
            .state
            .lock()
            .unwrap()
            .approvals
            .remove(approval_id)
            .ok_or_else(|| AgentError::Backend("Unknown or expired ACP approval".into()))?;
        pending
            .answer
            .send(answer)
            .map_err(|_| AgentError::Backend("ACP approval expired".into()))
    }
    async fn close(&self, id: &SessionId) -> Result<()> {
        let live = self.sessions.lock().unwrap().remove(id);
        if let Some(live) = live {
            live.retire().await;
        }
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        let ids: Vec<_> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.close(&id).await?;
        }
        Ok(())
    }
}

impl Drop for AcpBackend {
    fn drop(&mut self) {
        for live in self.sessions.get_mut().unwrap().values() {
            live.rpc.kill_now();
        }
    }
}
