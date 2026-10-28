//! Scriptable in-memory backend so UI can be built and tested without a model.
//!
//! The prompt text selects the scenario (case-insensitive substring):
//! `error` → streaming text then a recoverable error and a failed turn;
//! `approve` → a command tool call gated by an approval, then a file change;
//! `tool` → a tool call with a progress update and a file change;
//! anything else → streamed text with reasoning and token usage.

use crate::backend::*;
use crate::types::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

struct SessionState {
    provider: Provider,
    tx: broadcast::Sender<AgentEvent>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    turns: u64,
    task: Option<JoinHandle<()>>,
    current_turn: Option<String>,
    pending: HashMap<String, oneshot::Sender<ApprovalResponse>>,
}

#[derive(Clone)]
pub struct MockBackend {
    provider: Provider,
    delay: Duration,
    completion_delay: Duration,
    #[cfg(test)]
    pub title_response: Option<String>,
    /// Provider-owned name and the sessions it was read for.
    #[cfg(test)]
    pub session_title: Option<String>,
    #[cfg(test)]
    pub session_title_reads: Arc<Mutex<Vec<SessionId>>>,
    sessions: Arc<Mutex<HashMap<SessionId, Arc<SessionState>>>>,
}

impl MockBackend {
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            delay: Duration::ZERO,
            completion_delay: Duration::ZERO,
            #[cfg(test)]
            title_response: None,
            #[cfg(test)]
            session_title: None,
            #[cfg(test)]
            session_title_reads: Default::default(),
            sessions: Default::default(),
        }
    }

    /// Pause between scripted events; makes streaming visible in a UI.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Simulate a provider publishing completion before releasing its send slot.
    pub fn with_completion_delay(mut self, delay: Duration) -> Self {
        self.completion_delay = delay;
        self
    }

    fn get(&self, id: &SessionId) -> Result<Arc<SessionState>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))
    }

    fn register(&self, id: SessionId) -> SessionHandle {
        let (tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let state = Arc::new(SessionState {
            provider: self.provider,
            tx: tx.clone(),
            inner: Mutex::new(Inner::default()),
        });
        self.sessions.lock().unwrap().insert(id.clone(), state);
        SessionHandle::new(id, self.provider, tx)
    }
}

fn emit(state: &SessionState, ev: AgentEvent) {
    let _ = state.tx.send(ev);
}

async fn run_turn(state: Arc<SessionState>, turn_id: String, prompt: String, delay: Duration, completion_delay: Duration) {
    let pause = || async {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    };
    let lower = prompt.to_lowercase();
    emit(
        &state,
        AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
        },
    );
    pause().await;
    emit(
        &state,
        AgentEvent::ReasoningDelta {
            turn_id: turn_id.clone(),
            text: "Thinking it through. ".into(),
        },
    );

    let stream = |text: &str| {
        let t = turn_id.clone();
        text.split_inclusive(' ')
            .map(|w| AgentEvent::AssistantTextDelta {
                turn_id: t.clone(),
                text: w.to_string(),
            })
            .collect::<Vec<_>>()
    };
    let mut status = TurnStatus::Completed;
    let mut full = String::new();

    let intro = "Sure, let me take a look. ";
    full.push_str(intro);
    for ev in stream(intro) {
        pause().await;
        emit(&state, ev);
    }

    if lower.contains("error") {
        pause().await;
        emit(
            &state,
            AgentEvent::Error {
                message: "mock: upstream request failed".into(),
                recoverable: true,
            },
        );
        status = TurnStatus::Failed;
    } else if lower.contains("withdraw") {
        // The provider settles both requests itself, without our answer.
        for n in 1..=2 {
            emit(
                &state,
                AgentEvent::ApprovalRequested {
                    id: format!("appr-{turn_id}-{n}"),
                    kind: ApprovalKind::Command,
                    detail: "ls".into(),
                },
            );
        }
        for n in 1..=2 {
            pause().await;
            emit(
                &state,
                AgentEvent::ApprovalResolved {
                    id: format!("appr-{turn_id}-{n}"),
                },
            );
        }
        full.push_str("Never mind.");
    } else if lower.contains("approve") {
        let id = format!("call-{turn_id}");
        let approval = format!("appr-{turn_id}");
        emit(
            &state,
            AgentEvent::ToolCallStarted {
                id: id.clone(),
                name: "shell".into(),
                input_summary: "rm -rf build/".into(),
            },
        );
        let (tx, rx) = oneshot::channel();
        state
            .inner
            .lock()
            .unwrap()
            .pending
            .insert(approval.clone(), tx);
        emit(
            &state,
            AgentEvent::ApprovalRequested {
                id: approval,
                kind: ApprovalKind::Command,
                detail: "rm -rf build/".into(),
            },
        );
        match rx.await {
            Ok(ApprovalResponse::Approve) => {
                pause().await;
                emit(
                    &state,
                    AgentEvent::ToolCallFinished {
                        id,
                        status: ToolStatus::Completed,
                        output_summary: "removed build/".into(),
                    },
                );
                emit(
                    &state,
                    AgentEvent::FileChange {
                        path: PathBuf::from("build"),
                        kind: FileChangeKind::Deleted,
                        diff: None,
                    },
                );
                full.push_str("Done, cleaned up.");
            }
            _ => {
                emit(
                    &state,
                    AgentEvent::ToolCallFinished {
                        id,
                        status: ToolStatus::Declined,
                        output_summary: "denied by user".into(),
                    },
                );
                full.push_str("Understood, I left it alone.");
            }
        }
    } else if lower.contains("tool") {
        let id = format!("call-{turn_id}");
        pause().await;
        emit(
            &state,
            AgentEvent::ToolCallStarted {
                id: id.clone(),
                name: "read_file".into(),
                input_summary: "src/main.rs".into(),
            },
        );
        pause().await;
        emit(
            &state,
            AgentEvent::ToolCallUpdated {
                id: id.clone(),
                output_summary: "42 lines".into(),
            },
        );
        pause().await;
        emit(
            &state,
            AgentEvent::ToolCallFinished {
                id,
                status: ToolStatus::Completed,
                output_summary: "42 lines read".into(),
            },
        );
        emit(
            &state,
            AgentEvent::FileChange {
                path: PathBuf::from("src/main.rs"),
                kind: FileChangeKind::Modified,
                diff: Some("@@ -1 +1 @@\n-fn main() {}\n+fn main() { run() }\n".into()),
            },
        );
        full.push_str("I updated main.rs.");
    } else {
        full.push_str("Here is a streamed answer.");
    }

    // Stream whichever tail text was added (error scenario has none).
    let tail = &full[intro.len()..];
    for ev in stream(tail) {
        pause().await;
        emit(&state, ev);
    }
    emit(
        &state,
        AgentEvent::AssistantMessage {
            turn_id: turn_id.clone(),
            text: full,
        },
    );
    emit(
        &state,
        AgentEvent::TokenUsage {
            input: 120,
            output: 48,
            cached: 30,
        },
    );
    state.inner.lock().unwrap().current_turn = None;
    emit(&state, AgentEvent::TurnCompleted { turn_id, status });
    if !completion_delay.is_zero() { tokio::time::sleep(completion_delay).await; }
    let mut inner = state.inner.lock().unwrap();
    inner.task = None;
}

#[async_trait]
impl AgentBackend for MockBackend {
    fn provider(&self) -> Provider {
        self.provider
    }

    #[cfg(test)]
    async fn generate_title(&self, context: &str) -> Result<Option<String>> {
        assert!(context.contains("User:"));
        Ok(self.title_response.clone())
    }

    #[cfg(test)]
    async fn session_title(
        &self,
        session: &SessionId,
        _cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        self.session_title_reads.lock().unwrap().push(session.clone());
        Ok(self.session_title.clone())
    }

    async fn start_session(&self, _config: SessionConfig) -> Result<SessionHandle> {
        let id = SessionId::new(format!("mock-{}", uuid::Uuid::new_v4()));
        let handle = self.register(id.clone());
        emit(
            &*self.get(&id)?,
            AgentEvent::SessionStarted { session_id: id },
        );
        Ok(handle)
    }

    async fn send_prompt(&self, session: &SessionId, input: UserInput) -> Result<()> {
        let state = self.get(session)?;
        let mut inner = state.inner.lock().unwrap();
        if inner.task.as_ref().is_some_and(|t| !t.is_finished()) {
            return Err(AgentError::Busy("a turn is already running".into()));
        }
        inner.turns += 1;
        let turn_id = format!("turn-{}", inner.turns);
        inner.current_turn = Some(turn_id.clone());
        inner.task = Some(tokio::spawn(run_turn(
            state.clone(),
            turn_id,
            input.text,
            self.delay,
            self.completion_delay,
        )));
        Ok(())
    }

    async fn steer(&self, session: &SessionId, text: String) -> Result<()> {
        let state = self.get(session)?;
        let turn = state.inner.lock().unwrap().current_turn.clone();
        match turn {
            Some(turn_id) => {
                emit(
                    &state,
                    AgentEvent::ReasoningDelta {
                        turn_id,
                        text: format!("Steered: {text}"),
                    },
                );
                Ok(())
            }
            None => Err(AgentError::Backend("no turn in flight to steer".into())),
        }
    }

    async fn interrupt(&self, session: &SessionId) -> Result<()> {
        let state = self.get(session)?;
        let turn = {
            let mut inner = state.inner.lock().unwrap();
            if let Some(task) = inner.task.take() {
                task.abort();
            }
            inner.pending.clear();
            inner.current_turn.take()
        };
        if let Some(turn_id) = turn {
            emit(
                &state,
                AgentEvent::TurnCompleted {
                    turn_id,
                    status: TurnStatus::Interrupted,
                },
            );
        }
        Ok(())
    }

    async fn resume(&self, _config: SessionConfig, session: &SessionId) -> Result<SessionHandle> {
        if let Ok(state) = self.get(session) {
            return Ok(SessionHandle::new(
                session.clone(),
                state.provider,
                state.tx.clone(),
            ));
        }
        let handle = self.register(session.clone());
        emit(
            &*self.get(session)?,
            AgentEvent::SessionStarted {
                session_id: session.clone(),
            },
        );
        Ok(handle)
    }

    async fn respond_approval(
        &self,
        session: &SessionId,
        approval_id: &str,
        response: ApprovalResponse,
    ) -> Result<()> {
        let state = self.get(session)?;
        let tx = state.inner.lock().unwrap().pending.remove(approval_id);
        match tx {
            Some(tx) => {
                let _ = tx.send(response);
                Ok(())
            }
            None => Err(AgentError::Backend(format!(
                "no pending approval {approval_id}"
            ))),
        }
    }

    async fn close(&self, session: &SessionId) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .unwrap()
            .remove(session)
            .ok_or_else(|| AgentError::UnknownSession(session.clone()))?;
        if let Some(task) = state.inner.lock().unwrap().task.take() {
            task.abort();
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let all: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in all {
            if let Some(task) = s.inner.lock().unwrap().task.take() {
                task.abort();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SessionConfig {
        SessionConfig {
            cwd: ".".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider: Provider::Codex,
            permission_mode: PermissionMode::Ask,
        }
    }

    async fn collect_until_done(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = vec![];
        loop {
            let ev = rx.recv().await.unwrap();
            let done = matches!(ev, AgentEvent::TurnCompleted { .. });
            out.push(ev);
            if done {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn plain_text_streams_then_completes() {
        let b = MockBackend::new(Provider::Codex);
        let h = b.start_session(cfg()).await.unwrap();
        let mut rx = h.subscribe();
        b.send_prompt(&h.id, UserInput::text("hello"))
            .await
            .unwrap();
        let evs = collect_until_done(&mut rx).await;
        assert!(matches!(evs[0], AgentEvent::TurnStarted { .. }));
        let deltas: String = evs
            .iter()
            .filter_map(|e| match e {
                AgentEvent::AssistantTextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let msg = evs
            .iter()
            .find_map(|e| match e {
                AgentEvent::AssistantMessage { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(deltas, msg);
        assert!(matches!(
            evs.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn tool_scenario_pairs_start_and_finish() {
        let b = MockBackend::new(Provider::Claude);
        let h = b.start_session(cfg()).await.unwrap();
        let mut rx = h.subscribe();
        b.send_prompt(&h.id, UserInput::text("use a tool"))
            .await
            .unwrap();
        let evs = collect_until_done(&mut rx).await;
        assert!(
            evs.iter()
                .any(|e| matches!(e, AgentEvent::ToolCallStarted { .. }))
        );
        assert!(evs.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Completed,
                ..
            }
        )));
        assert!(
            evs.iter()
                .any(|e| matches!(e, AgentEvent::FileChange { diff: Some(_), .. }))
        );
    }

    #[tokio::test]
    async fn approval_approve_and_deny() {
        for (resp, expect) in [
            (ApprovalResponse::Approve, ToolStatus::Completed),
            (ApprovalResponse::Deny, ToolStatus::Declined),
        ] {
            let b = MockBackend::new(Provider::Codex);
            let h = b.start_session(cfg()).await.unwrap();
            let mut rx = h.subscribe();
            b.send_prompt(&h.id, UserInput::text("please approve"))
                .await
                .unwrap();
            let id = loop {
                if let AgentEvent::ApprovalRequested { id, .. } = rx.recv().await.unwrap() {
                    break id;
                }
            };
            b.respond_approval(&h.id, &id, resp).await.unwrap();
            let evs = collect_until_done(&mut rx).await;
            assert!(evs.iter().any(
                |e| matches!(e, AgentEvent::ToolCallFinished { status, .. } if *status == expect)
            ));
            assert!(
                b.respond_approval(&h.id, &id, resp).await.is_err(),
                "approval is single-use"
            );
        }
    }

    #[tokio::test]
    async fn error_scenario_fails_turn() {
        let b = MockBackend::new(Provider::Codex);
        let h = b.start_session(cfg()).await.unwrap();
        let mut rx = h.subscribe();
        b.send_prompt(&h.id, UserInput::text("trigger error"))
            .await
            .unwrap();
        let evs = collect_until_done(&mut rx).await;
        assert!(evs.iter().any(|e| matches!(
            e,
            AgentEvent::Error {
                recoverable: true,
                ..
            }
        )));
        assert!(matches!(
            evs.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Failed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn interrupt_during_approval_and_second_prompt_works() {
        let b = MockBackend::new(Provider::Codex);
        let h = b.start_session(cfg()).await.unwrap();
        let mut rx = h.subscribe();
        b.send_prompt(&h.id, UserInput::text("approve"))
            .await
            .unwrap();
        loop {
            if matches!(
                rx.recv().await.unwrap(),
                AgentEvent::ApprovalRequested { .. }
            ) {
                break;
            }
        }
        assert!(matches!(
            b.send_prompt(&h.id, UserInput::text("again")).await,
            Err(AgentError::Busy(_))
        ));
        b.interrupt(&h.id).await.unwrap();
        let evs = collect_until_done(&mut rx).await;
        assert!(matches!(
            evs.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            })
        ));
        b.send_prompt(&h.id, UserInput::text("again"))
            .await
            .unwrap();
        let evs = collect_until_done(&mut rx).await;
        assert!(matches!(
            evs.last(),
            Some(AgentEvent::TurnCompleted {
                status: TurnStatus::Completed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn steer_requires_running_turn_and_unknown_session_errors() {
        let b = MockBackend::new(Provider::Codex);
        let h = b.start_session(cfg()).await.unwrap();
        assert!(b.steer(&h.id, "x".into()).await.is_err());
        b.close(&h.id).await.unwrap();
        assert!(matches!(
            b.send_prompt(&h.id, UserInput::text("x")).await,
            Err(AgentError::UnknownSession(_))
        ));
    }

    #[tokio::test]
    async fn resume_reattaches_existing_and_creates_missing() {
        let b = MockBackend::new(Provider::Codex);
        let h = b.start_session(cfg()).await.unwrap();
        let h2 = b.resume(cfg(), &h.id).await.unwrap();
        let mut rx = h2.subscribe();
        b.send_prompt(&h.id, UserInput::text("hi")).await.unwrap();
        collect_until_done(&mut rx).await;
        let fresh = b.resume(cfg(), &SessionId::new("old")).await.unwrap();
        assert_eq!(fresh.id.as_str(), "old");
    }

    #[test]
    fn events_roundtrip_serde() {
        let ev = AgentEvent::ApprovalRequested {
            id: "a".into(),
            kind: ApprovalKind::Command,
            detail: "ls".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"approval_requested\""));
        assert_eq!(serde_json::from_str::<AgentEvent>(&s).unwrap(), ev);
    }
}
