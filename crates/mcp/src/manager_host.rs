//! [`McpHost`] over `del_agent::SessionManager`.
//!
//! Session-facing tools (list, summary, spawn, send) go straight to the
//! manager. The three UI-facing ones go through [`McpUi`], which is all the app
//! (GPUI side) has to implement.

use crate::host::*;
use async_trait::async_trait;
use del_agent::{
    AgentError, AgentEvent, PermissionMode, SessionConfig, SessionId, SessionManager,
    SessionRecord, SessionStatus, UserInput,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The UI half of the host. Implementations must be cheap and non-blocking
/// except `ask_user`, which resolves when the user answers.
#[async_trait]
pub trait McpUi: Send + Sync + 'static {
    /// Show `question` for `caller` and wait. Dropping the future cancels it.
    async fn ask_user(&self, caller: &SessionId, question: &str, options: &[String]) -> UserAnswer;
    /// An agent reported what it is doing.
    fn status_reported(&self, caller: &SessionId, state: ReportedState, summary: &str);
    fn notify(&self, caller: &SessionId, title: &str, body: &str);
}

const LAST_OUTPUT_CHARS: usize = 2_000;

pub struct ManagerHost {
    manager: SessionManager,
    ui: Arc<dyn McpUi>,
    reported: Mutex<HashMap<SessionId, (ReportedState, String)>>,
}

fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::WaitingApproval => "waiting_approval",
        SessionStatus::Error => "error",
    }
}

fn fail(e: AgentError) -> HostError {
    match e {
        AgentError::UnknownSession(id) => HostError::UnknownSession(id.to_string()),
        e => HostError::Failed(e.to_string()),
    }
}

impl ManagerHost {
    pub fn new(manager: SessionManager, ui: Arc<dyn McpUi>) -> Self {
        Self {
            manager,
            ui,
            reported: Mutex::new(HashMap::new()),
        }
    }

    fn info(&self, r: &SessionRecord) -> SessionInfo {
        let status = self.manager.status(&r.id).unwrap_or(r.status);
        SessionInfo {
            id: r.id.clone(),
            provider: Some(r.provider),
            mode: SessionMode::Chat,
            command: None,
            cwd: r.cwd.clone(),
            title: r.title.clone(),
            status: status_str(status).into(),
            reported_state: self.reported.lock().unwrap().get(&r.id).map(|(s, _)| *s),
        }
    }

    fn record(&self, id: &SessionId) -> HostResult<SessionRecord> {
        self.manager
            .list_sessions()
            .into_iter()
            .find(|r| &r.id == id)
            .ok_or_else(|| HostError::UnknownSession(id.to_string()))
    }
}

#[async_trait]
impl McpHost for ManagerHost {
    async fn report_status(
        &self,
        caller: &SessionId,
        state: ReportedState,
        summary: &str,
    ) -> HostResult<()> {
        self.reported
            .lock()
            .unwrap()
            .insert(caller.clone(), (state, summary.to_string()));
        self.ui.status_reported(caller, state, summary);
        Ok(())
    }

    async fn ask_user(
        &self,
        caller: &SessionId,
        question: &str,
        options: &[String],
    ) -> HostResult<UserAnswer> {
        Ok(self.ui.ask_user(caller, question, options).await)
    }

    async fn list_sessions(&self, _caller: &SessionId) -> HostResult<Vec<SessionInfo>> {
        Ok(self
            .manager
            .list_sessions()
            .iter()
            .map(|r| self.info(r))
            .collect())
    }

    async fn session_summary(
        &self,
        _caller: &SessionId,
        target: &SessionId,
    ) -> HostResult<SessionSummary> {
        let record = self.record(target)?;
        let last_output = self
            .manager
            .subscribe_with_history(target)
            .ok()
            .and_then(|(h, _)| {
                h.into_iter().rev().find_map(|e| match e {
                    AgentEvent::AssistantMessage { text, .. } => Some(text),
                    _ => None,
                })
            });
        let last_output = last_output.map(|t| {
            let n = t.chars().count();
            if n > LAST_OUTPUT_CHARS {
                t.chars().skip(n - LAST_OUTPUT_CHARS).collect()
            } else {
                t
            }
        });
        Ok(SessionSummary {
            info: self.info(&record),
            reported_summary: self
                .reported
                .lock()
                .unwrap()
                .get(target)
                .map(|(_, s)| s.clone()),
            last_output,
        })
    }

    async fn spawn_session(
        &self,
        _caller: &SessionId,
        request: SpawnRequest,
    ) -> HostResult<SessionInfo> {
        let handle = self
            .manager
            .start_session(SessionConfig {
                cwd: request.cwd,
                model: None,
                reasoning_effort: None,
                service_tier: None,
                provider: request.provider,
                permission_mode: PermissionMode::default(),
            })
            .await
            .map_err(fail)?;
        self.manager
            .send_prompt(&handle.id, UserInput::text(request.prompt))
            .await
            .map_err(fail)?;
        let record = self.record(&handle.id)?;
        Ok(self.info(&record))
    }

    async fn send_to_session(
        &self,
        _caller: &SessionId,
        target: &SessionId,
        text: &str,
    ) -> HostResult<()> {
        if self.manager.status(target) == Some(SessionStatus::Running) {
            self.manager
                .steer(target, text.to_string())
                .await
                .map_err(fail)
        } else {
            self.manager
                .send_prompt(target, UserInput::text(text))
                .await
                .map_err(fail)
        }
    }

    async fn notify(&self, caller: &SessionId, title: &str, body: &str) -> HostResult<()> {
        self.ui.notify(caller, title, body);
        Ok(())
    }
}
