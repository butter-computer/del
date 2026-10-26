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
    ProjectInspected(PathBuf, crate::projects::GitInfo),
    TitleLoading(SessionId, bool),
    Refresh,
    PanelDeleted(String, u64),
    ReplyFinished(SessionId),
    Unloaded(SessionId),
    Statuses(Vec<ProviderStatus>),
    Checked(ProviderStatus),
    SigninFinished(Provider),
    /// A draft tab became a session: (draft key, session, first prompt).
    Started(u64, SessionId, String),
    StartFailed(u64, String),
    AccountUsage(Provider, Option<String>, Option<f64>),
    AccountUsageFinished,
    AccountSwitched(SessionId, std::result::Result<(), String>),
    AccountIdentity(Provider, Option<String>, String),
    AccountChecked(String, std::result::Result<Option<String>, String>),
    Models(
        Provider,
        Option<String>,
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
    pub project_info: HashMap<PathBuf, crate::projects::GitInfo>,
    project_loading: std::collections::HashSet<PathBuf>,
    pub project_preferences: crate::projects::ProjectPreferences,
    pub launch_options: HashMap<u64, crate::projects::LaunchOptions>,
    catalogs: HashMap<(Provider, Option<String>, PathBuf), ModelCatalog>,
    pub accounts: Vec<crate::accounts::Account>,
    pub account_status: HashMap<String, String>,
    pub default_identities: HashMap<Provider, String>,
    pub account_usage: HashMap<(Provider, Option<String>), Option<f64>>,
    usage_loading: bool,
    draft_accounts: HashMap<u64, String>,
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
    readiness: Option<crate::readiness::Readiness>,
    pub browser_tools_ready: bool,
    unread_motion: HashMap<SessionId, (f32, std::time::Instant)>,
    chat_title_loading: std::collections::HashSet<SessionId>,
    pub terminal_working: std::collections::HashSet<SessionId>,
    pub terminal_title_loading: std::collections::HashSet<SessionId>,
    pub closed_terminals: Vec<SessionId>,
    pub unloaded_chats: Vec<SessionId>,
    unloading: std::collections::HashSet<SessionId>,
    pub removed_terminals: Vec<SessionId>,
    pub deleted_panels: Vec<(String, u64)>,
    pub terminal_pin_changes: Vec<(SessionId, bool)>,
    pub panel: Panel,
    pub preferences_open: bool,
    pub shortcut_hints: bool,
    pub window_active: bool,
    pub sidebar_drag: crate::sidebar::DragMotion,
    pub sidebar_hover: crate::hover_motion::HoverMotion,
    pub providers: Vec<ProviderStatus>,
    /// Provider whose sign-in is running in the user's terminal.
    pub signin: Option<Provider>,
    pub checking: std::collections::HashSet<Provider>,
    pub caffeine_enabled: bool,
    /// Session being renamed in the sidebar.
    pub renaming: Option<SessionId>,
    pub project_renaming: Option<PathBuf>,
    pub show_archived: bool,
    /// Last problem worth telling the user about.
    pub error: Option<String>,
    /// The prompt that started each session opened this run, for its chat's
    /// first row (taken once by the chat view).
    first_prompts: HashMap<SessionId, UserInput>,
    /// Submitted text stays visible while startup runs, including on failure.
    pending_prompts: HashMap<u64, PendingPrompt>,
    /// Tabs in the top bar, in order: new-session drafts and sessions opened this run.
    pub tabs: Vec<Tab>,
    /// At most one history session occupies the replaceable preview slot.
    pub preview_tab: Option<Tab>,
    /// A stable id per tab (kept when a draft becomes its session), so each
    /// tab animates in exactly once.
    uids: HashMap<Tab, u64>,
    next_uid: u64,
    pub ghosts: Vec<Ghost>,
    /// A draft is being turned into a session; blocks a double send.
    starting: std::collections::HashSet<u64>,
    switching_accounts: std::collections::HashSet<SessionId>,
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

        let accounts = match crate::accounts::load() {
            Ok(accounts) => accounts,
            Err(e) => { error = Some(format!("Could not read accounts: {e}")); vec![] }
        };
        for account in &accounts { manager.register_account(account.id.clone(), account.backend()); }
        let initial_account = accounts.iter().find(|a| a.provider == Provider::Claude && a.is_default).map(|a| a.id.clone());

        let identity_accounts = accounts.clone();
        let identity_tx = tx.clone();
        rt.spawn(async move {
            for provider in [Provider::Codex, Provider::Claude] {
                let Ok(binary) = crate::accounts::login_binary(provider).await else { continue; };
                if let Ok(Some(identity)) = crate::accounts::provider_identity(provider, None, &binary).await {
                    let _ = identity_tx.send(Msg::AccountIdentity(provider, None, identity));
                }
                for account in identity_accounts.iter().filter(|a| a.provider == provider) {
                    let result = crate::accounts::identity(account, &binary).await;
                    let _ = identity_tx.send(Msg::AccountChecked(account.id.clone(), result));
                }
            }
        });

        // Status changes anywhere refresh the sidebar.
        let mut events = manager.subscribe_all();
        let events_tx = tx.clone();
        let title_events_manager = manager.clone();
        rt.spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match events.recv().await {
                    Ok(ManagerEvent::TitleLoading { session, loading }) => {
                        let _ = events_tx.send(Msg::TitleLoading(session, loading));
                    }
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
                    Ok(ManagerEvent::PromptSent { session }) => {
                        let manager = title_events_manager.clone();
                        let tx = events_tx.clone();
                        tokio::spawn(async move {
                            if manager.generate_title(&session).await.unwrap_or(false) {
                                let _ = tx.send(Msg::Refresh);
                            }
                        });
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

        let readiness = crate::readiness::Readiness::new(rt.handle(), manager.clone());
        cx.observe_self(|state, _| { state.refresh_readiness(); state.inspect_projects(); })
            .detach();

        Self::pump(rx, cx);

        let records = manager.list_sessions();
        let mut overrides = sessions::overrides_path()
            .map(|p| Overrides::load(&p))
            .unwrap_or_default();
        overrides.initialize_pins(&records);
        Self {
            project_info: HashMap::new(),
            project_loading: Default::default(),
            project_preferences: crate::projects::ProjectPreferences::load(),
            launch_options: HashMap::new(),
            catalogs: HashMap::new(),
            accounts,
            account_status: HashMap::new(),
            default_identities: HashMap::new(),
            account_usage: HashMap::new(),
            usage_loading: false,
            draft_accounts: initial_account.map(|id| HashMap::from([(0, id)])).unwrap_or_default(),
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
                    permission_mode: providers::permission_mode(Provider::Claude),
                },
            )]),
            panel: Panel::Chat,
            preferences_open: true,
            shortcut_hints: false,
            window_active: true,
            sidebar_drag: Default::default(),
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
            project_renaming: None,
            show_archived: false,
            error,
            first_prompts: HashMap::new(),
            pending_prompts: HashMap::new(),
            starting: Default::default(),
            switching_accounts: Default::default(),
            tabs: Vec::new(),
            preview_tab: None,
            uids: HashMap::from([(Tab::Draft(0), 0)]),
            next_uid: 1,
            ghosts: Vec::new(),
            next_draft: 1,
            draft_modes: HashMap::new(),
            terminal_rows: Vec::new(),
            terminal_working: Default::default(),
            terminal_title_loading: Default::default(),
            chat_title_loading: Default::default(),
            unread_motion: HashMap::new(),
            readiness: Some(readiness),
            browser_tools_ready: false,
            closed_terminals: Vec::new(),
            unloaded_chats: Vec::new(),
            unloading: Default::default(),
            removed_terminals: Vec::new(),
            deleted_panels: Vec::new(),
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
            Msg::ProjectInspected(path, info) => {
                self.project_loading.remove(&path);
                let mut info = info;
                if !path.exists() && let Some(root) = self.project_preferences.directories.get(&path) { info.root = root.clone(); }
                let has_active = self.unified_rows().iter().any(|row| row.cwd == path);
                let reactivated = has_active && (self.project_preferences.archived.contains(&info.root) || self.project_preferences.deleted.contains(&info.root));
                if reactivated {
                    self.project_preferences.archived.remove(&info.root);
                    self.project_preferences.deleted.remove(&info.root);
                    self.project_preferences.collapsed.remove(&info.root);
                }
                let added = self.project_preferences.roots.insert(info.root.clone());
                let previous = self.project_preferences.directories.insert(path.clone(), info.root.clone());
                if (reactivated || added || previous.as_ref() != Some(&info.root)) && !cfg!(test)
                    && let Err(error) = self.project_preferences.save() { self.error = Some(error.to_string()); }
                self.project_info.insert(path, info);
            }
            Msg::AccountSwitched(id, result) => {
                self.switching_accounts.remove(&id);
                self.records = self.manager.list_sessions();
                if let Err(error) = result { self.error = Some(format!("Could not switch account: {error}")); }
                self.load_models(true, cx);
            }
            Msg::AccountUsage(provider, id, usage) => { self.account_usage.insert((provider, id), usage); }
            Msg::AccountUsageFinished => { self.usage_loading = false; }
            Msg::AccountIdentity(provider, id, identity) => {
                if let Some(id) = id {
                    if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) { account.identity = Some(identity); }
                    if let Err(e) = crate::accounts::save(&self.accounts) { self.error = Some(format!("Could not save account: {e}")); }
                } else { self.default_identities.insert(provider, identity); }
            }
            Msg::AccountChecked(id, result) => {
                match result {
                    Ok(identity) => {
                        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
                            if let Some(identity) = identity { account.identity = Some(identity); }
                            account.requires_login = false;
                        }
                        self.catalogs.retain(|(_, account, _), _| account.as_ref() != Some(&id));
                        self.account_status.insert(id, "Connected".into());
                        self.refresh_account_usage(cx);
                        if let Err(e) = crate::accounts::save(&self.accounts) { self.error = Some(format!("Could not save account: {e}")); }
                    }
                    Err(error) => {
                        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) { account.requires_login = true; }
                        self.account_status.insert(id, error);
                        if !cfg!(test) { let _ = crate::accounts::save(&self.accounts); }
                    }
                }
            }
            Msg::Models(provider, account, cwd, result) => {
                let catalog = self.catalogs.entry((provider, account, cwd)).or_default();
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
            Msg::PanelDeleted(key, uid) => {
                self.deleted_panels.push((key, uid));
            }
            Msg::TitleLoading(id, loading) => {
                if loading {
                    self.chat_title_loading.insert(id);
                } else {
                    self.chat_title_loading.remove(&id);
                }
                self.records = self.manager.list_sessions();
            }
            Msg::Refresh => self.records = self.manager.list_sessions(),
            Msg::Unloaded(id) => {
                self.unloading.remove(&id);
                self.records = self.manager.list_sessions();
            }
            Msg::ReplyFinished(id) => {
                self.reply_finished(&id, cx);
                self.records = self.manager.list_sessions();
            }
            Msg::SigninFinished(provider) => {
                if self.signin == Some(provider) {
                    self.signin = None;
                }
                self.refresh_account_usage(cx);
            }
            Msg::Checked(status) => {
                self.checking.remove(&status.provider);
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
                    for (key, draft) in self.drafts.iter_mut() {
                        if self.draft_accounts.contains_key(key) { continue; }
                        let draft_ready = list
                            .iter()
                            .any(|s| s.provider == draft.provider && providers::ready(s));
                        if !draft_ready {
                            draft.provider = ready.provider;
                            draft.permission_mode = providers::permission_mode(ready.provider);
                            draft.model = None;
                            draft.reasoning_effort = None;
                            draft.service_tier = None;
                        }
                    }
                }
                self.providers = list;
            }
            Msg::Started(key, id, prompt) => {
                self.launch_options.remove(&key);
                let input = self
                    .pending_prompts
                    .remove(&key)
                    .map(|pending| pending.input)
                    .unwrap_or_else(|| UserInput::text(prompt));
                self.first_prompts.insert(id.clone(), input);
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
                if let Some(record) = self.records.iter().find(|r| r.id == id) { self.project_info.remove(&record.cwd); }
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

    fn refresh_readiness(&self) {
        if !self.browser_tools_ready {
            return;
        }
        let Some(readiness) = &self.readiness else {
            return;
        };
        // Keep the last chat configuration ready while looking at a terminal.
        if matches!(self.selection, Selection::Terminal(_)) || self.mode() != SessionMode::Chat {
            return;
        }
        let config = self.draft().cloned().or_else(|| {
            self.active_row().map(|r| SessionConfig {
                provider: r.provider,
                model: r.model,
                cwd: r.cwd,
                reasoning_effort: self.effort(),
                service_tier: self.service_tier(),
                permission_mode: providers::permission_mode(r.provider),
            })
        });
        // Wait for provider discovery/sign-in; never open an authentication UI
        // or prepare a chat for a terminal tab.
        let launch = self.draft_launch_options();
        let config = config.filter(|c| !launch.worktree && launch.branch.is_none() && self.selected_account().is_none() && self.status_of(c.provider).is_some_and(providers::ready));
        readiness.set_config(config);
    }

    pub fn backend(&self) -> crate::chat::Backend {
        crate::chat::Backend {
            runtime: self.rt.handle().clone(),
            manager: self.manager.clone(),
        }
    }

    /// The prompt that started `id` in this run, once.
    pub fn take_first_prompt(&mut self, id: &SessionId) -> Option<UserInput> {
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

    pub fn session_working(&self, id: &SessionId) -> bool {
        self.records.iter().any(|row| &row.id == id && row.status == SessionStatus::Running)
            || self.terminal_working.contains(id)
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
        self.open_preview(Tab::Terminal(id.clone()));
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
        self.overrides.set_pinned(id, row.pinned);
        self.sort_terminal_rows();
        self.save_overrides();
        cx.notify();
    }

    pub fn sort_terminal_rows(&mut self) {
        self.terminal_rows.sort_by_key(|row| (!row.pinned,
            self.overrides.order(&row.id).unwrap_or(if row.pinned { 0 } else { u64::MAX })));
    }

    /// Reorder only within the same visible section and pin group.
    pub fn reorder_sidebar(&mut self, dragged: &SessionId, target: &SessionId, terminal: bool, cx: &mut Context<Self>) {
        let _ = terminal; // Session mode does not define sidebar membership.
        let rows = self.unified_rows();
        let Some(source) = rows.iter().find(|r| &r.id == dragged) else { return; };
        let Some(destination) = rows.iter().find(|r| &r.id == target) else { return; };
        if source.pinned != destination.pinned || (!source.pinned && self.project_root(&source.cwd) != self.project_root(&destination.cwd)) { return; }
        let mut ids: Vec<_> = rows.iter().filter(|r| r.pinned == source.pinned && (r.pinned || self.project_root(&r.cwd) == self.project_root(&source.cwd))).map(|r| r.id.clone()).collect();
        let from = ids.iter().position(|id| id == dragged).unwrap();
        let to = ids.iter().position(|id| id == target).unwrap();
        if from == to { return; }
        let previous = ids.clone();
        let id = ids.remove(from);
        ids.insert(to, id);
        if self.sidebar_drag.active.is_some() {
            self.sidebar_drag.shifts = ids.iter().enumerate().map(|(at, id)| {
                let before = previous.iter().position(|old| old == id).unwrap();
                (id.0.clone(), (before as f32 - at as f32) * 30.)
            }).collect();
            self.sidebar_drag.generation += 1;
            crate::haptics::tick();
        }
        self.overrides.set_order(&ids);
        self.sort_terminal_rows();
        self.save_overrides();
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
        self.overrides.initialize_pins(&self.records);
        self.overrides.set_pinned(id, !row.pinned);
        self.save_overrides();
        cx.notify();
    }

    pub fn reply_finished(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        self.terminal_working.remove(id);
        cx.notify();
        let visible = self.window_active && self.panel == Panel::Chat
            && matches!(&self.selection,
            Selection::Session(current) | Selection::Terminal(current) if current == id);
        if visible {
            self.mark_visible_chat_read(cx);
        } else {
            self.mark_session_unread(id, cx);
        }
    }

    pub fn mark_visible_chat_read(&mut self, cx: &mut Context<Self>) {
        if self.panel != Panel::Chat { return; }
        let id = match &self.selection {
            Selection::Session(id) | Selection::Terminal(id) => id.clone(),
            Selection::Draft(_) => return,
        };
        if self.overrides.unread(&id) {
            self.set_unread(&id, false);
            self.save_overrides();
            cx.notify();
        }
    }

    pub fn window_activation_changed(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if active { self.mark_visible_chat_read(cx); }
        cx.notify();
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
        self.deleted_panels.push((
            format!("terminal:{}", id.as_str()),
            self.uids
                .get(&Tab::Terminal(id.clone()))
                .copied()
                .unwrap_or(u64::MAX),
        ));
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
        self.project_info.remove(&row.cwd);
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
            self.launch_options.remove(&key);
            self.draft_modes.remove(&key);
            self.starting.remove(&key);
            self.pending_prompts.remove(&key);
        }
        self.select_terminal(id.clone(), cx);
        self.keep_tab(&Tab::Terminal(id), cx);
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

    pub fn pending_input(&self) -> Option<&UserInput> {
        let Selection::Draft(key) = self.selection else {
            return None;
        };
        self.pending_prompts.get(&key).map(|pending| &pending.input)
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
            .get(&(provider, self.selected_account(), cwd))
            .cloned()
            .unwrap_or_default()
    }

    pub fn load_models(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let (provider, _, cwd) = self.effective();
        let account = self.selected_account();
        let catalog = self.catalogs.entry((provider, account.clone(), cwd.clone())).or_default();
        if catalog.loading || (catalog.loaded && !refresh) {
            return;
        }
        catalog.loading = true;
        catalog.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        self.rt.spawn(async move {
            let result = async {
                manager
                    .account_models(provider, account.as_deref(), &cwd)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            let _ = tx.send(Msg::Models(provider, account, cwd, result));
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
            || matches!(&self.selection, Selection::Session(id) if self.switching_accounts.contains(id))
    }

    /// New drafts inherit the selected session directory; remembered directory is a fallback.
    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        self.show_archived = false;
        let (provider, model, cwd) = self.effective();
        let effort = self.effort();
        let tier = self.service_tier();
        let account = self.default_account(provider).map(str::to_owned);
        let has_context = self.tabs.iter().any(|tab| tab.selection() == self.selection);
        let cwd = if has_context { cwd } else { self.project_preferences.last_directory.clone().unwrap_or(cwd) };
        self.project_preferences.last_directory = Some(cwd.clone());
        self.project_info.remove(&cwd);
        self.open_draft(provider, model, cwd, cx);
        if let (Selection::Draft(key), Some(account)) = (&self.selection, account) { self.draft_accounts.insert(*key, account); }
        if let Some(draft) = self.draft_mut() {
            draft.reasoning_effort = effort;
            draft.service_tier = tier;
        }
        self.save_projects(cx);
    }

    fn open_draft(
        &mut self,
        provider: Provider,
        model: Option<String>,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) {
        // Replace an automatic empty composer rather than retaining a hidden draft.
        if let Selection::Draft(old) = self.selection
            && !self.tabs.contains(&Tab::Draft(old)) && !self.starting.contains(&old)
        {
            self.drafts.remove(&old);
            self.draft_modes.remove(&old);
            self.draft_accounts.remove(&old);
            self.launch_options.remove(&old);
            self.uids.remove(&Tab::Draft(old));
        }
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
                permission_mode: providers::permission_mode(provider),
            },
        );
        if let Some(account) = self.default_account(provider).map(str::to_owned) { self.draft_accounts.insert(key, account); }
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

    fn open_preview(&mut self, tab: Tab) {
        if self.tabs.contains(&tab) {
            return; // Selecting an already-open tab never makes it temporary.
        }
        if !self.uids.contains_key(&tab) {
            self.new_uid(tab.clone());
        }
        if let Some(at) = self
            .preview_tab
            .as_ref()
            .and_then(|preview| self.tabs.iter().position(|open| open == preview))
        {
            // Keep each session's UID for its cached composer text/attachments.
            // Replacing a preview does not close its backend or delete history.
            self.tabs[at] = tab.clone();
        } else {
            self.tabs.push(tab.clone());
        }
        self.preview_tab = Some(tab);
    }

    pub fn keep_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        if self.preview_tab.as_ref() == Some(tab) {
            self.preview_tab = None;
            cx.notify();
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
            self.keep_tab(&tab, cx);
            self.tabs.insert(to, tab);
            cx.notify();
        }
    }

    /// Close a tab. A session stays in the sidebar list and keeps running.
    pub fn close_tab(&mut self, tab: &Tab, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t == tab) else {
            return;
        };
        self.keep_tab(tab, cx);
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
            if !self.starting.contains(k) { self.launch_options.remove(k); }
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
        if self.unloading.contains(&id) {
            return;
        }
        self.set_unread(&id, false);
        self.save_overrides();
        self.open_preview(Tab::Session(id.clone()));
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
        if matches!(&self.selection, Selection::Session(id) if self.switching_accounts.contains(id)) { return; }
        let text = input.text.clone();
        self.error = None;
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
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
                let account = self.draft_accounts.get(&key).cloned();
                let options = self.launch_options.entry(key).or_insert_with(|| crate::projects::LaunchOptions { token: uuid::Uuid::new_v4().to_string(), ..Default::default() }).clone();
                self.rt.spawn(async move {
                    let result = async {
                        let mut config = config;
                        let cwd = config.cwd.clone();
                        config.cwd = tokio::task::spawn_blocking(move || crate::projects::prepare(&cwd, &options, &del_agent::data_dir().unwrap_or_else(std::env::temp_dir)))
                            .await.map_err(|e| AgentError::Backend(e.to_string()))?.map_err(AgentError::Backend)?;
                        let handle = manager.start_account_session(config, account).await?;
                        let submitted = std::time::Instant::now();
                        manager.send_prompt(&handle.id, input.clone()).await?;
                        eprintln!(
                            "[del startup] first prompt accepted in {}ms",
                            submitted.elapsed().as_millis()
                        );
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
                self.keep_tab(&Tab::Session(id.clone()), cx);
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

    /// Release the live process and view, retaining history and sidebar metadata.
    pub fn unload(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.terminal_rows.iter().any(|row| row.id == id) {
            self.close_tab(&Tab::Terminal(id.clone()), cx);
            // A preview or already closed tab may still own a terminal host.
            if !self.closed_terminals.contains(&id) {
                self.closed_terminals.push(id.clone());
            }
            if let Some(row) = self.terminal_rows.iter_mut().find(|row| row.id == id) {
                row.active = false;
            }
        } else if self.unloading.insert(id.clone()) {
            self.close_tab(&Tab::Session(id.clone()), cx);
            self.unloaded_chats.push(id.clone());
            let (manager, tx) = (self.manager.clone(), self.tx.clone());
            self.rt.spawn(async move {
                if let Err(error) = manager.close(&id).await {
                    let _ = tx.send(Msg::Error(format!("Could not unload session: {error}")));
                }
                let _ = tx.send(Msg::Unloaded(id));
            });
        }
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
        let root = self.records.iter().find(|row| row.id == id).map(|row| &row.cwd)
            .or_else(|| self.terminal_rows.iter().find(|row| row.id == id).map(|row| &row.cwd))
            .map(|cwd| self.project_root(cwd));
        if let Some(root) = root {
            self.project_preferences.archived.remove(&root);
            self.project_preferences.deleted.remove(&root);
            self.project_preferences.collapsed.remove(&root);
            self.project_preferences.roots.insert(root);
            self.save_projects(cx);
        }
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
        let uid = self
            .uids
            .get(&Tab::Session(id.clone()))
            .copied()
            .unwrap_or(u64::MAX);
        self.close_tab(&Tab::Session(id.clone()), cx);
        self.records.retain(|record| record.id != id);
        self.overrides.remove(&id);
        self.save_overrides();
        let (manager, tx) = (self.manager.clone(), self.tx.clone());
        self.rt.spawn(async move {
            if let Err(error) = manager.delete(&id).await {
                let _ = tx.send(Msg::Error(format!("Could not delete session: {error}")));
            } else {
                let _ = tx.send(Msg::PanelDeleted(format!("chat:{}", id.as_str()), uid));
            }
            let _ = tx.send(Msg::Refresh);
        });
        cx.notify();
    }

    pub fn start_rename(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.project_renaming = None;
        self.renaming = Some(id);
        cx.notify();
    }

    pub fn start_project_rename(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.renaming = None;
        self.project_renaming = Some(path);
        cx.notify();
    }

    /// `None` cancels.
    pub fn finish_rename(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        if let Some(path) = self.project_renaming.take() {
            if let Some(name) = title { self.rename_project(path, name, cx); }
            cx.notify();
            return;
        }
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
        (self.terminal_title_loading.contains(id) || self.chat_title_loading.contains(id))
            && self.overrides.title(id).is_none()
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

    pub fn session_account(&self, id: &SessionId) -> Option<crate::accounts::Account> {
        let record = self.records.iter().find(|r| &r.id == id)?;
        crate::accounts::for_record(record)
    }

    pub fn selected_account(&self) -> Option<String> {
        match &self.selection {
            Selection::Draft(key) => self.draft_accounts.get(key).cloned(),
            Selection::Session(id) => self.records.iter().find(|r| &r.id == id).and_then(|r| r.account_id.clone()),
            Selection::Terminal(id) => self.terminal_rows.iter().find(|r| &r.id == id).and_then(|r| r.account_id.clone()),
        }
    }

    pub fn select_default_account(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let account = self.default_account(provider).map(str::to_owned);
        self.select_account(provider, account, cx);
    }

    pub fn select_account(&mut self, provider: Provider, account: Option<String>, cx: &mut Context<Self>) {
        if self.is_starting() { return; }
        if account.as_ref().is_some_and(|id| !self.accounts.iter().any(|a| &a.id == id && a.provider == provider)) { return; }
        if account.as_ref().is_some_and(|id| self.account_status.get(id).is_some_and(|status| status == "Waiting for sign-in…")) {
            self.error = Some("Finish signing in before selecting this account.".into()); cx.notify(); return;
        }
        if self.effective().0 == provider && self.selected_account() == account { return; }
        if let Selection::Session(id) = self.selection.clone() {
            if self.effective().0 == provider {
                if matches!(self.manager.status(&id), Some(SessionStatus::Running | SessionStatus::WaitingApproval)) {
                    self.error = Some("Wait for the response to finish before switching accounts.".into()); cx.notify(); return;
                }
                let target = account.as_ref().and_then(|id| self.accounts.iter().find(|a| &a.id == id)).cloned();
                self.switching_accounts.insert(id.clone());
                let (manager, tx) = (self.manager.clone(), self.tx.clone());
                self.rt.spawn(async move {
                    let result = manager.switch_account(&id, account, |record| crate::accounts::transfer_conversation(record, target)).await;
                    let _ = tx.send(Msg::AccountSwitched(id, result.map_err(|e| e.to_string())));
                });
                cx.notify(); return;
            }
        }
        if !self.is_draft() { self.fork_with(provider, None, cx); }
        self.set_provider(provider, cx);
        if let Selection::Draft(key) = self.selection {
            self.draft_accounts.remove(&key);
            if let Some(account) = account { self.draft_accounts.insert(key, account); }
        }
        if let Some(draft) = self.draft_mut() { draft.model = None; draft.reasoning_effort = None; draft.service_tier = None; }
        self.load_models(false, cx);
        cx.notify();
    }

    pub fn visible_accounts(&self, provider: Provider) -> Vec<&crate::accounts::Account> {
        let mut seen = std::collections::HashSet::new();
        if let Some(identity) = self.default_identities.get(&provider).and_then(|s| crate::accounts::identity_key(s)) {
            seen.insert(identity);
        }
        self.accounts.iter().filter(|account| account.provider == provider)
            .filter(|account| account.identity.as_deref().and_then(crate::accounts::identity_key)
                .is_none_or(|identity| seen.insert(identity))).collect()
    }

    pub fn account_usage_remaining(&self, provider: Provider, id: Option<&str>) -> Option<f64> {
        self.account_usage.get(&(provider, id.map(str::to_owned))).copied().flatten()
    }

    pub fn account_is_selected(&self, provider: Provider, candidate: Option<&str>) -> bool {
        if self.effective().0 != provider { return false; }
        let selected = self.selected_account();
        if selected.as_deref() == candidate { return true; }
        let identity = |id: Option<&str>| {
            let value = match id {
                Some(id) => self.accounts.iter().find(|a| a.id == id && a.provider == provider).and_then(|a| a.identity.as_deref()),
                None => self.default_identities.get(&provider).map(String::as_str),
            };
            value.and_then(crate::accounts::identity_key)
        };
        identity(selected.as_deref()).is_some_and(|key| identity(candidate).as_ref() == Some(&key))
    }

    fn refresh_account_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_loading || cfg!(test) { return; }
        self.usage_loading = true;
        let accounts = [Provider::Codex, Provider::Claude].into_iter()
            .map(|provider| (provider, self.visible_accounts(provider).into_iter().cloned().collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            for (provider, accounts) in accounts {
                let usage = crate::accounts::remaining_usage(provider, None).await;
                let _ = tx.send(Msg::AccountUsage(provider, None, usage));
                for account in accounts {
                    let usage = crate::accounts::remaining_usage(provider, Some(&account)).await;
                    let _ = tx.send(Msg::AccountUsage(provider, Some(account.id), usage));
                }
            }
            let _ = tx.send(Msg::AccountUsageFinished);
        });
        cx.notify();
    }

    pub fn default_account(&self, provider: Provider) -> Option<&str> {
        self.accounts.iter().find(|a| a.provider == provider && a.is_default).map(|a| a.id.as_str())
    }

    pub fn account_needs_login(&self, id: &str) -> bool {
        self.account_status.get(id).map(|status| status != "Connected" && status != "Waiting for sign-in…")
            .unwrap_or_else(|| self.accounts.iter().find(|a| a.id == id).is_some_and(|a| a.requires_login))
    }

    pub fn make_default_account(&mut self, provider: Provider, id: Option<String>, cx: &mut Context<Self>) {
        if self.account_login_pending(provider) { return; }
        if id.as_ref().is_some_and(|id| self.account_needs_login(id) || !self.accounts.iter().any(|a| &a.id == id && a.provider == provider)) { return; }
        let mut accounts = self.accounts.clone();
        for account in accounts.iter_mut().filter(|a| a.provider == provider) { account.is_default = id.as_ref() == Some(&account.id); }
        if !cfg!(test) && let Err(error) = crate::accounts::save(&accounts) {
            self.error = Some(format!("Could not save default account: {error}"));
        } else { self.accounts = accounts; }
        cx.notify();
    }

    pub fn delete_account(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(account) = self.accounts.iter().find(|a| a.id == id).cloned() else { return; };
        if self.account_login_pending(account.provider) { return; }
        if !self.switching_accounts.is_empty() || self.starting.iter().any(|key| self.draft_accounts.get(key).is_some_and(|account| account == id)) {
            self.error = Some("Wait for account switching or chat startup to finish before deleting an account.".into()); cx.notify(); return;
        }
        if self.manager.list_sessions().iter().any(|r| r.account_id.as_deref() == Some(id))
            || self.terminal_rows.iter().any(|r| r.account_id.as_deref() == Some(id)) {
            self.error = Some("This account is used by existing chats. Switch those chats to another account before deleting it.".into());
            cx.notify(); return;
        }
        let accounts: Vec<_> = self.accounts.iter().filter(|a| a.id != id).cloned().collect();
        let result = (|| -> std::io::Result<()> {
            if cfg!(test) { return Ok(()); }
            let source = account.root();
            let destination = del_agent::data_dir().unwrap().join("removed-accounts").join(format!("{}-{}", id, uuid::Uuid::new_v4()));
            let moved = source.exists();
            if moved { std::fs::create_dir_all(destination.parent().unwrap())?; std::fs::rename(&source, &destination)?; }
            if let Err(error) = crate::accounts::save(&accounts) {
                if moved { let _ = std::fs::rename(destination, source); }
                return Err(error);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.accounts = accounts;
                self.draft_accounts.retain(|_, account| account != id);
                self.account_status.remove(id);
                self.account_usage.retain(|(_, account_id), _| account_id.as_deref() != Some(id));
                self.catalogs.retain(|(_, account, _), _| account.as_deref() != Some(id));
                if let Some(backend) = self.manager.unregister_account(account.provider, id) {
                    self.rt.spawn(async move { let _ = backend.shutdown().await; });
                }
            }
            Err(error) => self.error = Some(format!("Could not delete account: {error}")),
        }
        cx.notify();
    }

    pub fn default_account_label(&self, provider: Provider) -> String {
        if provider == Provider::OpenCode { return "System".into(); }
        self.default_identities.get(&provider).map(|identity| crate::accounts::display_identity(identity)).unwrap_or_else(|| "Account 1".into())
    }

    pub fn account_login_pending(&self, provider: Provider) -> bool {
        self.signin == Some(provider) || self.accounts.iter().any(|a| a.provider == provider
            && self.account_status.get(&a.id).is_some_and(|status| status == "Waiting for sign-in…"))
    }

    pub fn add_account(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.account_login_pending(provider) { return; }
        if provider == Provider::Acp { return; }
        match crate::accounts::create(provider, &self.accounts) {
            Ok(account) => {
                let id = account.id.clone();
                self.accounts.push(account.clone());
                if let Err(e) = crate::accounts::save(&self.accounts) {
                    self.accounts.pop(); self.error = Some(format!("Could not save account: {e}"));
                } else {
                    self.manager.register_account(id.clone(), account.backend());
                    self.login_account(&id, cx);
                }
            }
            Err(e) => self.error = Some(format!("Could not add account: {e}")),
        }
        cx.notify();
    }

    pub fn login_account(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.account_status.get(id).is_some_and(|status| status == "Waiting for sign-in…") { return; }
        let Some(account) = self.accounts.iter().find(|a| a.id == id).cloned() else { return; };
        if self.account_login_pending(account.provider) { return; }
        self.account_status.insert(id.to_owned(), "Waiting for sign-in…".into());
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let binary = match crate::accounts::login_binary(account.provider).await {
                Ok(binary) => binary,
                Err(error) => { let _ = tx.send(Msg::AccountChecked(account.id, Err(error))); return; }
            };
            let login_account = account.clone();
            let login_binary = binary.clone();
            let marker = match tokio::task::spawn_blocking(move || crate::accounts::launch_login(&login_account, &login_binary)).await {
                Ok(Ok(marker)) => marker,
                result => { let _ = tx.send(Msg::AccountChecked(account.id, Err(format!("Could not open sign-in: {result:?}")))); return; }
            };
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let Ok(status) = std::fs::read_to_string(&marker) else { continue; };
                if status.is_empty() { continue; }
                if status.trim() != "0" {
                    let _ = std::fs::remove_file(&marker);
                    let _ = tx.send(Msg::AccountChecked(account.id, Err("Sign-in failed or was cancelled. Check Terminal, then try Sign in again.".into()))); return;
                }
                if let Ok(identity) = crate::accounts::identity(&account, &binary).await {
                    let close_marker = marker.clone();
                    let _ = tokio::task::spawn_blocking(move || crate::accounts::close_login_terminal(&close_marker)).await;
                    let _ = std::fs::remove_file(&marker);
                    let _ = tx.send(Msg::AccountChecked(account.id, Ok(identity))); return;
                }
            }
            let _ = std::fs::remove_file(&marker);
            let _ = tx.send(Msg::AccountChecked(account.id, Err("Sign-in not confirmed. Try reconnecting.".into())));
        });
        cx.notify();
    }

    pub fn set_provider(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.effective().0 != provider && let Selection::Draft(key) = self.selection {
            self.draft_accounts.remove(&key);
            if let Some(account) = self.default_account(provider).map(str::to_owned) { self.draft_accounts.insert(key, account); }
        }
        if let Some(d) = self.draft_mut() {
            if d.provider != provider {
                d.provider = provider;
                d.permission_mode = providers::permission_mode(provider);
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

    /// Stable project identity shared by chat and terminal sessions.
    pub fn project_root(&self, cwd: &std::path::Path) -> PathBuf {
        self.project_info.get(cwd).map(|info| info.root.clone())
            .or_else(|| self.project_preferences.directories.get(cwd).cloned()).unwrap_or_else(|| cwd.to_path_buf())
    }

    fn inspect_projects(&mut self) {
        let mut paths: Vec<_> = self.records.iter().map(|r| r.cwd.clone())
            .chain(self.terminal_rows.iter().map(|r| r.cwd.clone()))
            .chain(self.drafts.values().map(|d| d.cwd.clone())).collect();
        paths.sort(); paths.dedup();
        for path in paths {
            if self.project_info.contains_key(&path) || !self.project_loading.insert(path.clone()) { continue; }
            let tx = self.tx.clone();
            self.rt.spawn(async move {
                let dir = path.clone();
                if let Ok(info) = tokio::task::spawn_blocking(move || crate::projects::inspect(&dir)).await {
                    let _ = tx.send(Msg::ProjectInspected(path, info));
                }
            });
        }
    }

    pub fn unified_rows(&self) -> Vec<Row> {
        let mut rows = self.rows();
        rows.extend(self.terminal_rows.iter().filter(|r| !self.terminal_archived(&r.id)).map(|r| Row {
            id: r.id.clone(), provider: r.provider.unwrap_or(Provider::Claude), model: None,
            cwd: r.cwd.clone(), title: self.current_title(&r.id), status: if self.session_working(&r.id) { SessionStatus::Running } else { SessionStatus::Idle },
            updated_secs: 0, archived: false, pinned: r.pinned, unread: self.terminal_unread(&r.id),
        }));
        rows.sort_by_key(|r| (!r.pinned, self.overrides.order(&r.id).unwrap_or(if r.pinned { 0 } else { u64::MAX }), std::cmp::Reverse(r.updated_secs)));
        rows
    }

    pub fn project_drafts(&self, path: &std::path::Path) -> Vec<u64> {
        self.tabs.iter().filter_map(|tab| match tab {
            Tab::Draft(key) if self.drafts.get(key).is_some_and(|d| self.project_root(&d.cwd) == path) => Some(*key),
            _ => None,
        }).collect()
    }

    pub fn project_paths(&self) -> Vec<PathBuf> {
        // Projects are directory groups, not independent visibility gates.
        let mut paths: Vec<_> = if self.show_archived {
            self.archived_rows().iter().map(|row| self.project_root(&row.cwd))
                .chain(self.terminal_rows.iter().filter(|row| self.terminal_archived(&row.id)).map(|row| self.project_root(&row.cwd))).collect()
        } else {
            self.unified_rows().iter().map(|row| self.project_root(&row.cwd))
                .chain(self.tabs.iter().filter_map(|tab| match tab { Tab::Draft(key) => self.drafts.get(key), _ => None }).map(|draft| self.project_root(&draft.cwd))).collect()
        };
        paths.sort(); paths.dedup(); paths
    }

    pub fn project_name(&self, path: &std::path::Path) -> String {
        self.project_preferences.names.get(path).cloned().unwrap_or_else(|| sessions::folder_name(path).to_string())
    }

    pub fn rename_project(&mut self, path: PathBuf, name: String, cx: &mut Context<Self>) {
        if !name.trim().is_empty() { self.project_preferences.names.insert(path, name.trim().to_owned()); }
        self.save_projects(cx);
    }

    fn save_projects(&mut self, cx: &mut Context<Self>) {
        if !cfg!(test) && let Err(error) = self.project_preferences.save() { self.error = Some(error.to_string()); }
        cx.notify();
    }

    pub fn archive_project(&mut self, path: PathBuf, delete: bool, cx: &mut Context<Self>) {
        let archived_only = delete && self.show_archived;
        let drafts = if archived_only { Vec::new() } else { self.project_drafts(&path) };
        if drafts.iter().any(|key| self.starting.contains(key)) {
            self.error = Some("Wait for this project's session to finish starting.".into());
            cx.notify();
            return;
        }
        let ids: Vec<_> = self.records.iter().map(|r| (&r.id, &r.cwd))
            .chain(self.terminal_rows.iter().map(|r| (&r.id, &r.cwd)))
            .filter(|(id, cwd)| self.project_root(cwd) == path && (!archived_only || self.overrides.archived(id))).map(|(id, _)| id.clone()).collect();
        let existing_tabs = self.tabs.clone();
        for id in ids { if delete { self.permanently_delete(id, cx); } else { self.archive(id, cx); } }
        for key in drafts { self.close_tab(&Tab::Draft(key), cx); }
        // Closing the final tab prepares a composer, not a replacement project.
        self.tabs.retain(|tab| existing_tabs.contains(tab));
        if delete {
            self.project_preferences.archived.remove(&path);
            let remains = self.records.iter().any(|r| self.project_root(&r.cwd) == path)
                || self.terminal_rows.iter().any(|r| self.project_root(&r.cwd) == path)
                || !self.project_drafts(&path).is_empty();
            if !remains {
                self.project_preferences.deleted.insert(path.clone());
                self.project_preferences.names.remove(&path);
                self.project_preferences.roots.remove(&path);
            }
        } else { self.project_preferences.archived.insert(path.clone()); }
        self.save_projects(cx);
    }

    pub fn restore_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.project_preferences.archived.remove(&path);
        let ids: Vec<_> = self.records.iter().map(|r| (&r.id, &r.cwd)).chain(self.terminal_rows.iter().map(|r| (&r.id, &r.cwd)))
            .filter(|(_, cwd)| self.project_root(cwd) == path).map(|(id, _)| id.clone()).collect();
        for id in ids { self.overrides.set_archived(&id, false); }
        self.save_overrides();
        self.save_projects(cx);
    }

    pub fn cleanup_project_worktrees(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let mut used: Vec<_> = self.records.iter().map(|r| r.cwd.clone()).chain(self.terminal_rows.iter().map(|r| r.cwd.clone())).collect();
        let storage = del_agent::data_dir().unwrap_or_else(std::env::temp_dir);
        used.extend(self.launch_options.values().map(|o| storage.join("worktrees").join(&o.token).join("checkout")));
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || crate::projects::cleanup(&root, &used, &storage)).await;
            if let Err(error) = result.map_err(|e| e.to_string()).and_then(|r| r) {
                let _ = tx.send(Msg::Error(format!("Worktree cleanup: {error}")));
            }
        });
        cx.notify();
    }

    pub fn toggle_sidebar_section(&mut self, section: &str, cx: &mut Context<Self>) {
        if !self.project_preferences.collapsed_sections.remove(section) {
            self.project_preferences.collapsed_sections.insert(section.to_owned());
        }
        self.save_projects(cx);
    }

    pub fn toggle_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.project_preferences.collapsed.remove(&path) { self.project_preferences.collapsed.insert(path); }
        if !cfg!(test) && let Err(e) = self.project_preferences.save() { self.error = Some(e.to_string()); }
        cx.notify();
    }

    pub fn new_project_session(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.project_preferences.archived.remove(&path);
        self.project_preferences.deleted.remove(&path);
        self.project_preferences.roots.insert(path.clone());
        self.new_session(cx);
        if let Selection::Draft(key) = self.selection {
            if let Some(draft) = self.drafts.get_mut(&key) {
                if draft.cwd != path { self.launch_options.remove(&key); }
                draft.cwd = path.clone();
            }
        }
        self.project_preferences.last_directory = Some(path.clone());
        self.project_preferences.collapsed.remove(&path);
        if !cfg!(test) && let Err(error) = self.project_preferences.save() { self.error = Some(error.to_string()); }
        cx.notify();
    }

    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Choose project directory".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picker.await && let Some(path) = paths.pop() {
                let _ = this.update(cx, |s, cx| s.new_project_session(path, cx));
            }
        }).detach();
    }

    pub fn draft_launch_options(&self) -> crate::projects::LaunchOptions {
        match self.selection { Selection::Draft(key) => self.launch_options.get(&key).cloned().unwrap_or_default(), _ => Default::default() }
    }

    pub fn set_launch_options(&mut self, branch: Option<String>, worktree: bool, cx: &mut Context<Self>) {
        let Selection::Draft(key) = self.selection else { return; };
        if self.is_starting() { return; }
        self.launch_options.insert(key, crate::projects::LaunchOptions { branch, worktree, token: uuid::Uuid::new_v4().to_string() });
        cx.notify();
    }

    fn set_draft_directory(&mut self, key: u64, dir: PathBuf, cx: &mut Context<Self>) {
        if self.starting.contains(&key) { return; }
        let Some(draft) = self.drafts.get_mut(&key) else { return; };
        draft.cwd = dir.clone();
        let root = self.project_root(&dir);
        self.project_preferences.collapsed.remove(&root);
        self.project_preferences.archived.remove(&root);
        self.project_preferences.deleted.remove(&root);
        self.project_preferences.last_directory = Some(dir);
        self.launch_options.remove(&key);
        self.show_archived = false;
        self.save_projects(cx);
    }

    pub fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let Selection::Draft(key) = self.selection else { return; };
        if self.is_starting() { return; }
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
                    s.set_draft_directory(key, dir, cx);
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
        if matches!(provider, Provider::Codex | Provider::Claude) { self.refresh_account_usage(cx); }
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
        if self.signin.is_some() || self.account_login_pending(provider) {
            return;
        }
        self.signin = Some(provider);
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = async {
                let binary = crate::accounts::login_binary(provider).await?;
                let launch_binary = binary.clone();
                let marker = tokio::task::spawn_blocking(move || crate::accounts::launch_provider_login(provider, None, &launch_binary))
                    .await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
                for _ in 0..100 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    let Ok(status) = std::fs::read_to_string(&marker) else { continue; };
                    if status.is_empty() { continue; }
                    if status.trim() != "0" { return Err("Sign-in cancelled or failed. Try Sign in again.".to_string()); }
                    let closing = marker.clone();
                    let _ = tokio::task::spawn_blocking(move || crate::accounts::close_login_terminal(&closing)).await;
                    let _ = std::fs::remove_file(&marker);
                    if let Ok(Some(identity)) = crate::accounts::provider_identity(provider, None, &binary).await {
                        let _ = tx.send(Msg::AccountIdentity(provider, None, identity));
                    }
                    let _ = tx.send(Msg::Checked(providers::detect(provider).await));
                    return Ok(());
                }
                Err("Sign-in timed out. Try Sign in again.".into())
            }.await;
            if let Err(error) = result { let _ = tx.send(Msg::Error(error)); }
            let _ = tx.send(Msg::SigninFinished(provider));
        });
        cx.notify();
    }

    /// Close every session and kill every backend child process. Blocks, with
    /// a deadline, because the app is quitting.
    pub fn shutdown(&self) {
        if let Some(readiness) = &self.readiness {
            readiness.stop();
        }
        let manager = self.manager.clone();
        self.rt.block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3), manager.shutdown()).await;
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        AppState, Arc, HashMap, Overrides, Panel, Provider, Selection, SessionConfig,
        SessionManager, Tab, unbounded_channel,
    };
    use crate::providers;
    use gpui::AppContext as _;
    use gpui::Entity;

    #[gpui::test]
    fn draft_permissions_follow_provider_on_new_tabs_and_switches(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |state, cx| {
            for provider in Provider::ALL {
                state.set_provider(provider, cx);
                assert_eq!(
                    state.draft().unwrap().permission_mode,
                    providers::permission_mode(provider)
                );
                state.new_session(cx);
                assert_eq!(
                    state.draft().unwrap().permission_mode,
                    providers::permission_mode(state.draft().unwrap().provider)
                );
            }
        });
    }

    #[gpui::test]
    fn closing_settings_preserves_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let selection = s.selection.clone();
            s.panel = Panel::Providers;
            s.toggle_providers(cx);
            assert_eq!(s.panel, Panel::Chat);
            assert_eq!(s.selection, selection);
            assert_eq!(s.selection, selection);
        });
    }

    // A parked runtime makes startup deterministically slow: no provider work
    // can finish before we check the UI state after send().
    pub(crate) fn draft(cx: &mut gpui::TestAppContext) -> Entity<AppState> {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(dir.path().join("sessions.jsonl"), []).unwrap();
        let (tx, _rx) = unbounded_channel();
        cx.new(|_| AppState {
            project_info: HashMap::new(),
            project_loading: Default::default(),
            project_preferences: Default::default(),
            launch_options: HashMap::new(),
            catalogs: HashMap::new(),
            accounts: Vec::new(),
            account_status: HashMap::new(),
            default_identities: HashMap::new(),
            account_usage: HashMap::new(),
            usage_loading: false,
            draft_accounts: HashMap::new(),
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
                    permission_mode: providers::permission_mode(Provider::Claude),
                },
            )]),
            next_draft: 1,
            draft_modes: HashMap::new(),
            terminal_rows: vec![],
            terminal_working: Default::default(),
            terminal_title_loading: Default::default(),
            chat_title_loading: Default::default(),
            unread_motion: HashMap::new(),
            readiness: None,
            browser_tools_ready: true,
            closed_terminals: vec![],
            unloaded_chats: vec![],
            unloading: Default::default(),
            removed_terminals: vec![],
            deleted_panels: vec![],
            terminal_pin_changes: vec![],
            panel: Panel::Chat,
            preferences_open: true,
            shortcut_hints: false,
            window_active: true,
            sidebar_drag: Default::default(),
            sidebar_hover: Default::default(),
            providers: vec![],
            signin: None,
            checking: Default::default(),
            caffeine_enabled: false,
            renaming: None,
            project_renaming: None,
            show_archived: false,
            error: None,
            first_prompts: HashMap::new(),
            pending_prompts: HashMap::new(),
            tabs: vec![Tab::Draft(0)],
            preview_tab: None,
            uids: HashMap::from([(Tab::Draft(0), 0)]),
            next_uid: 1,
            ghosts: vec![],
            starting: Default::default(),
            switching_accounts: Default::default(),
        })
    }

    #[gpui::test]
    fn terminal_previews_share_the_chat_slot_and_can_be_kept(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let a = del_agent::SessionId::new("terminal-preview-a");
            let b = del_agent::SessionId::new("terminal-preview-b");
            s.select_terminal(a.clone(), cx);
            let uid = s.uid_of(&Tab::Terminal(a.clone()));
            s.select_terminal(b.clone(), cx);
            assert_eq!(s.tabs, vec![Tab::Draft(0), Tab::Terminal(b.clone())]);
            assert_eq!(s.preview_tab, Some(Tab::Terminal(b.clone())));
            assert!(s.closed_terminals.is_empty());
            s.keep_tab(&Tab::Terminal(b.clone()), cx);
            s.select_terminal(a.clone(), cx);
            assert_eq!(s.uid_of(&Tab::Terminal(a.clone())), uid);
            s.select(del_agent::SessionId::new("chat-preview"), cx);
            assert!(s.tabs.contains(&Tab::Terminal(b)));
            assert!(!s.tabs.contains(&Tab::Terminal(a)));
            assert!(s.closed_terminals.is_empty());
        });
    }

    #[gpui::test]
    fn history_previews_replace_in_place_and_keep_separate_composer_owners(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let a = del_agent::SessionId::new("preview-a");
            let b = del_agent::SessionId::new("preview-b");
            s.select(a.clone(), cx);
            let owner_a = s.uid_of(&Tab::Session(a.clone()));
            s.select(b.clone(), cx);
            assert_eq!(s.tabs, vec![Tab::Draft(0), Tab::Session(b.clone())]);
            assert_eq!(s.preview_tab, Some(Tab::Session(b.clone())));
            assert!(s.ghosts.is_empty());
            assert_ne!(owner_a, s.uid_of(&Tab::Session(b)));
            s.select(a.clone(), cx);
            assert_eq!(s.uid_of(&Tab::Session(a)), owner_a);
        });
    }

    #[gpui::test]
    fn keeping_a_preview_preserves_it_and_existing_tabs_never_become_previews(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let a = del_agent::SessionId::new("kept-a");
            let b = del_agent::SessionId::new("preview-b");
            let c = del_agent::SessionId::new("preview-c");
            s.select(a.clone(), cx);
            s.keep_tab(&Tab::Session(a.clone()), cx);
            s.keep_tab(&Tab::Session(a.clone()), cx);
            s.select(b.clone(), cx);
            s.select(a.clone(), cx);
            assert_eq!(s.preview_tab, Some(Tab::Session(b)));
            s.select(c.clone(), cx);
            assert_eq!(
                s.tabs,
                vec![Tab::Draft(0), Tab::Session(a), Tab::Session(c.clone())]
            );
            s.close_tab(&Tab::Session(c), cx);
            assert!(s.preview_tab.is_none());
        });
    }

    #[gpui::test]
    fn chat_submission_event_keeps_the_preview_before_backend_delivery(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = draft(cx);
        let id = del_agent::SessionId::new("queued-preview");
        let backend = state.update(cx, |s, cx| {
            s.select(id.clone(), cx);
            s.backend()
        });
        let chat = cx.new(|cx| crate::chat::ChatView::new(backend, None, None, cx));
        state.update(cx, |_, cx| {
            cx.subscribe(
                &chat,
                move |state, _, _: &crate::chat::PromptSubmitted, cx| {
                    state.keep_tab(&Tab::Session(id.clone()), cx);
                },
            )
            .detach();
        });
        chat.update(cx, |chat, cx| {
            chat.submit_input(del_agent::UserInput::text("queued message"), false, cx);
        });
        cx.run_until_parked();
        state.read_with(cx, |state, _| assert!(state.preview_tab.is_none()));
    }

    #[gpui::test]
    fn sending_promotes_a_preview_even_while_delivery_is_pending(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let a = del_agent::SessionId::new("sent-a");
            let b = del_agent::SessionId::new("preview-b");
            s.select(a.clone(), cx);
            s.send("keep this conversation".into(), cx);
            assert!(s.preview_tab.is_none());
            s.select(b.clone(), cx);
            assert_eq!(
                s.tabs,
                vec![Tab::Draft(0), Tab::Session(a), Tab::Session(b)]
            );
        });
    }

    #[gpui::test]
    fn returning_to_chat_or_submitting_clears_stale_unread(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let id = del_agent::SessionId::new("visible-reply");
            s.select(id.clone(), cx);
            s.window_activation_changed(false, cx);
            s.reply_finished(&id, cx);
            assert!(s.terminal_unread(&id));
            s.window_activation_changed(true, cx);
            assert!(!s.terminal_unread(&id));
            s.mark_session_unread(&id, cx);
            s.reply_finished(&id, cx);
            assert!(!s.terminal_unread(&id));
            s.mark_session_unread(&id, cx);
            s.mark_visible_chat_read(cx);
            assert!(!s.terminal_unread(&id));
            let other = del_agent::SessionId::new("background-reply");
            s.reply_finished(&other, cx);
            s.window_activation_changed(true, cx);
            assert!(s.terminal_unread(&other));
        });
    }

    #[gpui::test]
    fn selected_chat_finishing_in_inactive_window_is_unread(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let id = del_agent::SessionId::new("away-reply");
            s.select(id.clone(), cx);
            s.window_active = false;
            s.apply(super::Msg::ReplyFinished(id.clone()), cx);
            assert!(s.terminal_unread(&id));
            s.select(id.clone(), cx);
            assert!(!s.terminal_unread(&id));
        });
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
                account_id: None,
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
                    account_id: None,
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
            let input = del_agent::UserInput {
                command: None,
                skills: Vec::new(),
                text: "first".into(),
                attachments: vec![del_agent::Attachment {
                    path: "/tmp/first.png".into(),
                    mime: Some("image/png".into()),
                }],
            };
            s.send_input(input.clone(), cx);
            assert_eq!(s.pending_input(), Some(&input));
            let id = del_agent::SessionId::new("ready");
            s.apply(super::Msg::Started(0, id.clone(), "first".into()), cx);
            assert_eq!(s.selection, Selection::Session(id.clone()));
            assert!(!s.is_starting());
            assert!(s.pending_prompts.is_empty());
            assert_eq!(s.take_first_prompt(&id), Some(input));
            assert_eq!(s.take_first_prompt(&id), None);
        });
    }
    #[gpui::test]
    fn account_lists_deduplicate_full_identity_without_losing_profiles(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, _| {
            s.default_identities.insert(Provider::Codex, "user@gmail.com".into());
            for (i, identity) in ["USER@gmail.com", "other@gmail.com", "other@gmail.com", "user@work.com", "API key", "API key"].into_iter().enumerate() {
                s.accounts.push(crate::accounts::Account { id: i.to_string(), provider: Provider::Codex, number: i + 2, identity: Some(identity.into()), is_default: false, requires_login: false });
            }
            assert_eq!(s.visible_accounts(Provider::Codex).iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec!["1", "3", "4", "5"]);
            assert_eq!(s.accounts.len(), 6); // Existing chats retain their credential profiles.
            assert!(s.visible_accounts(Provider::Claude).is_empty());
        });
    }

    #[gpui::test]
    fn adding_accounts_waits_for_login_to_finish(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let provider = s.effective().0;
            let id = uuid::Uuid::new_v4().to_string();
            s.accounts.push(crate::accounts::Account { id: id.clone(), provider, number: 2, identity: None, is_default: false, requires_login: false });
            s.account_status.insert(id.clone(), "Waiting for sign-in…".into());
            assert!(s.account_login_pending(provider));
            s.add_account(provider, cx);
            assert_eq!(s.accounts.len(), 1);
            s.login_account(&id, cx);
            assert_eq!(s.accounts.len(), 1);
            s.account_status.insert(id, "Sign-in cancelled".into());
            assert!(!s.account_login_pending(provider));
            assert_eq!(s.default_account_label(provider), "Account 1");
            s.default_identities.insert(provider, "user@example.com".into());
            assert_eq!(s.default_account_label(provider), "user");
        });
    }

    #[gpui::test]
    fn account_default_and_delete_actions_preserve_other_choices(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let provider = s.effective().0;
            let id = uuid::Uuid::new_v4().to_string();
            let account = crate::accounts::Account { id: id.clone(), provider, number: 2, identity: Some("user@example.com".into()), is_default: false, requires_login: false };
            s.accounts.push(account);
            assert!(!s.account_needs_login(&id));
            s.make_default_account(provider, Some(id.clone()), cx);
            let saved = serde_json::to_vec(&s.accounts).unwrap();
            let restored: Vec<crate::accounts::Account> = serde_json::from_slice(&saved).unwrap();
            assert!(restored[0].is_default);
            s.new_session(cx);
            assert_eq!(s.selected_account().as_deref(), Some(id.as_str()));
            s.account_status.insert(id.clone(), "Authentication expired".into());
            assert!(s.account_needs_login(&id));
            s.delete_account(&id, cx);
            assert!(s.accounts.is_empty());
            assert_eq!(s.default_account(provider), None);
            assert_eq!(s.selected_account(), None);
        });
    }

    #[gpui::test]
    fn provider_click_selects_only_its_default_account(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let provider = s.effective().0;
            let id = uuid::Uuid::new_v4().to_string();
            s.accounts.push(crate::accounts::Account { id: id.clone(), provider, number: 2, identity: None, is_default: true, requires_login: false });
            s.select_default_account(provider, cx);
            assert_eq!(s.selected_account().as_deref(), Some(id.as_str()));
            assert!(s.account_is_selected(provider, Some(&id)));
            assert!(!s.account_is_selected(provider, None));
            s.select_account(provider, None, cx);
            assert!(s.account_is_selected(provider, None));
            s.select_default_account(provider, cx);
            assert!(s.account_is_selected(provider, Some(&id)));
            assert_eq!(s.default_account_label(del_agent::Provider::OpenCode), "System");
        });
    }

    #[gpui::test]
    fn account_choices_isolate_models_and_allow_terminal_mode(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let provider = s.effective().0;
            let id = uuid::Uuid::new_v4().to_string();
            s.accounts.push(crate::accounts::Account { id: id.clone(), provider, number: 1, identity: None, is_default: false, requires_login: false });
            s.select_account(provider, Some(id.clone()), cx);
            assert_eq!(s.selected_account(), Some(id.clone()));
            assert!(s.account_is_selected(provider, Some(&id)));
            assert!(!s.account_is_selected(provider, None));
            let cwd = s.effective().2;
            s.apply(super::Msg::Models(provider, Some(id.clone()), cwd, Ok(vec![])), cx);
            assert!(s.model_catalog().loaded);
            s.set_mode(del_mcp::SessionMode::Terminal, cx);
            assert_eq!(s.mode(), del_mcp::SessionMode::Terminal);
            s.make_default_account(provider, Some(id.clone()), cx);
            s.new_session(cx);
            assert_eq!(s.selected_account(), Some(id));
            s.select_account(provider, None, cx);
            assert_eq!(s.selected_account(), None);
            assert!(s.account_is_selected(provider, None));
            assert!(!s.model_catalog().loaded);
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
                    None,
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
                    None,
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
    fn project_actions_preserve_directories_and_can_be_reopened(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().to_path_buf();
            std::fs::write(path.join("keep.txt"), "keep").unwrap();
            s.new_project_session(path.clone(), cx);
            s.terminal_rows.push(crate::sessions::TerminalRow { id: del_agent::SessionId::new("project-actions"), title: "Shell".into(), cwd: path.clone(),
                provider: None, account_id: None, active: false, summary: String::new(), pinned: false });
            s.rename_project(path.clone(), "Named project".into(), cx);
            assert_eq!(s.project_name(&path), "Named project");
            s.archive_project(path.clone(), false, cx);
            assert!(!s.project_paths().contains(&path));
            s.show_archived = true;
            assert_eq!(s.project_paths(), vec![path.clone()]);
            s.restore_project(path.clone(), cx);
            s.show_archived = false;
            assert!(s.project_paths().contains(&path));
            s.archive_project(path.clone(), true, cx);
            assert!(!s.project_paths().contains(&path));
            assert_eq!(std::fs::read_to_string(path.join("keep.txt")).unwrap(), "keep");
            s.new_project_session(path.clone(), cx);
            assert!(s.project_paths().contains(&path));
        });
    }

    #[gpui::test]
    fn new_session_follows_selected_context_instead_of_last_creation(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            s.new_project_session("/nihal".into(), cx);
            let selected = s.tabs.last().unwrap().clone();
            s.new_project_session("/del".into(), cx);
            s.select_tab(&selected, cx);
            s.new_session(cx);
            assert_eq!(s.effective().2, std::path::PathBuf::from("/nihal"));
            s.new_session(cx);
            assert_eq!(s.effective().2, std::path::PathBuf::from("/nihal"));
            let terminal = del_agent::SessionId::new("context-terminal");
            s.terminal_rows.push(crate::sessions::TerminalRow { id: terminal.clone(), title: "Shell".into(), cwd: "/terminal-dir".into(),
                provider: None, account_id: None, active: false, summary: String::new(), pinned: false });
            s.select_terminal(terminal, cx);
            s.new_session(cx);
            assert_eq!(s.effective().2, std::path::PathBuf::from("/terminal-dir"));
        });
    }

    #[gpui::test]
    fn repeated_new_sessions_are_independent_and_remember_project_directory(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            for _ in 0..100 { s.new_session(cx); }
            assert_eq!(s.tabs.len(), 101);
            s.new_project_session("/first".into(), cx);
            s.set_launch_options(Some("main".into()), true, cx);
            s.new_project_session("/second".into(), cx);
            assert_eq!(s.tabs.len(), 103);
            assert_eq!(s.effective().2, std::path::PathBuf::from("/second"));
            assert!(!s.draft_launch_options().worktree);
            s.new_session(cx);
            assert_eq!(s.effective().2, std::path::PathBuf::from("/second"));
            let Selection::Draft(submitted) = s.selection else { panic!("expected draft") };
            s.send("starting".into(), cx);
            s.new_session(cx);
            assert_eq!(s.pending_prompts.get(&submitted).unwrap().text, "starting");
            assert_eq!(s.tabs.len(), 105);
            s.new_session(cx);
            assert_eq!(s.tabs.len(), 106);
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
    fn unload_retains_terminal_history_and_metadata(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let id = del_agent::SessionId::new("unload-terminal");
            s.terminal_rows.push(crate::sessions::TerminalRow {
                account_id: None,
                id: id.clone(),
                title: "Saved conversation".into(),
                cwd: "/tmp".into(),
                provider: Some(del_agent::Provider::Claude),
                active: true,
                summary: "Saved output".into(),
                pinned: true,
            });
            s.overrides.rename(&id, "My name");
            s.select_terminal(id.clone(), cx);
            s.unload(id.clone(), cx);
            assert!(!s.tabs.contains(&Tab::Terminal(id.clone())));
            assert_eq!(s.closed_terminals, vec![id.clone()]);
            assert!(s.removed_terminals.is_empty());
            assert_eq!(s.terminal_rows[0].summary, "Saved output");
            assert!(s.terminal_rows[0].pinned);
            assert!(!s.terminal_rows[0].active);
            assert_eq!(s.overrides.title(&id), Some("My name"));
            assert!(!s.terminal_archived(&id));
            s.select_terminal(id.clone(), cx);
            assert_eq!(s.selection, Selection::Terminal(id));
        });
    }

    #[gpui::test]
    fn sidebar_reorder_keeps_pin_groups_and_selection(cx: &mut gpui::TestAppContext) {
        use del_agent::SessionId;
        let state = draft(cx);
        state.update(cx, |s, cx| {
            for name in ["a", "b", "c", "d"] {
                s.terminal_rows.push(crate::sessions::TerminalRow {
                    account_id: None,
                    id: SessionId::new(name), title: name.into(), cwd: "/tmp".into(),
                    provider: None, active: false, summary: String::new(), pinned: false,
                });
            }
            let a = SessionId::new("a"); let b = SessionId::new("b");
            let c = SessionId::new("c"); let d = SessionId::new("d");
            s.toggle_terminal_pin(&b, cx);
            s.toggle_terminal_pin(&a, cx);
            assert_eq!(s.terminal_rows[0].id, b);
            s.select_terminal(c.clone(), cx);
            let selection = s.selection.clone();
            s.reorder_sidebar(&a, &b, true, cx);
            assert_eq!(s.terminal_rows[0].id, a);
            s.reorder_sidebar(&d, &c, true, cx);
            assert_eq!(s.terminal_rows[2].id, d);
            s.reorder_sidebar(&d, &a, true, cx);
            s.reorder_sidebar(&a, &d, true, cx);
            // Mode no longer separates groups: reorder through either mode.
            s.reorder_sidebar(&b, &a, false, cx);
            assert_eq!(s.terminal_rows.iter().map(|r| &r.id).collect::<Vec<_>>(), vec![&b, &a, &d, &c]);
            assert_eq!(s.selection, selection);
            let saved = serde_json::to_string(&s.overrides).unwrap();
            s.overrides = serde_json::from_str(&saved).unwrap();
            s.terminal_rows.reverse();
            s.sort_terminal_rows();
            assert_eq!(s.terminal_rows.iter().map(|r| &r.id).collect::<Vec<_>>(), vec![&b, &a, &d, &c]);
        });
    }

    #[gpui::test]
    fn changing_draft_directory_moves_its_group_and_reopens_destination(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let home = std::path::PathBuf::from("/new-home");
            let old = s.effective().2;
            s.project_preferences.archived.insert(home.clone());
            s.project_preferences.collapsed.insert(home.clone());
            s.set_draft_directory(0, home.clone(), cx);
            assert_eq!(s.project_drafts(&home), vec![0]);
            assert!(s.project_drafts(&old).is_empty());
            assert_eq!(s.project_paths(), vec![home.clone()]);
            assert!(!s.project_preferences.collapsed.contains(&home));
            assert_eq!(s.project_preferences.last_directory, Some(home));
        });
    }

    #[gpui::test]
    fn active_session_in_archived_directory_is_visible_in_its_own_project(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, _| {
            let home = std::path::PathBuf::from("/chosen-home");
            s.project_preferences.archived.insert(home.clone());
            s.project_preferences.collapsed.insert(home.clone());
            s.terminal_rows.push(crate::sessions::TerminalRow { id: del_agent::SessionId::new("chosen-directory"),
                title: "Greeting".into(), cwd: home.clone(), provider: None, account_id: None,
                active: false, summary: String::new(), pinned: false });
            assert!(s.project_paths().contains(&home), "active session must reveal its own directory project despite old archive metadata");
        });
    }

    #[gpui::test]
    fn restoring_one_session_reopens_project_without_restoring_siblings(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let path = std::path::PathBuf::from("/archived-project-test");
            let first = del_agent::SessionId::new("restore-first");
            let second = del_agent::SessionId::new("restore-second");
            for id in [&first, &second] {
                s.terminal_rows.push(crate::sessions::TerminalRow { id: id.clone(), title: "Shell".into(), cwd: path.clone(),
                    provider: None, account_id: None, active: false, summary: String::new(), pinned: false });
            }
            s.project_preferences.roots.insert(path.clone());
            s.archive(first.clone(), cx);
            s.archive(second.clone(), cx);
            assert!(!s.project_paths().contains(&path), "all individually archived sessions hide their project");
            s.project_preferences.archived.insert(path.clone());
            s.show_archived = true;
            assert!(s.project_paths().contains(&path));
            s.restore(first.clone(), cx);
            assert!(!s.terminal_archived(&first));
            assert!(s.terminal_archived(&second));
            assert!(s.project_paths().contains(&path), "archived sibling keeps archive group visible");
            s.show_archived = false;
            assert!(s.project_paths().contains(&path), "restored session brings its project back");
            assert_eq!(s.unified_rows().iter().filter(|r| r.cwd == path).map(|r| r.id.clone()).collect::<Vec<_>>(), vec![first.clone()]);
            s.restore(second.clone(), cx);
            s.show_archived = true;
            assert!(!s.project_paths().contains(&path), "last restored session removes archive group");
            s.archive(second.clone(), cx);
            s.archive_project(path.clone(), true, cx);
            assert!(s.terminal_rows.iter().any(|r| r.id == first), "archived deletion preserves active sibling");
            assert!(!s.terminal_rows.iter().any(|r| r.id == second));
            assert!(!s.project_paths().contains(&path));
            s.show_archived = false;
            assert!(s.project_paths().contains(&path));
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
                    account_id: None,
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
                account_id: None,
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
    fn projects_mix_modes_preserve_pins_and_prevent_cross_project_reorder(cx: &mut gpui::TestAppContext) {
        use del_agent::{SessionId, SessionRecord, SessionStatus, PermissionMode};
        use crate::sessions::TerminalRow;
        use std::path::PathBuf;
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let chat = SessionId::new("chat");
            s.records.push(SessionRecord { id: chat.clone(), provider: Provider::Codex, account_id: None, cwd: "/repo/worktree".into(), model: None, reasoning_effort: None, service_tier: None, permission_mode: PermissionMode::FullAuto, title: Some("Chat".into()), title_turns: 0, created_at_ms: 1, updated_at_ms: 1, status: SessionStatus::Idle });
            for (name, cwd) in [("terminal", "/repo"), ("other", "/elsewhere")] {
                s.terminal_rows.push(TerminalRow { id: SessionId::new(name), provider: None, account_id: None, cwd: cwd.into(), title: name.into(), summary: String::new(), active: false, pinned: false });
            }
            s.project_info.insert("/repo/worktree".into(), crate::projects::GitInfo { root: "/repo".into(), ..Default::default() });
            let terminal = SessionId::new("terminal");
            s.reorder_sidebar(&terminal, &chat, true, cx);
            assert_eq!(s.unified_rows()[0].id, terminal);
            let order = s.overrides.clone();
            s.reorder_sidebar(&SessionId::new("other"), &chat, true, cx);
            assert_eq!(s.overrides, order);
            s.toggle_session_pin(&chat, cx);
            s.toggle_session_pin(&terminal, cx);
            let pins: Vec<_> = s.unified_rows().into_iter().filter(|r| r.pinned).map(|r| r.id).collect();
            assert_eq!(pins, vec![chat.clone(), terminal.clone()]);
            s.reorder_sidebar(&terminal, &chat, true, cx);
            assert_eq!(s.unified_rows()[0].id, terminal);
            let saved = serde_json::to_string(&s.overrides).unwrap();
            s.overrides = serde_json::from_str(&saved).unwrap();
            assert_eq!(s.unified_rows()[0].id, terminal);
            s.new_project_session("/repo".into(), cx);
            assert_eq!(s.effective().2, PathBuf::from("/repo"));
            assert_eq!(s.project_drafts(std::path::Path::new("/repo")).len(), 1);
            s.toggle_project("/repo".into(), cx);
            assert!(s.project_preferences.collapsed.contains(std::path::Path::new("/repo")));
            assert_eq!(s.records.len(), 1);
        });
    }

    #[gpui::test]
    fn archiving_last_project_leaves_only_an_empty_composer(cx: &mut gpui::TestAppContext) {
        let state = draft(cx);
        state.update(cx, |s, cx| {
            let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
            s.set_draft_directory(0, home.clone(), cx);
            s.archive_project(home, false, cx);
            assert!(s.tabs.is_empty());
            assert!(s.project_paths().is_empty());
            assert!(s.draft().is_some(), "empty page must remain usable");
            s.new_session(cx);
            assert_eq!(s.tabs.len(), 1);
            assert_eq!(s.project_paths().len(), 1);
        });
    }

    #[gpui::test]
    fn project_sidebar_renders_with_drafts_and_terminal_sessions(cx: &mut gpui::TestAppContext) {
        use gpui::{ParentElement, Styled, Render, IntoElement};
        struct Fixture {
            state: Entity<AppState>,
            rename: Entity<gpui_kit::component::input::InputState>,
            usage: Entity<crate::usage::Usage>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
                gpui::div().w(gpui::px(240.)).h(gpui::px(700.))
                    .child(crate::sidebar::render(&self.state, &self.rename, &self.usage, cx))
            }
        }
        cx.update(gpui_kit::init);
        let state = draft(cx);
        state.update(cx, |s, _| {
            let cwd = s.effective().2;
            s.terminal_rows.push(crate::sessions::TerminalRow { id: del_agent::SessionId::new("terminal-render"), provider: None, account_id: None, cwd, title: "Terminal session".into(), summary: String::new(), active: false, pinned: false });
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            let rename = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
            let usage = cx.new(|_| crate::usage::Usage::new(state.clone()));
            Fixture { state: state.clone(), rename, usage }
        });
        cx.update(|window, cx| { window.draw(cx).clear(cx); });
        assert!(cx.debug_bounds("pinned-heading").is_none());
        let projects = cx.debug_bounds("projects-heading").unwrap();

        assert!(projects.size.width > gpui::px(100.));
        let content = cx.debug_bounds("project-session-content").unwrap();
        assert_eq!(content.size.width, projects.size.width, "animated project rows must fill the sidebar width");
        cx.update(|window, cx| {
            root.update(cx, |fixture, cx| { fixture.state.update(cx, |s, cx| { let path = s.effective().2; s.toggle_project(path, cx); }); cx.notify(); });
            window.draw(cx).clear(cx);
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
