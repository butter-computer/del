//! In-memory [`McpHost`] for tests (and for running the server without an app).

use crate::host::*;
use async_trait::async_trait;
use del_agent::{Provider, SessionId};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A prompt the mock is currently holding for `ask_user`.
pub struct PendingQuestion {
    pub caller: SessionId,
    pub question: String,
    pub options: Vec<String>,
    reply: oneshot::Sender<UserAnswer>,
}

impl PendingQuestion {
    pub fn answer(self, answer: UserAnswer) {
        let _ = self.reply.send(answer);
    }
}

#[derive(Default)]
struct State {
    sessions: Vec<SessionInfo>,
    summaries: HashMap<SessionId, SessionSummary>,
    reports: Vec<(SessionId, ReportedState, String)>,
    notifications: Vec<(SessionId, String, String)>,
    sent: Vec<(SessionId, SessionId, String)>,
    spawned: Vec<SpawnRequest>,
    questions: Vec<PendingQuestion>,
    next_id: u32,
}

/// Records every call; `ask_user` parks until the test calls
/// [`MockHost::take_question`] and answers it.
#[derive(Default)]
pub struct MockHost {
    state: Mutex<State>,
}

impl MockHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_session(&self, id: &str, provider: Provider) -> SessionInfo {
        let info = SessionInfo {
            id: SessionId::new(id),
            provider: Some(provider),
            mode: SessionMode::Chat,
            command: None,
            cwd: "/tmp".into(),
            title: None,
            status: "idle".into(),
            reported_state: None,
        };
        let mut s = self.state.lock().unwrap();
        s.summaries.insert(
            info.id.clone(),
            SessionSummary {
                info: info.clone(),
                reported_summary: None,
                last_output: Some(format!("output of {id}")),
            },
        );
        s.sessions.push(info.clone());
        info
    }

    pub fn reports(&self) -> Vec<(SessionId, ReportedState, String)> {
        self.state.lock().unwrap().reports.clone()
    }
    pub fn notifications(&self) -> Vec<(SessionId, String, String)> {
        self.state.lock().unwrap().notifications.clone()
    }
    pub fn sent(&self) -> Vec<(SessionId, SessionId, String)> {
        self.state.lock().unwrap().sent.clone()
    }
    pub fn spawned(&self) -> Vec<SpawnRequest> {
        self.state.lock().unwrap().spawned.clone()
    }

    /// Wait until `ask_user` has parked a question, then take it.
    pub async fn take_question(&self) -> PendingQuestion {
        loop {
            if let Some(q) = {
                let mut s = self.state.lock().unwrap();
                (!s.questions.is_empty()).then(|| s.questions.remove(0))
            } {
                return q;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

#[async_trait]
impl McpHost for MockHost {
    async fn report_status(
        &self,
        caller: &SessionId,
        state: ReportedState,
        summary: &str,
    ) -> HostResult<()> {
        let mut s = self.state.lock().unwrap();
        s.reports.push((caller.clone(), state, summary.into()));
        if let Some(i) = s.sessions.iter_mut().find(|i| &i.id == caller) {
            i.reported_state = Some(state);
        }
        Ok(())
    }

    async fn ask_user(
        &self,
        caller: &SessionId,
        question: &str,
        options: &[String],
    ) -> HostResult<UserAnswer> {
        let (tx, rx) = oneshot::channel();
        self.state.lock().unwrap().questions.push(PendingQuestion {
            caller: caller.clone(),
            question: question.into(),
            options: options.to_vec(),
            reply: tx,
        });
        Ok(rx.await.unwrap_or(UserAnswer::Dismissed))
    }

    async fn list_sessions(&self, _caller: &SessionId) -> HostResult<Vec<SessionInfo>> {
        Ok(self.state.lock().unwrap().sessions.clone())
    }

    async fn session_summary(
        &self,
        _caller: &SessionId,
        target: &SessionId,
    ) -> HostResult<SessionSummary> {
        self.state
            .lock()
            .unwrap()
            .summaries
            .get(target)
            .cloned()
            .ok_or_else(|| HostError::UnknownSession(target.to_string()))
    }

    async fn spawn_session(
        &self,
        _caller: &SessionId,
        request: SpawnRequest,
    ) -> HostResult<SessionInfo> {
        let id = {
            let mut s = self.state.lock().unwrap();
            s.next_id += 1;
            s.spawned.push(request.clone());
            format!("spawned-{}", s.next_id)
        };
        let mut info = self.add_session(&id, request.provider);
        info.cwd = request.cwd;
        Ok(info)
    }

    async fn send_to_session(
        &self,
        caller: &SessionId,
        target: &SessionId,
        text: &str,
    ) -> HostResult<()> {
        let mut s = self.state.lock().unwrap();
        if !s.sessions.iter().any(|i| &i.id == target) {
            return Err(HostError::UnknownSession(target.to_string()));
        }
        s.sent.push((caller.clone(), target.clone(), text.into()));
        Ok(())
    }

    async fn notify(&self, caller: &SessionId, title: &str, body: &str) -> HostResult<()> {
        self.state
            .lock()
            .unwrap()
            .notifications
            .push((caller.clone(), title.into(), body.into()));
        Ok(())
    }
}
