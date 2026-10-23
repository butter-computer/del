//! App state: the session manager, the sidebar selection and the draft
//! session, provider status, and the bridge from the tokio runtime that owns
//! the backends back into GPUI.

use crate::providers;
use crate::sessions::{self, Overrides, Row, TerminalRow};
use del_agent::*;
use del_mcp::SessionMode;
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
    Terminal(SessionId),
}

/// How long a tab takes to grow in or shrink away.
pub const TAB_ANIM: Duration = Duration::from_millis(180);

/// A tab that was just closed, kept on screen while it shrinks away.
#[derive(Debug, Clone)]
pub struct Ghost {
    pub uid: u64,
    /// Where it was in the tab list.
    pub at: usize,
    pub title: String,
    pub provider: Option<Provider>,
}

/// One tab in the top bar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tab {
    Draft(u64),
    Session(SessionId),
    Terminal(SessionId),
}

impl Tab {
    pub fn selection(&self) -> Selection {
        match self {
            Tab::Draft(k) => Selection::Draft(*k),
            Tab::Session(id) => Selection::Session(id.clone()),
            Tab::Terminal(id) => Selection::Terminal(id.clone()),
        }
    }
}

/// Results coming back from the runtime.
enum Msg {
    Refresh,
    UpdateProgress(String, Option<String>),
    ReplyFinished(SessionId),
    Statuses(Vec<ProviderStatus>),
    Checked(ProviderStatus),
    SigninFinished(Provider),
    /// A draft tab became a session: (draft key, session, first prompt).
    Started(u64, SessionId, String),
    StartFailed(u64, String),
    Models(
        Provider,
        PathBuf,
        std::result::Result<Vec<ModelInfo>, String>,
    ),
    Error(String),
}

#[derive(Default)]
struct PendingPrompt {
    text: String,
    input: UserInput,
    error: Option<String>,
}

#[derive(Clone, Default)]
pub struct ModelCatalog {
    pub models: Vec<ModelInfo>,
    pub loading: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

pub struct AppState {
    catalogs: HashMap<(Provider, PathBuf), ModelCatalog>,
    rt: Arc<Runtime>,
    manager: SessionManager,
    tx: UnboundedSender<Msg>,
    overrides: Overrides,
    records: Vec<SessionRecord>,
    pub selection: Selection,
    drafts: HashMap<u64, SessionConfig>,
    next_draft: u64,
    draft_modes: HashMap<u64, SessionMode>,
    pub terminal_rows: Vec<TerminalRow>,
    pub update_progress: HashMap<String, String>,
    updates_enabled: bool,
    unread_motion: HashMap<SessionId, (f32, std::time::Instant)>,
    pub terminal_title_loading: std::collections::HashSet<SessionId>,
    pub closed_terminals: Vec<SessionId>,
    pub removed_terminals: Vec<SessionId>,
    pub terminal_pin_changes: Vec<(SessionId, bool)>,
    pub panel: Panel,
    pub preferences_open: bool,
    pub shortcut_hints: bool,
    pub sidebar_hover: crate::hover_motion::HoverMotion,
    pub providers: Vec<ProviderStatus>,
    /// Provider whose sign-in is running in the user's terminal.
    pub signin: Option<Provider>,
    pub checking: std::collections::HashSet<Provider>,
    pub caffeine_enabled: bool,
    /// Session being renamed in the sidebar.
    pub renaming: Option<SessionId>,
    pub show_archived: bool,
    pub terminals_collapsed: bool,
    pub terminals_motion: Option<(std::time::Instant, f32)>,
    pub terminals_toggle: u64,
    /// Last problem worth telling the user about.
    pub error: Option<String>,
    /// The prompt that started each session opened this run, for its chat's
    /// first row (taken once by the chat view).
    first_prompts: HashMap<SessionId, String>,
    /// Submitted text stays visible while startup runs, including on failure.
    pending_prompts: HashMap<u64, PendingPrompt>,
    /// Tabs in the top bar, in order: new-session drafts and sessions opened this run.
    pub tabs: Vec<Tab>,
    /// A stable id per tab (kept when a draft becomes its session), so each
    /// tab animates in exactly once.
    uids: HashMap<Tab, u64>,
    next_uid: u64,
    pub ghosts: Vec<Ghost>,
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
                    Ok(ManagerEvent::Event {
                        session,
                        event:
                            del_agent::AgentEvent::TurnCompleted {
                                status: del_agent::TurnStatus::Completed,
                                ..
                            },
                    }) => {
                        let _ = events_tx.send(Msg::ReplyFinished(session));
                    }
                    Ok(_) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });

        let title_manager = manager.clone();
        let title_tx = tx.clone();
        rt.spawn(async move {
            loop {
                if title_manager.refresh_titles().await.unwrap_or(false)
                    && title_tx.send(Msg::Refresh).is_err()
                {
                    break;
                }
                if title_tx.is_closed() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });

        let detect_tx = tx.clone();
        rt.spawn(async move {
            let _ = detect_tx.send(Msg::Statuses(providers::detect_all().await));
        });

        Self::pump(rx, cx);

        let records = manager.list_sessions();
        let overrides = sessions::overrides_path()
            .map(|p| Overrides::load(&p))
            .unwrap_or_default();
        Self {
            catalogs: HashMap::new(),
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
                    reasoning_effort: None,
                    service_tier: None,
                    provider: Provider::Claude,
                    permission_mode: PermissionMode::default(),
                },
            )]),
            panel: Panel::Chat,
            preferences_open: true,
            shortcut_hints: false,
            sidebar_hover: Default::default(),
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
            checking: Default::default(),
            caffeine_enabled: false,
            renaming: None,
            show_archived: false,
            terminals_collapsed: false,
            terminals_motion: None,
            terminals_toggle: 0,
            error,
            first_prompts: HashMap::new(),
            pending_prompts: HashMap::new(),
            starting: Default::default(),
            tabs: vec![Tab::Draft(0)],
            uids: HashMap::from([(Tab::Draft(0), 0)]),
            next_uid: 1,
            ghosts: Vec::new(),
            next_draft: 1,
            draft_modes: HashMap::new(),
            terminal_rows: Vec::new(),
            terminal_title_loading: Default::default(),
            unread_motion: HashMap::new(),
            update_progress: HashMap::new(),
            updates_enabled: !std::env::var("DEL_BACKEND")
                .is_ok_and(|v| v.eq_ignore_ascii_case("mock")),
            closed_terminals: Vec::new(),
            removed_terminals: Vec::new(),
            terminal_pin_changes: Vec::new(),
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
            Msg::Models(provider, cwd, result) => {
                let catalog = self.catalogs.entry((provider, cwd)).or_default();
                catalog.loading = false;
                catalog.loaded = true;
                match result {
                    Ok(models) => {
                        catalog.models = models;
                        catalog.error = None;
                    }
                    Err(error) => {
                        catalog.error = Some(error.clone());
                        self.error = Some(format!(
                            "Could not load models: {error}. Try Refresh again."
                        ));
                    }
                }
            }
            Msg::Refresh => self.records = self.manager.list_sessions(),
            Msg::UpdateProgress(key, message) => {
                if let Some(message) = message {
                    self.update_progress.insert(key, message);
                } else {
                    self.update_progress.remove(&key);
                }
            }
            Msg::ReplyFinished(id) => {
                self.reply_finished(&id, cx);
                self.records = self.manager.list_sessions();
            }
            Msg::SigninFinished(provider) => {
                if self.signin == Some(provider) {
                    self.signin = None;
                }
            }
            Msg::Checked(status) => {
                self.checking.remove(&status.provider);
                if self.signin == Some(status.provider) && providers::ready(&status) {
                    self.signin = None;
                }
                if let Some(old) = self
                    .providers
                    .iter_mut()
                    .find(|p| p.provider == status.provider)
                {
                    *old = status;
                } else {
                    self.providers.push(status);
                }
            }
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
                            draft.reasoning_effort = None;
                            draft.service_tier = None;
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
                self.pending_prompts.remove(&key);
                self.starting.remove(&key);
                self.drafts.remove(&key);
                // The draft tab turns into the session's tab, in place.
                match self.tabs.iter().position(|t| *t == Tab::Draft(key)) {
                    Some(at) => {
                        self.tabs[at] = Tab::Session(id.clone());
                        if let Some(uid) = self.uids.remove(&Tab::Draft(key)) {
                            self.uids.insert(Tab::Session(id.clone()), uid);
                        }
                    }
                    None => self.add_tab(&id),
                }
                self.records = self.manager.list_sessions();
                if self.selection == Selection::Draft(key) {
                    self.selection = Selection::Session(id);
                }
            }
            Msg::StartFailed(key, error) => {
                self.error = Some(format!(
                    "Could not start the session: {error}. Your message is saved; use Retry message."
                ));
                self.starting.remove(&key);
                if let Some(pending) = self.pending_prompts.get_mut(&key) {
                    pending.error = Some(error);
                }
            }
            Msg::Error(e) => {
                self.error = Some(e);
            }
        }
        cx.notify();
    }

    /// What a chat view needs to talk to the agents.
    pub fn update_reporter(&self, key: String) -> impl Fn(Option<String>) + Send + 'static {
        let tx = self.tx.clone();
        move |message| {
            let _ = tx.send(Msg::UpdateProgress(key.clone(), message));
        }
    }

    pub fn current_update(&self) -> Option<&str> {
        let key = match &self.selection {
            Selection::Draft(key) => format!("draft-{key}"),
            Selection::Session(id) | Selection::Terminal(id) => id.0.clone(),
        };
        self.update_progress.get(&key).map(String::as_str)
    }

    pub fn backend(&self) -> crate::chat::Backend {
        let tx = self.tx.clone();
        crate::chat::Backend {
            runtime: self.rt.handle().clone(),
            manager: self.manager.clone(),
            updates_enabled: self.updates_enabled,
            update_progress: std::sync::Arc::new(move |key, message| {
                let _ = tx.send(Msg::UpdateProgress(key, message));
            }),
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
        if let Selection::Terminal(id) = &self.selection
            && let Some(row) = self.terminal_rows.iter().find(|r| &r.id == id)
        {
            return (
                row.provider.unwrap_or(Provider::Claude),
                None,
                row.cwd.clone(),
            );
        }
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
            Selection::Session(_) | Selection::Terminal(_) => None,
        }
    }

    fn draft_mut(&mut self) -> Option<&mut SessionConfig> {
        match self.selection {
            Selection::Draft(k) => self.drafts.get_mut(&k),
            Selection::Session(_) | Selection::Terminal(_) => None,
        }
    }

    pub fn mode(&self) -> SessionMode {
        match &self.selection {
            Selection::Terminal(_) => SessionMode::Terminal,
            Selection::Session(_) => SessionMode::Chat,
            Selection::Draft(k) => self.draft_modes.get(k).copied().unwrap_or_default(),
        }
    }

    pub fn set_mode(&mut self, mode: SessionMode, cx: &mut Context<Self>) {
        if let Selection::Draft(key) = self.selection
            && !self.starting.contains(&key)
        {
            self.draft_modes.insert(key, mode);
            cx.notify();
        }
    }

    pub fn terminal_draft(&self) -> Option<(u64, SessionConfig)> {
        let Selection::Draft(key) = self.selection else {
            return None;
        };
        (self.mode() == SessionMode::Terminal)
            .then(|| self.drafts.get(&key).cloned().map(|config| (key, config)))
            .flatten()
    }

    pub fn begin_terminal_prompt(&mut self, key: u64, text: String, cx: &mut Context<Self>) {
        self.error = None;
        self.starting.insert(key);
        self.pending_prompts.insert(
            key,
            PendingPrompt {
                input: UserInput::text(text.clone()),
                text,
                error: None,
            },
        );
        cx.notify();
    }

    pub fn terminal_start_failed(&mut self, key: u64, error: String, cx: &mut Context<Self>) {
        self.error = Some(format!("Could not start terminal: {error}"));
        self.starting.remove(&key);
        if let Some(pending) = self.pending_prompts.get_mut(&key) {
            pending.error = Some(error);
        }
        cx.notify();
    }

    pub fn terminal_archived(&self, id: &SessionId) -> bool {
        self.overrides.archived(id)
    }
    pub fn unread_motion(&self, id: &SessionId) -> (f32, f32, Option<std::time::Instant>) {
        let target = if self.terminal_unread(id) { 1.0 } else { 0.0 };
        self.unread_motion
            .get(id)
            .map_or((target, target, None), |&(from, start)| {
                (from, target, Some(start))
            })
    }

    fn set_unread(&mut self, id: &SessionId, unread: bool) {
        if self.overrides.unread(id) == unread {
            return;
        }
        let (from, target, start) = self.unread_motion(id);
        let progress = start.map_or(1.0, |start| (start.elapsed().as_secs_f32() / 0.18).min(1.0));
        let current = from + (target - from) * (1.0 - (1.0 - progress).powi(3));
        self.unread_motion
            .insert(id.clone(), (current, std::time::Instant::now()));
        self.overrides.set_unread(id, unread);
    }

    pub fn terminal_unread(&self, id: &SessionId) -> bool {
        self.overrides.unread(id)
    }

    pub fn select_terminal(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.set_unread(&id, false);
        self.save_overrides();
        let tab = Tab::Terminal(id.clone());
        if !self.tabs.contains(&tab) {
            self.new_uid(tab.clone());
            self.tabs.push(tab);
        }
        self.selection = Selection::Terminal(id);
        self.panel = Panel::Chat;
        cx.notify();
    }

    pub fn toggle_terminal_pin(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        let Some(row) = self.terminal_rows.iter_mut().find(|row| &row.id == id) else {
            return;
        };
        row.pinned = !row.pinned;
        self.terminal_pin_changes.push((id.clone(), row.pinned));
        self.terminal_rows.sort_by_key(|row| !row.pinned);
        cx.notify();
    }

    pub fn toggle_session_pin(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        if self.terminal_rows.iter().any(|row| &row.id == id) {
            self.toggle_terminal_pin(id, cx);
            return;
        }
        let Some(row) = self.rows().into_iter().find(|row| &row.id == id) else {
            return;
        };
        self.overrides.set_pinned(id, !row.pinned);
        self.save_overrides();
        cx.notify();
    }

    pub fn reply_finished(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        let visible = self.panel == Panel::Chat
            && matches!(&self.selection,
            Selection::Session(current) | Selection::Terminal(current) if current == id);
        if !visible {
            self.mark_session_unread(id, cx);
        }
    }

    pub fn mark_session_unread(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        self.set_unread(id, true);
        self.save_overrides();
        cx.notify();
    }

    pub fn remove_terminal(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        if !self.terminal_rows.iter().any(|row| &row.id == id) {
            return;
        }
        self.close_tab(&Tab::Terminal(id.clone()), cx);
        self.terminal_rows.retain(|row| &row.id != id);
        self.removed_terminals.push(id.clone());
        cx.notify();
    }

    pub fn terminal_started(
        &mut self,
        draft: Option<u64>,
        row: TerminalRow,
        cx: &mut Context<Self>,
    ) {
        let id = row.id.clone();
        if let Some(old) = self.terminal_rows.iter_mut().find(|r| r.id == id) {
            *old = row;
        } else {
            self.terminal_rows.push(row);
        }
        if let Some(key) = draft {
            if let Some(at) = self.tabs.iter().position(|t| *t == Tab::Draft(key)) {
                let tab = Tab::Terminal(id.clone());
                self.tabs[at] = tab.clone();
                if let Some(uid) = self.uids.remove(&Tab::Draft(key)) {
                    self.uids.insert(tab, uid);
                }
            }
            self.drafts.remove(&key);
            self.draft_modes.remove(&key);
            self.starting.remove(&key);
            self.pending_prompts.remove(&key);
        }
        self.select_terminal(id, cx);
    }

    /// Restoring a background terminal must not change the selected tab or its pin.
    pub fn terminal_reopened(&mut self, mut row: TerminalRow, cx: &mut Context<Self>) {
        if let Some(old) = self.terminal_rows.iter_mut().find(|r| r.id == row.id) {
            row.pinned = old.pinned;
            *old = row;
        }
        cx.notify();
    }

    pub fn pending_prompt(&self) -> Option<&str> {
        let Selection::Draft(key) = self.selection else {
            return None;
        };
        self.pending_prompts.get(&key).map(|p| p.text.as_str())
    }

    pub fn retry_pending(&mut self, cx: &mut Context<Self>) {
        let Selection::Draft(key) = self.selection else {
            return;
        };
        if let Some(input) = self.pending_prompts.get(&key).map(|p| p.input.clone()) {
            self.send_input(input, cx);
        }
    }

    pub fn pending_error(&self) -> Option<&str> {
        let Selection::Draft(key) = self.selection else {
            return None;
        };
        self.pending_prompts
            .get(&key)
            .and_then(|p| p.error.as_deref())
    }

    pub fn model_catalog(&self) -> ModelCatalog {
        let (provider, _, cwd) = self.effective();
        self.catalogs
            .get(&(provider, cwd))
            .cloned()
            .unwrap_or_default()
    }

    pub fn load_models(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let (provider, _, cwd) = self.effective();
        let catalog = self.catalogs.entry((provider, cwd.clone())).or_default();
        if catalog.loading || (catalog.loaded && !refresh) {
            return;
        }
        catalog.loading = true;
        catalog.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        let model = self.effective().1;
        let updates_enabled = self.updates_enabled;
        self.rt.spawn(async move {
            let result = async {
                if updates_enabled {
                    crate::provider_updates::ensure_provider(provider, model, |_| {}).await?;
                }
                manager
                    .list_models(provider, &cwd)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            let _ = tx.send(Msg::Models(provider, cwd, result));
        });
        cx.notify();
    }

    pub fn model_info(&self) -> Option<ModelInfo> {
        let (_, selected, _) = self.effective();
        self.model_catalog()
            .models
            .into_iter()
            .find(|m| selected.as_ref().map_or(m.is_default, |id| &m.id == id))
    }

    pub fn effort(&self) -> Option<String> {
        match &self.selection {
            Selection::Draft(_) => self.draft().and_then(|d| d.reasoning_effort.clone()),
            Selection::Session(id) => self
                .records
                .iter()
                .find(|r| &r.id == id)
                .and_then(|r| r.reasoning_effort.clone()),
            Selection::Terminal(_) => None,
        }
    }

    pub fn set_effort(&mut self, effort: Option<String>, cx: &mut Context<Self>) {
        if self.is_starting() || self.effort() == effort {
            return;
        }
        if let Some(value) = &effort
            && !self.model_info().is_some_and(|m| m.supports_effort(value))
        {
            return;
        }
        let tier = self.service_tier();
        if matches!(self.selection, Selection::Session(_)) {
            let info = self.model_info();
            let model = self
                .effective()
                .1
                .or_else(|| info.as_ref().map(|m| m.id.clone()));
            let effort = effort.or_else(|| info.and_then(|m| m.default_effort));
            self.update_session_model_settings(model, effort, tier, cx);
            return;
        }
        // Pin the advertised default when setting effort: it must apply to
        // the same model whose capabilities the picker displayed.
        let model_id = self.model_info().map(|m| m.id);
        if let Some(draft) = self.draft_mut() {
            if effort.is_some() && draft.model.is_none() {
                draft.model = model_id;
            }
            draft.reasoning_effort = effort;
            draft.service_tier = tier;
        }
        cx.notify();
    }

    pub fn service_tier(&self) -> Option<String> {
        match &self.selection {
            Selection::Draft(_) => self.draft().and_then(|d| d.service_tier.clone()),
            Selection::Session(id) => self
                .records
                .iter()
                .find(|r| &r.id == id)
                .and_then(|r| r.service_tier.clone()),
            Selection::Terminal(_) => None,
        }
    }

    pub fn set_fast_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.is_starting() {
            return;
        }
        let tier = if enabled {
            let Some(tier) = self.model_info().and_then(|m| m.fast_tier) else {
                return;
            };
            Some(tier)
        } else {
            None
        };
        if self.service_tier() == tier {
            return;
        }
        let effort = self.effort();
        if matches!(self.selection, Selection::Session(_)) {
            self.update_session_model_settings(self.effective().1, effort, tier, cx);
            return;
        }
        if let Some(draft) = self.draft_mut() {
            draft.service_tier = tier;
            draft.reasoning_effort = effort;
        }
        cx.notify();
    }

    pub fn reset_model_settings(&mut self, cx: &mut Context<Self>) {
        if self.is_starting() {
            return;
        }
        self.set_fast_mode(false, cx);
        self.set_effort(None, cx);
        self.load_models(true, cx);
    }

    /// Whether the open draft is being turned into a session.
    pub fn is_starting(&self) -> bool {
        matches!(self.selection, Selection::Draft(k) if self.starting.contains(&k))
    }

    /// Focus an unused draft if available; otherwise inherit the current settings.
    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        let reusable = |tab: &&Tab| {
            matches!(tab, Tab::Draft(key)
            if self.drafts.contains_key(key)
                && !self.starting.contains(key)
                && !self.pending_prompts.contains_key(key))
        };
        let existing = self
            .tabs
            .iter()
            .filter(reusable)
            .find(|tab| tab.selection() == self.selection)
            .or_else(|| self.tabs.iter().find(reusable))
            .cloned();
        if let Some(tab) = existing {
            self.select_tab(&tab, cx);
            return;
        }
        let (provider, model, cwd) = self.effective();
        let effort = self.effort();
        let tier = self.service_tier();
        self.open_draft(provider, model, cwd, cx);
        if let Some(draft) = self.draft_mut() {
            draft.reasoning_effort = effort;
            draft.service_tier = tier;
        }
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
                reasoning_effort: None,
                service_tier: None,
                provider,
                permission_mode: PermissionMode::default(),
            },
        );
        self.new_uid(Tab::Draft(key));
        self.tabs.push(Tab::Draft(key));
        self.selection = Selection::Draft(key);
        self.panel = Panel::Chat;
        self.renaming = None;
        cx.notify();
    }

    /// Changing providers opens a draft in the same folder. Model and effort
    /// changes stay in the current provider session.
    pub fn fork_with(&mut self, provider: Provider, model: Option<String>, cx: &mut Context<Self>) {
        let (_, _, cwd) = self.effective();
        self.open_draft(provider, model, cwd, cx);
    }

    fn new_uid(&mut self, tab: Tab) {
        self.uids.insert(tab, self.next_uid);
        self.next_uid += 1;
    }

    pub fn uid_of(&self, tab: &Tab) -> u64 {
        self.uids.get(tab).copied().unwrap_or(0)
    }

    fn add_tab(&mut self, id: &SessionId) {
        let tab = Tab::Session(id.clone());
        if !self.tabs.contains(&tab) {
            self.new_uid(tab.clone());
            self.tabs.push(tab);
        }
    }

    pub fn select_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        match tab {
            Tab::Session(id) => self.select(id.clone(), cx),
            Tab::Terminal(id) => self.select_terminal(id.clone(), cx),
            Tab::Draft(k) => {
                self.selection = Selection::Draft(*k);
                self.panel = Panel::Chat;
                self.renaming = None;
                cx.notify();
            }
        }
    }

    /// Move the dragged tab to the target's position without changing selection.
    pub fn reorder_tab(&mut self, dragged: u64, target: u64, cx: &mut Context<Self>) {
        let from = self.tabs.iter().position(|tab| self.uid_of(tab) == dragged);
        let to = self.tabs.iter().position(|tab| self.uid_of(tab) == target);
        if let (Some(from), Some(to)) = (from, to)
            && from != to
        {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
            cx.notify();
        }
    }

    /// Close a tab. A session stays in the sidebar list and keeps running.
    pub fn close_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t == tab) else {
            return;
        };
        let (title, provider) = self.tab_info(tab);
        let uid = self.uid_of(tab);
        self.uids.remove(tab);
        self.ghosts.push(Ghost {
            uid,
            at,
            title,
            provider,
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TAB_ANIM * 12 / 10).await;
            let _ = this.update(cx, |s, cx| {
                s.ghosts.retain(|g| g.uid != uid);
                cx.notify();
            });
        })
        .detach();
        self.tabs.remove(at);
        if let Tab::Terminal(id) = tab {
            self.closed_terminals.push(id.clone());
            if let Some(row) = self.terminal_rows.iter_mut().find(|r| &r.id == id) {
                row.active = false;
            }
        }
        if let Tab::Draft(k) = tab {
            self.drafts.remove(k);
            self.draft_modes.remove(k);
            self.starting.remove(k);
            self.pending_prompts.remove(k);
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
            Selection::Terminal(id) => Tab::Terminal(id.clone()),
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
        self.set_unread(&id, false);
        self.save_overrides();
        self.add_tab(&id);
        self.selection = Selection::Session(id.clone());
        self.panel = Panel::Chat;
        self.renaming = None;
        // Reattach in the background; a live session returns immediately.
        let (backend, tx) = (self.backend(), self.tx.clone());
        self.rt.spawn(async move {
            if let Err(e) = backend.resume(&id).await {
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
    #[cfg(test)]
    pub fn send(&mut self, text: String, cx: &mut Context<Self>) {
        self.send_input(UserInput::text(text), cx);
    }

    pub fn send_input(&mut self, input: UserInput, cx: &mut Context<Self>) {
        let text = input.text.clone();
        self.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        let updates_enabled = self.updates_enabled;
        match self.selection.clone() {
            Selection::Terminal(_) => return,
            Selection::Draft(key) => {
                let Some(config) = self.drafts.get(&key).cloned() else {
                    return;
                };
                if !self.starting.insert(key) {
                    return;
                }
                self.pending_prompts.insert(
                    key,
                    PendingPrompt {
                        text: text.clone(),
                        input: input.clone(),
                        error: None,
                    },
                );
                let report = self.update_reporter(format!("draft-{key}"));
                self.rt.spawn(async move {
                    let update = if updates_enabled {
                        crate::provider_updates::ensure_provider(
                            config.provider,
                            config.model.clone(),
                            |message| report(Some(message)),
                        )
                        .await
                    } else {
                        Ok(())
                    };
                    report(None);
                    if let Err(error) = update {
                        let _ = tx.send(Msg::StartFailed(key, error));
                        return;
                    }
                    let result = async {
                        let handle = manager.start_session(config).await?;
                        manager.send_prompt(&handle.id, input.clone()).await?;
                        Ok::<_, AgentError>(handle.id)
                    }
                    .await;
                    let _ = tx.send(match result {
                        Ok(id) => Msg::Started(key, id, text),
                        Err(e) => Msg::StartFailed(key, e.to_string()),
                    });
                });
            }
            Selection::Session(id) => {
                let backend = self.backend();
                self.rt.spawn(async move {
                    let result = async {
                        backend.resume(&id).await?;
                        manager.send_prompt(&id, input).await
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
        if self.terminal_rows.iter().any(|row| row.id == id) {
            self.overrides.set_archived(&id, true);
            self.save_overrides();
            self.close_tab(&Tab::Terminal(id), cx);
            cx.notify();
            return;
        }
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
        if self.terminal_rows.iter().any(|row| row.id == id) {
            self.select_terminal(id, cx);
        } else {
            self.select(id, cx);
        }
    }

    pub fn permanently_delete(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.terminal_rows.iter().any(|row| row.id == id) {
            self.overrides.remove(&id);
            self.save_overrides();
            self.remove_terminal(&id, cx);
            return;
        }
        self.close_tab(&Tab::Session(id.clone()), cx);
        self.records.retain(|record| record.id != id);
        self.overrides.remove(&id);
        self.save_overrides();
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        self.rt.spawn(async move {
            if let Err(error) = manager.delete(&id).await {
                let _ = tx.send(Msg::Error(format!("Could not delete session: {error}")));
            }
            let _ = tx.send(Msg::Refresh);
        });
        cx.notify();
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
    pub fn tab_info(&self, tab: &Tab) -> (String, Option<Provider>) {
        match tab {
            Tab::Session(id) => (self.current_title(id), self.provider_of(id)),
            Tab::Terminal(id) => self
                .terminal_rows
                .iter()
                .find(|r| &r.id == id)
                .map(|r| (self.current_title(&r.id), r.provider))
                .unwrap_or_else(|| ("Terminal".into(), None)),
            Tab::Draft(k) => (
                sessions::DEFAULT_TITLE.to_string(),
                self.drafts.get(k).map(|d| d.provider),
            ),
        }
    }

    pub fn provider_of(&self, id: &SessionId) -> Option<Provider> {
        self.records
            .iter()
            .find(|r| &r.id == id)
            .map(|r| r.provider)
    }

    pub fn title_loading(&self, id: &SessionId) -> bool {
        self.terminal_title_loading.contains(id) && self.overrides.title(id).is_none()
    }

    pub fn current_title(&self, id: &SessionId) -> String {
        if let Some(row) = self.terminal_rows.iter().find(|row| &row.id == id) {
            return self.overrides.title(id).unwrap_or(&row.title).to_owned();
        }
        self.overrides
            .rows(&self.records)
            .into_iter()
            .find(|r| &r.id == id)
            .map(|r| r.title)
            .unwrap_or_default()
    }

    fn save_overrides(&mut self) {
        if cfg!(test) {
            return;
        }
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
                d.reasoning_effort = None;
                d.service_tier = None;
                cx.notify();
            }
        } else if self.effective().0 != provider {
            self.fork_with(provider, None, cx);
        }
    }

    pub fn set_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if let Some(d) = self.draft_mut() {
            if d.model != model {
                d.model = model;
                d.reasoning_effort = None;
                d.service_tier = None;
            }
            cx.notify();
        } else {
            let (_, current, _) = self.effective();
            let catalog = self.model_catalog();
            let info = catalog
                .models
                .iter()
                .find(|m| model.as_ref().map_or(m.is_default, |id| id == &m.id));
            let model = model.or_else(|| info.map(|m| m.id.clone()));
            if current != model {
                let effort = info.and_then(|m| m.default_effort.clone());
                self.update_session_model_settings(model, effort, None, cx);
            }
        }
    }

    fn update_session_model_settings(
        &mut self,
        model: Option<String>,
        effort: Option<String>,
        tier: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Selection::Session(id) = &self.selection else {
            return;
        };
        match self.manager.set_model_settings(id, model, effort, tier) {
            Ok(()) => self.records = self.manager.list_sessions(),
            Err(error) => self.error = Some(format!("Could not save model settings: {error}")),
        }
        cx.notify();
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

    pub fn terminals_fraction(&self) -> f32 {
        let target = if self.terminals_collapsed { 0.0 } else { 1.0 };
        self.terminals_motion.map_or(target, |(start, from)| {
            let t = (start.elapsed().as_secs_f32() / 0.24).min(1.0);
            from + (target - from) * (1.0 - (1.0 - t).powi(4))
        })
    }

    pub fn toggle_terminals(&mut self, cx: &mut Context<Self>) {
        let from = self.terminals_fraction();
        self.terminals_collapsed = !self.terminals_collapsed;
        self.terminals_motion = Some((std::time::Instant::now(), from));
        self.terminals_toggle += 1;
        cx.notify();
    }

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

    pub fn toggle_caffeine(&mut self, cx: &mut Context<Self>) {
        self.caffeine_enabled = !self.caffeine_enabled;
        cx.notify();
    }

    pub fn recheck(&mut self, cx: &mut Context<Self>) {
        for provider in Provider::ALL {
            self.recheck_provider(provider, cx);
        }
    }

    pub fn recheck_provider(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if !self.checking.insert(provider) {
            return;
        }
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Msg::Checked(providers::detect(provider).await));
        });
        cx.notify();
    }

    /// Run the CLI's own login in the user's terminal, then poll its status
    /// command until it reports signed in (or five minutes pass).
    pub fn sign_in(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.signin.is_some() {
            return;
        }
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
                let all = providers::detect_all().await;
                let done = all
                    .iter()
                    .any(|s| s.provider == provider && s.login == LoginState::LoggedIn);
                let _ = tx.send(Msg::Statuses(all));
                if done {
                    return;
                }
            }
            let _ = tx.send(Msg::SigninFinished(provider));
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

#[cfg(test)]
mod tests {
    use super::{
        AppState, Arc, HashMap, Overrides, Panel, PermissionMode, Provider, Selection,
        SessionConfig, SessionManager, Tab, unbounded_channel,
    };
    use gpui::AppContext as _;
    use gpui::Entity;

    #[gpui::test]
    fn closing_settings_preserves_selection_and_terminal_toggle_keeps_sessions(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let selection = s.selection.clone();
            s.panel = Panel::Providers;
            s.toggle_providers(cx);
            assert_eq!(s.panel, Panel::Chat);
            assert_eq!(s.selection, selection);
            s.toggle_terminals(cx);
            assert!(s.terminals_collapsed);
            s.terminals_motion = Some((
                std::time::Instant::now() - std::time::Duration::from_secs(1),
                1.0,
            ));
            assert_eq!(s.terminals_fraction(), 0.0);
            s.toggle_terminals(cx);
            assert!(!s.terminals_collapsed);
            assert!(s.terminals_fraction() < 0.1);
            assert_eq!(s.selection, selection);
        });
    }

    // A parked runtime makes startup deterministically slow: no provider work
    // can finish before we check the UI state after send().
    fn draft(cx: &mut gpui::TestAppContext) -> Entity<AppState> {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(dir.path().join("sessions.jsonl"), []).unwrap();
        let (tx, _rx) = unbounded_channel();
        cx.new(|_| AppState {
            catalogs: HashMap::new(),
            rt: Arc::new(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
            ),
            manager,
            tx,
            overrides: Overrides::default(),
            records: vec![],
            selection: Selection::Draft(0),
            drafts: HashMap::from([(
                0,
                SessionConfig {
                    cwd: dir.path().into(),
                    model: None,
                    reasoning_effort: None,
                    service_tier: None,
                    provider: Provider::Claude,
                    permission_mode: PermissionMode::default(),
                },
            )]),
            next_draft: 1,
            draft_modes: HashMap::new(),
            terminal_rows: vec![],
            terminal_title_loading: Default::default(),
            unread_motion: HashMap::new(),
            update_progress: HashMap::new(),
            updates_enabled: false,
            closed_terminals: vec![],
            removed_terminals: vec![],
            terminal_pin_changes: vec![],
            panel: Panel::Chat,
            preferences_open: true,
            shortcut_hints: false,
            sidebar_hover: Default::default(),
            providers: vec![],
            signin: None,
            checking: Default::default(),
            caffeine_enabled: false,
            renaming: None,
            show_archived: false,
            terminals_collapsed: false,
            terminals_motion: None,
            terminals_toggle: 0,
            error: None,
            first_prompts: HashMap::new(),
            pending_prompts: HashMap::new(),
            tabs: vec![Tab::Draft(0)],
            uids: HashMap::from([(Tab::Draft(0), 0)]),
            next_uid: 1,
            ghosts: vec![],
            starting: Default::default(),
        })
    }

    #[gpui::test]
    fn completed_background_reply_is_unread_until_opened(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let id = del_agent::SessionId::new("reply");
            s.apply(super::Msg::ReplyFinished(id.clone()), cx);
            assert!(s.terminal_unread(&id));
            s.select(id.clone(), cx);
            assert!(!s.terminal_unread(&id));
            s.apply(super::Msg::ReplyFinished(id.clone()), cx);
            assert!(!s.terminal_unread(&id));
            s.panel = Panel::Providers;
            s.apply(super::Msg::ReplyFinished(id.clone()), cx);
            assert!(s.terminal_unread(&id));
            s.select(id.clone(), cx);
            s.mark_session_unread(&id, cx);
            assert!(s.terminal_unread(&id));
        });
    }

    #[gpui::test]
    fn reopening_terminal_does_not_steal_selection_or_unpin_it(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let mut row = crate::sessions::TerminalRow {
                id: del_agent::SessionId::new("terminal-a"),
                title: "Task".into(),
                cwd: "/tmp".into(),
                provider: Some(Provider::Codex),
                active: false,
                summary: String::new(),
                pinned: true,
            };
            s.terminal_rows.push(row.clone());
            row.pinned = false;
            row.active = true;
            s.terminal_reopened(row, cx);
            assert_eq!(s.selection, Selection::Draft(0));
            assert!(s.terminal_rows[0].pinned);
            assert!(s.terminal_rows[0].active);
        });
    }

    #[gpui::test]
    fn terminal_choice_keeps_draft_until_prompt_launch_succeeds(cx: &mut gpui::TestAppContext) {
        use crate::sessions::TerminalRow;
        use del_agent::SessionId;
        use del_mcp::SessionMode;
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let tab_info = s.tab_info(&Tab::Draft(0));
            s.set_mode(SessionMode::Terminal, cx);
            assert_eq!(s.selection, Selection::Draft(0));
            assert_eq!(s.tab_info(&Tab::Draft(0)), tab_info);
            assert!(s.pending_prompt().is_none());
            assert!(!s.is_starting());
            let (key, config) = s.terminal_draft().unwrap();
            s.begin_terminal_prompt(key, "build this".into(), cx);
            assert!(s.is_starting());
            assert_eq!(s.pending_prompt(), Some("build this"));
            s.set_mode(SessionMode::Chat, cx);
            assert_eq!(s.mode(), SessionMode::Terminal);
            s.terminal_start_failed(key, "spawn failed".into(), cx);
            assert!(!s.is_starting());
            assert_eq!(s.selection, Selection::Draft(key));
            assert_eq!(s.pending_error(), Some("spawn failed"));
            s.begin_terminal_prompt(key, "retry".into(), cx);
            let id = SessionId::new("terminal-test");
            s.terminal_started(
                Some(key),
                TerminalRow {
                    id: id.clone(),
                    title: "Codex".into(),
                    cwd: config.cwd,
                    provider: Some(config.provider),
                    active: true,
                    summary: String::new(),
                    pinned: false,
                },
                cx,
            );
            assert_eq!(s.selection, Selection::Terminal(id.clone()));
            assert_eq!(s.tabs, vec![Tab::Terminal(id)]);
            assert!(!s.starting.contains(&key));
            assert!(!s.pending_prompts.contains_key(&key));
        });
    }

    #[gpui::test]
    fn first_prompt_is_visible_before_backend_startup(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            s.send("hello immediately".into(), cx);
            assert!(s.is_starting());
            assert_eq!(s.pending_prompt(), Some("hello immediately"));
            s.send("duplicate".into(), cx);
            assert_eq!(s.pending_prompt(), Some("hello immediately"));
        });
    }

    #[gpui::test]
    fn startup_failure_keeps_text_and_only_unlocks_its_own_tab(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            s.send("first".into(), cx);
            s.new_session(cx);
            s.send("second".into(), cx);
            s.apply(super::Msg::StartFailed(0, "offline".into()), cx);
            assert!(s.is_starting());
            assert_eq!(s.pending_prompt(), Some("second"));
            assert_eq!(s.pending_error(), None);
            s.select_tab(&Tab::Draft(0), cx);
            assert!(!s.is_starting());
            assert_eq!(s.pending_prompt(), Some("first"));
            assert_eq!(s.pending_error(), Some("offline"));
            s.send("retry".into(), cx);
            assert!(s.is_starting());
            assert_eq!(s.pending_prompt(), Some("retry"));
            assert_eq!(s.pending_error(), None);
        });
    }

    #[gpui::test]
    fn startup_hands_opening_message_to_chat_once(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            s.send("first".into(), cx);
            let id = del_agent::SessionId::new("ready");
            s.apply(super::Msg::Started(0, id.clone(), "first".into()), cx);
            assert_eq!(s.selection, Selection::Session(id.clone()));
            assert!(!s.is_starting());
            assert!(s.pending_prompts.is_empty());
            assert_eq!(s.take_first_prompt(&id).as_deref(), Some("first"));
            assert_eq!(s.take_first_prompt(&id), None);
        });
    }
    #[gpui::test]
    fn model_choices_validate_and_reset_effort(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let (provider, _, cwd) = s.effective();
            s.apply(
                super::Msg::Models(
                    provider,
                    cwd,
                    Ok(vec![del_agent::ModelInfo {
                        id: "model-a".into(),
                        name: "Model A".into(),
                        description: String::new(),
                        efforts: vec![del_agent::EffortOption::named("high")],
                        default_effort: None,
                        is_default: true,
                        fast_tier: Some("priority".into()),
                    }]),
                ),
                cx,
            );
            s.set_effort(Some("invalid".into()), cx);
            assert_eq!(s.effort(), None);
            s.set_fast_mode(true, cx);
            assert_eq!(s.service_tier().as_deref(), Some("priority"));
            s.set_effort(Some("high".into()), cx);
            assert_eq!(s.service_tier().as_deref(), Some("priority"));
            assert_eq!(s.effective().1.as_deref(), Some("model-a"));
            s.set_model(Some("model-a".into()), cx);
            assert_eq!(s.effort().as_deref(), Some("high"));
            s.set_model(Some("model-b".into()), cx);
            assert_eq!(s.effort(), None);
            assert_eq!(s.service_tier(), None);
            s.set_fast_mode(true, cx);
            assert_eq!(s.service_tier(), None);
            s.set_model(Some("model-a".into()), cx);
            s.set_fast_mode(true, cx);
            s.set_fast_mode(false, cx);
            assert_eq!(s.service_tier(), None);
            s.set_fast_mode(true, cx);
            s.set_effort(Some("high".into()), cx);
            s.reset_model_settings(cx);
            assert_eq!(s.service_tier(), None);
            assert_eq!(s.effort(), None);
            assert!(s.model_catalog().loading);
            s.set_effort(Some("high".into()), cx);
            s.set_provider(
                if provider == del_agent::Provider::Claude {
                    del_agent::Provider::Codex
                } else {
                    del_agent::Provider::Claude
                },
                cx,
            );
            assert_eq!(s.effort(), None);
        });
    }
    #[gpui::test]
    fn existing_chat_model_and_effort_changes_keep_the_same_session(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        let dir = tempfile::tempdir().unwrap();
        state.update(cx, |s, cx| {
            let config = s.draft().unwrap().clone();
            s.manager = SessionManager::new(
                dir.path().join("sessions.jsonl"),
                [Arc::new(del_agent::MockBackend::new(config.provider))
                    as Arc<dyn del_agent::AgentBackend>],
            )
            .unwrap();
            let handle =
                s.rt.block_on(s.manager.start_session(config.clone()))
                    .unwrap();
            s.records = s.manager.list_sessions();
            s.selection = Selection::Session(handle.id.clone());
            s.tabs = vec![Tab::Session(handle.id.clone())];
            s.apply(
                super::Msg::Models(
                    config.provider,
                    config.cwd,
                    Ok(vec![del_agent::ModelInfo {
                        id: "model-a".into(),
                        name: "Model A".into(),
                        description: String::new(),
                        efforts: vec![del_agent::EffortOption::named("high")],
                        default_effort: Some("high".into()),
                        is_default: true,
                        fast_tier: Some("priority".into()),
                    }]),
                ),
                cx,
            );
            let selection = s.selection.clone();
            let tabs = s.tabs.clone();
            s.set_effort(Some("high".into()), cx);
            assert_eq!(s.selection, selection, "effort must not open a new chat");
            s.set_model(Some("model-a".into()), cx);
            s.set_fast_mode(true, cx);
            assert_eq!(s.selection, selection);
            assert_eq!(s.tabs, tabs);
            let saved = &s.manager.list_sessions()[0];
            assert_eq!(saved.model.as_deref(), Some("model-a"));
            assert_eq!(saved.reasoning_effort.as_deref(), Some("high"));
            assert_eq!(saved.service_tier.as_deref(), Some("priority"));
        });
    }

    #[gpui::test]
    fn reordering_preserves_selection_and_handles_both_directions(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let (provider, model, cwd) = s.effective();
            s.open_draft(provider, model.clone(), cwd.clone(), cx);
            s.open_draft(provider, model, cwd, cx);
            let original = s.tabs.clone();
            let selection = s.selection.clone();
            let first = s.uid_of(&original[0]);
            let last = s.uid_of(&original[2]);
            s.reorder_tab(first, last, cx);
            assert_eq!(
                s.tabs,
                vec![
                    original[1].clone(),
                    original[2].clone(),
                    original[0].clone()
                ]
            );
            assert_eq!(s.selection, selection);
            s.reorder_tab(first, s.uid_of(&original[1]), cx);
            assert_eq!(s.tabs, original);
            s.reorder_tab(first, first, cx);
            s.reorder_tab(u64::MAX, last, cx);
            assert_eq!(s.tabs, original);
        });
    }
    #[gpui::test]
    fn repeated_new_session_reuses_the_existing_draft(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            for _ in 0..10 {
                s.new_session(cx);
            }
            assert_eq!(s.tabs, vec![Tab::Draft(0)]);
            s.selection = Selection::Session(del_agent::SessionId::new("another"));
            s.new_session(cx);
            assert_eq!(s.selection, Selection::Draft(0));
            assert_eq!(s.tabs.len(), 1);
            s.send("starting".into(), cx);
            s.new_session(cx);
            assert_eq!(s.tabs.len(), 2);
            let selected = s.selection.clone();
            s.new_session(cx);
            assert_eq!(s.selection, selected);
            assert_eq!(s.tabs.len(), 2);
            assert_eq!(s.pending_prompts.get(&0).unwrap().text, "starting");
        });
    }
    #[gpui::test]
    fn provider_checks_finish_independently(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            s.recheck_provider(del_agent::Provider::Codex, cx);
            s.recheck_provider(del_agent::Provider::Codex, cx);
            s.recheck_provider(del_agent::Provider::Claude, cx);
            assert_eq!(s.checking.len(), 2);
            s.apply(
                super::Msg::Checked(del_agent::ProviderStatus {
                    provider: del_agent::Provider::Codex,
                    installed: true,
                    path: None,
                    version: None,
                    login: del_agent::LoginState::LoggedIn,
                }),
                cx,
            );
            assert!(!s.checking.contains(&del_agent::Provider::Codex));
            assert!(s.checking.contains(&del_agent::Provider::Claude));
            assert!(super::providers::ready(
                s.status_of(del_agent::Provider::Codex).unwrap()
            ));
        });
    }

    #[gpui::test]
    fn terminal_pin_and_close_update_sidebar_state(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let first = del_agent::SessionId::new("first-terminal");
            let second = del_agent::SessionId::new("second-terminal");
            for id in [&first, &second] {
                s.terminal_rows.push(crate::sessions::TerminalRow {
                    id: id.clone(),
                    title: "Shell".into(),
                    cwd: "/tmp".into(),
                    provider: None,
                    active: false,
                    summary: String::new(),
                    pinned: false,
                });
            }
            s.select_terminal(first.clone(), cx);
            s.toggle_terminal_pin(&second, cx);
            assert_eq!(s.terminal_rows[0].id, second);
            assert_eq!(s.terminal_pin_changes, vec![(second.clone(), true)]);
            s.remove_terminal(&first, cx);
            assert!(!s.terminal_rows.iter().any(|row| row.id == first));
            assert_eq!(s.removed_terminals, vec![first]);
        });
    }

    #[gpui::test]
    fn terminal_context_actions_share_chat_metadata_and_keep_manual_names(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let id = del_agent::SessionId::new("terminal-menu-test");
            s.terminal_rows.push(crate::sessions::TerminalRow {
                id: id.clone(),
                title: "Provider title".into(),
                cwd: "/tmp".into(),
                provider: Some(Provider::Codex),
                active: false,
                summary: String::new(),
                pinned: false,
            });
            s.start_rename(id.clone(), cx);
            s.finish_rename(Some("My title".into()), cx);
            s.terminal_rows[0].title = "New provider title".into();
            assert_eq!(s.current_title(&id), "My title");
            assert_eq!(s.tab_info(&Tab::Terminal(id.clone())).0, "My title");
            s.toggle_session_pin(&id, cx);
            assert!(s.terminal_rows[0].pinned);
            s.mark_session_unread(&id, cx);
            assert!(s.terminal_unread(&id));
            s.select_terminal(id.clone(), cx);
            assert!(!s.terminal_unread(&id));
            s.archive(id.clone(), cx);
            assert!(s.terminal_archived(&id));
            assert!(s.terminal_rows.iter().any(|row| row.id == id));
            s.restore(id.clone(), cx);
            assert!(!s.terminal_archived(&id));
            assert_eq!(s.selection, Selection::Terminal(id.clone()));
            s.permanently_delete(id.clone(), cx);
            assert!(!s.terminal_rows.iter().any(|row| row.id == id));
        });
    }

    #[gpui::test]
    fn caffeine_toggle_is_explicit_and_reversible(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            assert!(!s.caffeine_enabled);
            s.toggle_caffeine(cx);
            assert!(s.caffeine_enabled);
            s.toggle_caffeine(cx);
            assert!(!s.caffeine_enabled);
        });
    }
}
