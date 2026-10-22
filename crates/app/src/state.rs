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
    /// A session that does not exist yet (a "new tab"); it starts on the first
    /// prompt, with the provider, model and folder its own pickers hold.
    Draft(u64),
    Session(SessionId),
}

/// One tab in the top bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    Draft(u64),
    Session(SessionId),
}

impl Tab {
    pub fn selection(&self) -> Selection {
        match self {
            Tab::Draft(k) => Selection::Draft(*k),
            Tab::Session(id) => Selection::Session(id.clone()),
        }
    }
}

/// Results coming back from the runtime.
enum Msg {
    Refresh,
    Statuses(Vec<ProviderStatus>),
    /// A draft tab became a session: (draft key, session, first prompt).
    Started(u64, SessionId, String),
    Error(String),
}

pub struct AppState {
    rt: Arc<Runtime>,
    manager: SessionManager,
    tx: UnboundedSender<Msg>,
    overrides: Overrides,
    records: Vec<SessionRecord>,
    pub selection: Selection,
    drafts: HashMap<u64, SessionConfig>,
    next_draft: u64,
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
    /// Tabs in the top bar, in order: new-session drafts and sessions opened this run.
    pub tabs: Vec<Tab>,
    /// A draft is being turned into a session; blocks a double send.
    starting: std::collections::HashSet<u64>,
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
            selection: Selection::Draft(0),
            drafts: HashMap::from([(
                0,
                SessionConfig {
                    cwd: std::env::current_dir()
                        .ok()
                        .filter(|d| d != std::path::Path::new("/"))
                        .or_else(dirs::home_dir)
                        .unwrap_or_else(|| PathBuf::from("/")),
                    model: None,
                    provider: Provider::Claude,
                    permission_mode: PermissionMode::default(),
                },
            )]),
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
            starting: Default::default(),
            tabs: vec![Tab::Draft(0)],
            next_draft: 1,
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
                if let Some(ready) = list.iter().find(|s| providers::ready(s)) {
                    for draft in self.drafts.values_mut() {
                        let draft_ready = list
                            .iter()
                            .any(|s| s.provider == draft.provider && providers::ready(s));
                        if !draft_ready {
                            draft.provider = ready.provider;
                            draft.model = None;
                        }
                    }
                }
                if let Some(p) = self.signin
                    && list.iter().any(|s| s.provider == p && providers::ready(s))
                {
                    self.signin = None;
                }
                self.providers = list;
            }
            Msg::Started(key, id, prompt) => {
                self.first_prompts.insert(id.clone(), prompt);
                self.starting.remove(&key);
                self.drafts.remove(&key);
                // The draft tab turns into the session's tab, in place.
                match self.tabs.iter().position(|t| *t == Tab::Draft(key)) {
                    Some(at) => self.tabs[at] = Tab::Session(id.clone()),
                    None => self.add_tab(&id),
                }
                self.records = self.manager.list_sessions();
                if self.selection == Selection::Draft(key) {
                    self.selection = Selection::Session(id);
                }
            }
            Msg::Error(e) => {
                self.starting.clear();
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
            None => match self.draft() {
                Some(d) => (d.provider, d.model.clone(), d.cwd.clone()),
                None => (Provider::Claude, None, PathBuf::from("/")),
            },
        }
    }

    pub fn active_status(&self) -> SessionStatus {
        self.active_row().map_or(SessionStatus::Idle, |r| r.status)
    }

    pub fn status_of(&self, provider: Provider) -> Option<&ProviderStatus> {
        self.providers.iter().find(|s| s.provider == provider)
    }

    // ---- sessions ----

    /// The open draft tab's config, if a draft is open.
    fn draft(&self) -> Option<&SessionConfig> {
        match self.selection {
            Selection::Draft(k) => self.drafts.get(&k),
            Selection::Session(_) => None,
        }
    }

    fn draft_mut(&mut self) -> Option<&mut SessionConfig> {
        match self.selection {
            Selection::Draft(k) => self.drafts.get_mut(&k),
            Selection::Session(_) => None,
        }
    }

    /// Whether the open draft is being turned into a session.
    pub fn is_starting(&self) -> bool {
        matches!(self.selection, Selection::Draft(k) if self.starting.contains(&k))
    }

    /// Open a new tab, starting from the provider, model and folder of the
    /// current one. Any number can be open.
    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        let (provider, model, cwd) = self.effective();
        self.open_draft(provider, model, cwd, cx);
    }

    fn open_draft(
        &mut self,
        provider: Provider,
        model: Option<String>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let key = self.next_draft;
        self.next_draft += 1;
        self.drafts.insert(
            key,
            SessionConfig {
                cwd,
                model,
                provider,
                permission_mode: PermissionMode::default(),
            },
        );
        self.tabs.push(Tab::Draft(key));
        self.selection = Selection::Draft(key);
        self.panel = Panel::Chat;
        self.renaming = None;
        cx.notify();
    }

    /// The agent or model of an existing session cannot change, so choosing
    /// another in its composer opens a new tab with that choice, in the same
    /// folder.
    pub fn fork_with(&mut self, provider: Provider, model: Option<String>, cx: &mut Context<Self>) {
        let (_, _, cwd) = self.effective();
        self.open_draft(provider, model, cwd, cx);
    }

    fn add_tab(&mut self, id: &SessionId) {
        let tab = Tab::Session(id.clone());
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab);
        }
    }

    pub fn select_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        match tab {
            Tab::Session(id) => self.select(id.clone(), cx),
            Tab::Draft(k) => {
                self.selection = Selection::Draft(*k);
                self.panel = Panel::Chat;
                self.renaming = None;
                cx.notify();
            }
        }
    }

    /// Close a tab. A session stays in the sidebar list and keeps running.
    pub fn close_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t == tab) else {
            return;
        };
        self.tabs.remove(at);
        if let Tab::Draft(k) = tab {
            self.drafts.remove(k);
            self.starting.remove(k);
        }
        if self.selection == tab.selection() {
            let next = self
                .tabs
                .get(at)
                .or_else(|| at.checked_sub(1).and_then(|j| self.tabs.get(j)))
                .cloned();
            match next {
                Some(next) => self.select_tab(&next, cx),
                None => self.new_session(cx),
            }
        }
        cx.notify();
    }

    /// Close the open tab (Cmd-W).
    pub fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        let tab = match &self.selection {
            Selection::Draft(k) => Tab::Draft(*k),
            Selection::Session(id) => Tab::Session(id.clone()),
        };
        self.close_tab(&tab, cx);
    }

    /// Jump to tab `n` (0-based), as Cmd-1..9 do.
    pub fn select_nth(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(n).cloned() {
            self.select_tab(&tab, cx);
        }
    }

    pub fn select(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.add_tab(&id);
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

    /// Move through the tabs by `delta` (wrapping).
    pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        let Some(at) = self
            .tabs
            .iter()
            .position(|t| t.selection() == self.selection)
        else {
            return;
        };
        let next = (at as isize + delta).rem_euclid(n) as usize;
        self.select_nth(next, cx);
    }

    /// Send a prompt to the open session, starting it first if it is a draft.
    pub fn send(&mut self, text: String, cx: &mut Context<Self>) {
        self.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        match self.selection.clone() {
            Selection::Draft(key) => {
                if !self.starting.insert(key) {
                    return;
                }
                let Some(config) = self.drafts.get(&key).cloned() else {
                    return;
                };
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
                        Ok(id) => Msg::Started(key, id, text),
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
        self.close_tab(&Tab::Session(id), cx);
        let _ = was_active;
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

    /// The agent a session runs on.
    /// Title and agent shown on a tab.
    pub fn tab_info(&self, tab: &Tab) -> (String, Provider) {
        match tab {
            Tab::Session(id) => (
                self.current_title(id),
                self.provider_of(id).unwrap_or(Provider::Claude),
            ),
            Tab::Draft(k) => (
                sessions::DEFAULT_TITLE.to_string(),
                self.drafts.get(k).map_or(Provider::Claude, |d| d.provider),
            ),
        }
    }

    pub fn provider_of(&self, id: &SessionId) -> Option<Provider> {
        self.records
            .iter()
            .find(|r| &r.id == id)
            .map(|r| r.provider)
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

    pub fn is_draft(&self) -> bool {
        matches!(self.selection, Selection::Draft(_))
    }

    pub fn set_provider(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if let Some(d) = self.draft_mut() {
            if d.provider != provider {
                d.provider = provider;
                d.model = None;
                cx.notify();
            }
        } else if self.effective().0 != provider {
            self.fork_with(provider, None, cx);
        }
    }

    pub fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if let Some(d) = self.draft_mut() {
            d.model = model;
            cx.notify();
        } else {
            let (provider, current, _) = self.effective();
            if current != model {
                self.fork_with(provider, model, cx);
            }
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
                    if let Some(d) = s.draft_mut() {
                        d.cwd = dir;
                    }
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
