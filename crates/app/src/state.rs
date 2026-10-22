//! App state: the session manager, the sidebar selection and the draft
//! session, provider status, and the bridge from the tokio runtime that owns
//! the backends back into GPUI.

use crate::providers;
use crate::sessions::{self, Overrides, Row};
use del_agent::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Chat,
    Providers,
}

/// Which session is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// A session that does not exist yet; it starts on the first prompt, with
    /// whatever provider, model and folder the pickers hold.
    Draft,
    Session(SessionId),
}

/// Results coming back from the runtime.
enum Msg {
    Refresh,
    Statuses(Vec<ProviderStatus>),
    Started(SessionId, String),
    Error(String),
}

pub struct AppState {
    rt: Arc<Runtime>,
    manager: SessionManager,
    tx: UnboundedSender<Msg>,
    overrides: Overrides,
    records: Vec<SessionRecord>,
    pub selection: Selection,
    pub draft: SessionConfig,
    pub panel: Panel,
    pub providers: Vec<ProviderStatus>,
    /// Provider whose sign-in is running in the user's terminal.
    pub signin: Option<Provider>,
    /// Session being renamed in the sidebar.
    pub renaming: Option<SessionId>,
    pub show_archived: bool,
    /// Last problem worth telling the user about.
    pub error: Option<String>,
    /// The prompt that started each session opened this run, for its chat's
    /// first row (taken once by the chat view).
    first_prompts: HashMap<SessionId, String>,
    /// A draft is being turned into a session; blocks a double send.
    pub starting: bool,
}

impl AppState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("del-agent")
                .build()
                .expect("failed to start the async runtime"),
        );
        let (tx, rx) = unbounded_channel();

        let mut error = None;
        let index_path =
            default_index_path().unwrap_or_else(|| std::env::temp_dir().join("del-sessions.jsonl"));
        let manager = match SessionManager::new(&index_path, crate::backends()) {
            Ok(m) => m,
            Err(e) => {
                // A corrupt index must not block the app: fall back to a
                // throwaway one and say so.
                error = Some(format!("Could not read saved sessions: {e}"));
                let fallback =
                    std::env::temp_dir().join(format!("del-index-{}.jsonl", std::process::id()));
                SessionManager::new(&fallback, crate::backends()).expect("fallback session index")
            }
        };

        // Status changes anywhere refresh the sidebar.
        let mut events = manager.subscribe_all();
        let events_tx = tx.clone();
        rt.spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match events.recv().await {
                    Ok(ManagerEvent::StatusChanged { .. }) | Err(RecvError::Lagged(_)) => {
                        let _ = events_tx.send(Msg::Refresh);
                    }
                    Ok(_) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });

        let detect_tx = tx.clone();
        rt.spawn(async move {
            let _ = detect_tx.send(Msg::Statuses(detect_all().await));
        });

        Self::pump(rx, cx);

        let records = manager.list_sessions();
        let overrides = sessions::overrides_path()
            .map(|p| Overrides::load(&p))
            .unwrap_or_default();
        Self {
            rt,
            manager,
            tx,
            overrides,
            records,
            selection: Selection::Draft,
            draft: SessionConfig {
                cwd: std::env::current_dir()
                    .ok()
                    .filter(|d| d != std::path::Path::new("/"))
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/")),
                model: None,
                provider: Provider::Claude,
                permission_mode: PermissionMode::default(),
            },
            panel: Panel::Chat,
            providers: Provider::ALL
                .iter()
                .map(|&provider| ProviderStatus {
                    provider,
                    installed: false,
                    path: None,
                    version: None,
                    login: LoginState::Unknown,
                })
                .collect(),
            signin: None,
            renaming: None,
            show_archived: false,
            error,
            first_prompts: HashMap::new(),
            starting: false,
        }
    }

    /// Apply runtime results on the UI thread.
    fn pump(mut rx: UnboundedReceiver<Msg>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.recv().await {
                if this.update(cx, |s, cx| s.apply(msg, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Refresh => self.records = self.manager.list_sessions(),
            Msg::Statuses(list) => {
                // Default the draft to a usable provider once we know.
                let draft_ready = list
                    .iter()
                    .any(|s| s.provider == self.draft.provider && providers::ready(s));
                if !draft_ready
                    && self.selection == Selection::Draft
                    && let Some(ready) = list.iter().find(|s| providers::ready(s))
                {
                    self.draft.provider = ready.provider;
                    self.draft.model = None;
                }
                if let Some(p) = self.signin
                    && list.iter().any(|s| s.provider == p && providers::ready(s))
                {
                    self.signin = None;
                }
                self.providers = list;
            }
            Msg::Started(id, prompt) => {
                self.first_prompts.insert(id.clone(), prompt);
                self.starting = false;
                self.records = self.manager.list_sessions();
                self.selection = Selection::Session(id);
            }
            Msg::Error(e) => {
                self.starting = false;
                self.error = Some(e);
            }
        }
        cx.notify();
    }

    /// What a chat view needs to talk to the agents.
    pub fn backend(&self) -> crate::chat::Backend {
        crate::chat::Backend {
            runtime: self.rt.handle().clone(),
            manager: self.manager.clone(),
        }
    }

    /// The prompt that started `id` in this run, once.
    pub fn take_first_prompt(&mut self, id: &SessionId) -> Option<String> {
        self.first_prompts.remove(id)
    }

    // ---- reading ----

    /// Rows for the sidebar (not archived), most recent first.
    pub fn rows(&self) -> Vec<Row> {
        self.overrides
            .rows(&self.records)
            .into_iter()
            .filter(|r| !r.archived)
            .collect()
    }

    pub fn archived_rows(&self) -> Vec<Row> {
        self.overrides
            .rows(&self.records)
            .into_iter()
            .filter(|r| r.archived)
            .collect()
    }

    pub fn active_row(&self) -> Option<Row> {
        let Selection::Session(id) = &self.selection else {
            return None;
        };
        self.overrides
            .rows(&self.records)
            .into_iter()
            .find(|r| &r.id == id)
    }

    /// Provider, model and folder in effect: the open session's, or the draft's.
    pub fn effective(&self) -> (Provider, Option<String>, PathBuf) {
        match self.active_row() {
            Some(r) => (r.provider, r.model, r.cwd),
            None => (
                self.draft.provider,
                self.draft.model.clone(),
                self.draft.cwd.clone(),
            ),
        }
    }

    pub fn active_status(&self) -> SessionStatus {
        self.active_row().map_or(SessionStatus::Idle, |r| r.status)
    }

    pub fn status_of(&self, provider: Provider) -> Option<&ProviderStatus> {
        self.providers.iter().find(|s| s.provider == provider)
    }

    // ---- sessions ----

    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        self.selection = Selection::Draft;
        self.panel = Panel::Chat;
        self.renaming = None;
        cx.notify();
    }

    pub fn select(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.selection = Selection::Session(id.clone());
        self.panel = Panel::Chat;
        self.renaming = None;
        // Reattach in the background; a live session returns immediately.
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        self.rt.spawn(async move {
            if let Err(e) = manager.resume(&id).await {
                let _ = tx.send(Msg::Error(format!("Could not reopen session: {e}")));
            }
            let _ = tx.send(Msg::Refresh);
        });
        cx.notify();
    }

    /// Move through the sidebar by `delta` rows (wrapping).
    pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids: Vec<SessionId> = self.rows().into_iter().map(|r| r.id).collect();
        let current = match &self.selection {
            Selection::Session(id) => Some(id),
            Selection::Draft => None,
        };
        if let Some(next) = sessions::step(&ids, current, delta).cloned() {
            self.select(next, cx);
        }
    }

    /// Send a prompt to the open session, starting it first if it is a draft.
    pub fn send(&mut self, text: String, cx: &mut Context<Self>) {
        self.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        match self.selection.clone() {
            Selection::Draft => {
                if self.starting {
                    return;
                }
                self.starting = true;
                let config = self.draft.clone();
                self.rt.spawn(async move {
                    let result = async {
                        let handle = manager.start_session(config).await?;
                        manager
                            .send_prompt(&handle.id, UserInput::text(text.clone()))
                            .await?;
                        Ok::<_, AgentError>(handle.id)
                    }
                    .await;
                    let _ = tx.send(match result {
                        Ok(id) => Msg::Started(id, text),
                        Err(e) => Msg::Error(e.to_string()),
                    });
                });
            }
            Selection::Session(id) => {
                self.rt.spawn(async move {
                    let result = async {
                        manager.resume(&id).await?;
                        manager.send_prompt(&id, UserInput::text(text)).await
                    }
                    .await;
                    if let Err(e) = result {
                        let _ = tx.send(Msg::Error(e.to_string()));
                    }
                    let _ = tx.send(Msg::Refresh);
                });
            }
        }
        cx.notify();
    }

    /// Stop the turn in flight in the open session.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let Selection::Session(id) = self.selection.clone() else {
            return;
        };
        if self.active_status() != SessionStatus::Running {
            return;
        }
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        self.rt.spawn(async move {
            if let Err(e) = manager.interrupt(&id).await {
                let _ = tx.send(Msg::Error(format!("Could not stop: {e}")));
            }
        });
        cx.notify();
    }

    /// Archive: hide from the list and end the live session (killing its
    /// child process). It can be restored and resumed later.
    pub fn archive(&mut self, id: SessionId, cx: &mut Context<Self>) {
        let was_active = self.selection == Selection::Session(id.clone());
        self.overrides.set_archived(&id, true);
        self.save_overrides();
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        let close_id = id.clone();
        self.rt.spawn(async move {
            let _ = manager.close(&close_id).await;
            let _ = tx.send(Msg::Refresh);
        });
        if was_active {
            self.selection = Selection::Draft;
            if let Some(next) = self.rows().into_iter().next() {
                self.select(next.id, cx);
            }
        }
        cx.notify();
    }

    pub fn restore(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.overrides.set_archived(&id, false);
        self.save_overrides();
        self.select(id, cx);
    }

    pub fn start_rename(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.renaming = Some(id);
        cx.notify();
    }

    /// `None` cancels.
    pub fn finish_rename(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        if let (Some(id), Some(title)) = (self.renaming.take(), title) {
            self.overrides.rename(&id, &title);
            self.save_overrides();
        }
        cx.notify();
    }

    pub fn current_title(&self, id: &SessionId) -> String {
        self.overrides
            .rows(&self.records)
            .into_iter()
            .find(|r| &r.id == id)
            .map(|r| r.title)
            .unwrap_or_default()
    }

    fn save_overrides(&mut self) {
        if let Some(path) = sessions::overrides_path()
            && let Err(e) = self.overrides.save(&path)
        {
            self.error = Some(format!("Could not save sidebar changes: {e}"));
        }
    }

    // ---- composer pickers (a draft's choices; an open session's are fixed) ----

    fn is_draft(&self) -> bool {
        self.selection == Selection::Draft
    }

    pub fn cycle_provider(&mut self, cx: &mut Context<Self>) {
        if self.is_draft() {
            self.draft.provider = providers::next_provider(self.draft.provider);
            self.draft.model = None;
            cx.notify();
        }
    }

    pub fn cycle_model(&mut self, cx: &mut Context<Self>) {
        if self.is_draft() {
            self.draft.model =
                providers::next_model(self.draft.provider, self.draft.model.as_deref());
            cx.notify();
        }
    }

    /// Native folder dialog; the choice becomes the draft's working directory.
    pub fn pick_folder(&mut self, cx: &mut Context<Self>) {
        if !self.is_draft() {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |s, cx| {
                    s.draft.cwd = dir;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // ---- providers ----

    pub fn toggle_providers(&mut self, cx: &mut Context<Self>) {
        self.panel = if self.panel == Panel::Providers {
            Panel::Chat
        } else {
            Panel::Providers
        };
        if self.panel == Panel::Providers {
            self.recheck(cx);
        }
        cx.notify();
    }

    pub fn recheck(&mut self, cx: &mut Context<Self>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Statuses(detect_all().await));
        });
        cx.notify();
    }

    /// Run the CLI's own login in the user's terminal, then poll its status
    /// command until it reports signed in (or five minutes pass).
    pub fn sign_in(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let Some(path) = self.status_of(provider).and_then(|s| s.path.clone()) else {
            return;
        };
        if let Err(e) = providers::launch_login(provider, &path) {
            self.error = Some(format!("Could not open Terminal: {e}"));
            cx.notify();
            return;
        }
        self.signin = Some(provider);
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let all = detect_all().await;
                let done = all
                    .iter()
                    .any(|s| s.provider == provider && s.login == LoginState::LoggedIn);
                let _ = tx.send(Msg::Statuses(all));
                if done {
                    return;
                }
            }
        });
        cx.notify();
    }

    /// Close every session and kill every backend child process. Blocks, with
    /// a deadline, because the app is quitting.
    pub fn shutdown(&self) {
        let manager = self.manager.clone();
        self.rt.block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3), manager.shutdown()).await;
        });
    }
}
