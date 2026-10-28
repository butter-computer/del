//! Main-app terminal launcher and running-session host.
//!
//! The parent subscribes to `TerminalLauncherEvent::Launch`, registers the
//! session with MCP, and constructs `TerminalHost::new_prepared`. Persist only
//! the original `TerminalLaunchSpec`; injected MCP args/env never enter it.
//! Keep MCP launch resources in the parent until exit or explicit close.
//! Use `TerminalHost::spawn_prepared` to create an entity without panicking on
//! invalid launch settings or PTY spawn errors.

use crate::zoom::px as zpx;
use del_terminal::{TerminalEvent, TerminalSession, TerminalSpec, TerminalView};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLaunchKind {
    Shell,
    Codex,
    Claude,
    #[serde(rename = "opencode")]
    OpenCode,
    Custom,
}

impl TerminalLaunchKind {
    pub const ALL: [Self; 5] = [
        Self::Shell,
        Self::Codex,
        Self::Claude,
        Self::OpenCode,
        Self::Custom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::OpenCode => "OpenCode",
            Self::Custom => "Custom",
        }
    }
}

/// History-safe command description. Never append MCP config to these fields
/// before persisting it; pass that configuration to `new_prepared` instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLaunchSpec {
    #[serde(default)]
    pub account_id: Option<String>,
    pub kind: TerminalLaunchKind,
    pub cwd: PathBuf,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub title: String,
    pub login_env: bool,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl TerminalLaunchSpec {
    pub fn provider(&self) -> Option<del_agent::Provider> {
        match self.kind {
            TerminalLaunchKind::Codex => Some(del_agent::Provider::Codex),
            TerminalLaunchKind::Claude => Some(del_agent::Provider::Claude),
            TerminalLaunchKind::OpenCode => Some(del_agent::Provider::OpenCode),
            _ => None,
        }
    }
    pub fn account(&self) -> io::Result<Option<crate::accounts::Account>> {
        let Some(id) = &self.account_id else {
            return Ok(None);
        };
        crate::accounts::load()
            .map_err(io::Error::other)?
            .into_iter()
            .find(|a| &a.id == id && Some(a.provider) == self.provider())
            .map(Some)
            .ok_or_else(|| io::Error::other("This terminal's saved account is unavailable"))
    }
    pub fn codex_root(&self) -> PathBuf {
        self.account()
            .ok()
            .flatten()
            .map(|a| a.root())
            .unwrap_or_else(crate::terminal_conversation::codex_root)
    }
    pub fn label(&self) -> String {
        self.title.clone()
    }

    pub fn shell(cwd: impl Into<PathBuf>) -> Self {
        Self {
            account_id: None,
            kind: TerminalLaunchKind::Shell,
            cwd: cwd.into(),
            program: None,
            args: Vec::new(),
            title: "Shell".into(),
            login_env: true,
            variant: None,
            model: None,
        }
    }
    pub fn agent(kind: TerminalLaunchKind, cwd: impl Into<PathBuf>) -> io::Result<Self> {
        let name = match kind {
            TerminalLaunchKind::Codex => "codex",
            TerminalLaunchKind::Claude => "claude",
            TerminalLaunchKind::OpenCode => "opencode",
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "choose Codex, Claude, or OpenCode",
                ));
            }
        };
        let path = del_agent::detect::find_binary(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not installed", kind.label()),
            )
        })?;
        Ok(Self {
            account_id: None,
            kind,
            cwd: cwd.into(),
            program: Some(path.to_string_lossy().into_owned()),
            args: Vec::new(),
            title: kind.label().into(),
            login_env: true,
            variant: None,
            model: None,
        })
    }
    pub fn from_config(config: &del_agent::SessionConfig) -> io::Result<Self> {
        let kind = match config.provider {
            del_agent::Provider::Codex => TerminalLaunchKind::Codex,
            del_agent::Provider::Claude => TerminalLaunchKind::Claude,
            del_agent::Provider::OpenCode => TerminalLaunchKind::OpenCode,
            del_agent::Provider::Acp => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Choose Codex, Claude, or OpenCode to start a terminal session",
                ));
            }
        };
        let mut spec = Self::agent(kind, config.cwd.clone())?;
        spec.apply_model_settings(config);
        if kind == TerminalLaunchKind::OpenCode {
            spec.variant = config.reasoning_effort.clone();
            spec.model = config.model.clone();
        }
        Ok(spec)
    }

    fn apply_model_settings(&mut self, config: &del_agent::SessionConfig) {
        if self.kind == TerminalLaunchKind::Claude {
            self.args.extend([
                "--settings".into(),
                serde_json::json!({"fastMode": config.service_tier.as_deref() == Some("fast")})
                    .to_string(),
            ]);
        }
        if let Some(model) = &config.model
            && self.kind != TerminalLaunchKind::OpenCode
        {
            self.args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &config.reasoning_effort {
            match self.kind {
                TerminalLaunchKind::Codex => self.args.extend([
                    "-c".into(),
                    format!(
                        "model_reasoning_effort={}",
                        serde_json::to_string(effort).unwrap()
                    ),
                ]),
                TerminalLaunchKind::Claude => self.args.extend(["--effort".into(), effort.clone()]),
                _ => {}
            }
        }
        if self.kind == TerminalLaunchKind::Codex
            && let Some(tier) = &config.service_tier
        {
            self.args.extend([
                "-c".into(),
                format!("service_tier={}", serde_json::to_string(tier).unwrap()),
            ]);
        }
    }

    /// Run the user's explicit command as one shell command, preserving its
    /// quoting and pipelines. This is separate from the agent argv interface.
    pub fn custom(command: impl Into<String>, cwd: impl Into<PathBuf>) -> io::Result<Self> {
        let command = command.into();
        if command.trim().is_empty() || command.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "enter a command",
            ));
        }
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        Ok(Self {
            account_id: None,
            kind: TerminalLaunchKind::Custom,
            cwd: cwd.into(),
            program: Some(shell),
            args: vec!["-lc".into(), command],
            title: "Custom terminal".into(),
            login_env: false,
            variant: None,
            model: None,
        })
    }

    fn isolate_account(&self, account: &crate::accounts::Account) -> io::Result<Self> {
        // Apply profile variables after the login shell, so shell startup
        // cannot redirect authentication to another account.
        let mut profile_args = Vec::new();
        for key in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "OPENAI_ACCESS_TOKEN",
            "OPENAI_IDENTITY_TOKEN_FILE",
            "OPENAI_IDENTITY_PROVIDER_ID",
        ] {
            profile_args.extend(["-u".into(), key.into()]);
        }
        for (key, value) in account.environment() {
            profile_args.push(format!("{key}={value}"));
        }
        profile_args.push(
            self.program
                .clone()
                .ok_or_else(|| io::Error::other("Missing provider command"))?,
        );
        let mut isolated = self.clone();
        isolated.account_id = None;
        isolated.program = Some("/usr/bin/env".into());
        profile_args.extend(self.args.clone());
        if self.kind == TerminalLaunchKind::Codex {
            profile_args.extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
        }
        isolated.args = profile_args;
        Ok(isolated)
    }

    fn prepare(
        &self,
        extra_args: Vec<String>,
        mut env: Vec<(String, String)>,
    ) -> io::Result<TerminalSpec> {
        if self.kind != TerminalLaunchKind::Shell && self.program.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal command is missing",
            ));
        }
        if !self.cwd.is_absolute() || !self.cwd.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal workspace must be an existing absolute directory",
            ));
        }
        if self
            .program
            .as_ref()
            .is_some_and(|p| p.trim().is_empty() || p.contains('\0'))
            || self
                .args
                .iter()
                .chain(extra_args.iter())
                .any(|a| a.contains('\0'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid terminal command",
            ));
        }
        if self.program.is_none() && (!self.args.is_empty() || !extra_args.is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell launches cannot have agent arguments",
            ));
        }
        let mut args = self.args.clone();
        if let Some(account) = self.account()? {
            return self.isolate_account(&account)?.prepare(extra_args, env);
        }
        let bypass = match self.kind {
            TerminalLaunchKind::Codex => Some("--dangerously-bypass-approvals-and-sandbox"),
            TerminalLaunchKind::Claude => Some("--dangerously-skip-permissions"),
            TerminalLaunchKind::OpenCode => Some("--auto"),
            _ => None,
        };
        if let Some(flag) = bypass
            && !args.iter().any(|arg| arg == flag)
        {
            args.push(flag.into());
        }
        let cwd = self.cwd.canonicalize()?;
        if self.kind == TerminalLaunchKind::Codex {
            // Keep conversation output in the emulator's searchable scrollback.
            if !args.iter().any(|arg| arg == "--no-alt-screen") {
                args.push("--no-alt-screen".into());
            }
            // Login shell startup files may change directories. Pin the CLI's
            // workspace independently of that shell's final working directory.
            args.extend(["--cd".into(), cwd.to_string_lossy().into_owned()]);
            // Scope startup folder trust to the workspace opened in Del.
            args.extend([
                "-c".into(),
                format!(
                    "projects.{}.trust_level=\"trusted\"",
                    serde_json::to_string(&cwd.to_string_lossy()).unwrap()
                ),
            ]);
            // Use the provider's conversation name, without the folder/status
            // decorations or a UUID while the name is still being generated.
            args.extend([
                "-c".into(),
                r#"tui.terminal_title=["session-id","thread-name"]"#.into(),
            ]);
        }
        if self.kind == TerminalLaunchKind::OpenCode {
            // Del owns conversation tabs. OpenCode's shared tab strip otherwise
            // restores unrelated sessions even with an explicit --session ID.
            const CLI_CONFIG: &str = "OPENCODE_CLI_CONFIG_CONTENT";
            let existing = env
                .iter()
                .rev()
                .find(|(key, _)| key == CLI_CONFIG)
                .map(|(_, value)| value.clone())
                .or_else(|| std::env::var(CLI_CONFIG).ok());
            let mut config: serde_json::Value = existing
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
                .unwrap_or_else(|| serde_json::json!({}));
            if !config.is_object() || config.get("tabs").is_some_and(|tabs| !tabs.is_object()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Invalid OpenCode CLI config",
                ));
            }
            config["tabs"]["mode"] = "off".into();
            env.retain(|(key, _)| key != CLI_CONFIG);
            env.push((CLI_CONFIG.into(), config.to_string()));
            // Private server consumes this launch's MCP config; the shared
            // background service cannot inherit per-terminal credentials.
            args.push("--standalone".into());
            args.push(cwd.to_string_lossy().into_owned());
        }
        args.extend(extra_args);
        Ok(TerminalSpec {
            program: self.program.clone(),
            args,
            env,
            env_remove: del_agent::environment::WORKER_SESSION_ENV
                .iter()
                .map(|key| (*key).to_owned())
                .collect(),
            cwd: Some(cwd),
            login_env: self.login_env,
            ..TerminalSpec::default()
        })
    }
}

#[derive(Clone, Debug)]
pub enum TerminalLauncherEvent {
    Launch(TerminalLaunchSpec),
}

pub struct TerminalLauncher {
    cwd: PathBuf,
    kind: TerminalLaunchKind,
    custom: Entity<InputState>,
    focus: FocusHandle,
    launching: bool,
    error: Option<String>,
}
impl EventEmitter<TerminalLauncherEvent> for TerminalLauncher {}
impl Focusable for TerminalLauncher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl TerminalLauncher {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let custom = cx
            .new(|cx| InputState::new(window, cx).placeholder("Command, for example: npm run dev"));
        cx.subscribe_in(&custom, window, |this, _, event, _, cx| match event {
            InputEvent::PressEnter { .. } if this.kind == TerminalLaunchKind::Custom => {
                this.request_launch(cx)
            }
            InputEvent::Change => {
                this.error = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        Self {
            cwd,
            kind: TerminalLaunchKind::Shell,
            custom,
            focus: cx.focus_handle(),
            launching: false,
            error: None,
        }
    }
    pub fn set_workspace(&mut self, cwd: PathBuf, cx: &mut Context<Self>) {
        self.cwd = cwd;
        self.error = None;
        cx.notify();
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.kind == TerminalLaunchKind::Custom {
            self.custom.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }
    /// Reset the launch button when the parent's asynchronous preparation fails.
    pub fn launch_failed(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.launching = false;
        self.error = Some(message.into());
        cx.notify();
    }
    fn request_launch(&mut self, cx: &mut Context<Self>) {
        if self.launching {
            return;
        }
        let spec = match self.kind {
            TerminalLaunchKind::Shell => Ok(TerminalLaunchSpec::shell(self.cwd.clone())),
            TerminalLaunchKind::Codex
            | TerminalLaunchKind::Claude
            | TerminalLaunchKind::OpenCode => {
                TerminalLaunchSpec::agent(self.kind, self.cwd.clone())
            }
            TerminalLaunchKind::Custom => TerminalLaunchSpec::custom(
                self.custom.read(cx).value().to_string(),
                self.cwd.clone(),
            ),
        }
        .and_then(|spec| {
            spec.prepare(Vec::new(), Vec::new())?;
            Ok(spec)
        });
        match spec {
            Ok(spec) => {
                self.launching = true;
                self.error = None;
                cx.emit(TerminalLauncherEvent::Launch(spec));
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }
}
impl Render for TerminalLauncher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut choices = h_flex().gap_1().flex_wrap();
        for kind in TerminalLaunchKind::ALL {
            let installed = match kind {
                TerminalLaunchKind::Codex => del_agent::detect::find_binary("codex").is_some(),
                TerminalLaunchKind::Claude => del_agent::detect::find_binary("claude").is_some(),
                _ => true,
            };
            let selected = kind == self.kind;
            choices = choices.child(
                Button::new(("terminal-kind", kind as usize))
                    .ghost()
                    .small()
                    .label(kind.label())
                    .disabled(self.launching || !installed)
                    .when(selected, |b| b.bg(rgba(0xffffff18)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.kind = kind;
                        this.error = None;
                        this.focus(window, cx);
                        cx.notify();
                    })),
            );
        }
        let blank = self.kind == TerminalLaunchKind::Custom
            && self.custom.read(cx).value().trim().is_empty();
        div()
            .id("terminal-launcher")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .child(
                v_flex()
                    .w_full()
                    .max_w(zpx(560.))
                    .gap_3()
                    .child(
                        div()
                            .text_size(zpx(20.))
                            .text_color(rgb(0xf5f5f5))
                            .child("Open a terminal"),
                    )
                    .child(
                        div()
                            .text_size(zpx(12.))
                            .text_color(rgb(0x999999))
                            .child(self.cwd.to_string_lossy().into_owned()),
                    )
                    .child(choices)
                    .when(self.kind == TerminalLaunchKind::Custom, |d| {
                        d.child(
                            Input::new(&self.custom)
                                .appearance(false)
                                .bordered(false)
                                .disabled(self.launching),
                        )
                    })
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .text_size(zpx(12.))
                                .text_color(rgb(0xe58b8b))
                                .child(error),
                        )
                    })
                    .child(
                        Button::new("launch-terminal")
                            .ghost()
                            .small()
                            .label(if self.launching {
                                "Opening…"
                            } else {
                                "Open terminal"
                            })
                            .disabled(self.launching || blank)
                            .on_click(cx.listener(|this, _, _, cx| this.request_launch(cx))),
                    ),
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalHostStatus {
    Running,
    Exited { code: Option<i32> },
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalHostSnapshot {
    pub kind: TerminalLaunchKind,
    pub cwd: PathBuf,
    pub title: String,
    pub status: TerminalHostStatus,
    pub pid: u32,
    pub summary: Option<String>,
    pub last_output: String,
    pub conversation_hint: Option<String>,
    pub title_loading: bool,
    title_pending_since: Option<std::time::Instant>,
}

impl TerminalHostSnapshot {
    fn update_title(&mut self, title: Option<&str>, fallback: &str) {
        let title = title.map(|title| {
            if self.kind == TerminalLaunchKind::Claude {
                crate::terminal_conversation::claude_osc_title(title)
            } else {
                title
            }
        });
        if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
            if crate::terminal_conversation::title_is_pending(title) {
                if self.title_pending_since.is_none() {
                    self.title_pending_since = Some(std::time::Instant::now());
                    self.title_loading = true;
                }
                return;
            }
            if self.kind == TerminalLaunchKind::Codex
                && let Some((identity, name)) = crate::terminal_conversation::codex_title(title)
            {
                self.conversation_hint = Some(identity);
                if let Some(name) = name {
                    self.update_title(Some(&name), fallback);
                } else {
                    self.title_pending_since = None;
                    self.title_loading = false;
                }
                return;
            }
            self.title_pending_since = None;
            self.title_loading = false;
            self.title = title.to_owned();
        } else if !matches!(
            self.kind,
            TerminalLaunchKind::Codex | TerminalLaunchKind::Claude
        ) {
            self.title = fallback.to_owned();
        }
        // Agent TUIs clear OSC titles on exit; keep their last conversation name.
    }
}

#[derive(Clone, Debug)]
pub enum TerminalHostEvent {
    Changed(TerminalHostSnapshot),
    Terminal(TerminalEvent),
}

pub struct TerminalHost {
    terminal: Entity<TerminalView>,
    snapshot: TerminalHostSnapshot,
    fallback_title: String,
    /// A trailing screen re-read is scheduled (output/keystroke bursts).
    output_refresh_pending: bool,
}

/// Output and keystrokes re-read the screen at most this often.
const OUTPUT_REFRESH: std::time::Duration = std::time::Duration::from_millis(250);
impl EventEmitter<TerminalHostEvent> for TerminalHost {}
impl Focusable for TerminalHost {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal.read(cx).focus_handle(cx)
    }
}
impl TerminalHost {
    pub fn terminal_view(&self) -> Entity<TerminalView> {
        self.terminal.clone()
    }

    pub fn spawn(
        spec: TerminalLaunchSpec,
        window: &mut Window,
        cx: &mut App,
    ) -> io::Result<Entity<Self>> {
        Self::spawn_prepared(spec, Vec::new(), Vec::new(), window, cx)
    }

    /// Complete all fallible work before creating the GPUI entity.
    pub fn spawn_prepared(
        spec: TerminalLaunchSpec,
        extra_args: Vec<String>,
        env: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut App,
    ) -> io::Result<Entity<Self>> {
        let (session, cwd) = spawn_session(&spec, extra_args, env)?;
        Ok(cx.new(|cx| Self::from_session(spec, cwd, session, window, cx)))
    }

    pub fn new(
        spec: TerminalLaunchSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<Self> {
        Self::new_prepared(spec, Vec::new(), Vec::new(), window, cx)
    }
    /// Inject bridge args/env without mutating the history-safe launch spec.
    /// The parent explicitly revokes MCP credentials and deletes config files.
    pub fn new_prepared(
        spec: TerminalLaunchSpec,
        extra_args: Vec<String>,
        env: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<Self> {
        let (session, cwd) = spawn_session(&spec, extra_args, env)?;
        Ok(Self::from_session(spec, cwd, session, window, cx))
    }

    fn from_session(
        spec: TerminalLaunchSpec,
        cwd: PathBuf,
        session: TerminalSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pid = session.pid();
        let terminal = cx.new(|cx| TerminalView::new(session, window, cx));
        cx.subscribe(&terminal, |this, _, event: &TerminalEvent, cx| {
            cx.emit(TerminalHostEvent::Terminal(event.clone()));
            let before = this.snapshot.clone();
            match event {
                TerminalEvent::TitleChanged(title) => {
                    this.snapshot
                        .update_title(title.as_deref(), &this.fallback_title);
                }
                TerminalEvent::Exited { code } => {
                    if this.snapshot.status != TerminalHostStatus::Closed {
                        this.snapshot.status = TerminalHostStatus::Exited { code: *code };
                    }
                }
                // Output and keystrokes arrive in bursts: walking the grid
                // and republishing on each would run per byte, so coalesce.
                _ => {
                    this.schedule_output_refresh(cx);
                    return;
                }
            }
            // Exit consumers read the final screen; refresh it now.
            this.refresh_output(before, cx);
        })
        .detach();
        if matches!(
            spec.kind,
            TerminalLaunchKind::Claude | TerminalLaunchKind::Codex
        ) {
            // Confirm only the exact startup trust dialog for the selected
            // workspace. Leave tool approvals and custom terminals alone.
            let kind = spec.kind;
            let workspace = cwd.clone();
            cx.spawn(async move |this, cx| {
                let started = std::time::Instant::now();
                let mut last_attempt = None;
                while started.elapsed() < std::time::Duration::from_secs(120) {
                    cx.background_executor()
                        .timer(trust_poll_interval(started.elapsed()))
                        .await;
                    let keep_waiting = this.update(cx, |this, cx| {
                        let session = this.terminal.read(cx).session();
                        if session.exited() || this.snapshot.status != TerminalHostStatus::Running {
                            return false;
                        }
                        let screen = screen_tail(session);
                        let key = match kind {
                            TerminalLaunchKind::Codex => {
                                codex_workspace_trust_key(&screen, &workspace)
                            }
                            _ => claude_workspace_trust_key(&screen, &workspace),
                        };
                        let Some(key) = key else {
                            // Once the recognized dialog disappears, stop.
                            return last_attempt.is_none();
                        };
                        // Respond immediately. Some CLI versions briefly ignore
                        // input while mounting; retry only while this exact
                        // workspace dialog is still on screen.
                        let now = std::time::Instant::now();
                        if !trust_confirmation_due(last_attempt, now) {
                            return true;
                        }
                        session.write(key.to_vec());
                        last_attempt = Some(now);
                        true
                    });
                    if !matches!(keep_waiting, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
        let fallback_title = spec.title.clone();
        Self {
            terminal,
            fallback_title,
            output_refresh_pending: false,
            snapshot: TerminalHostSnapshot {
                kind: spec.kind,
                cwd,
                title: spec.title,
                status: TerminalHostStatus::Running,
                pid,
                summary: None,
                last_output: String::new(),
                conversation_hint: None,
                title_loading: false,
                title_pending_since: None,
            },
        }
    }
    fn schedule_output_refresh(&mut self, cx: &mut Context<Self>) {
        if self.output_refresh_pending {
            return;
        }
        self.output_refresh_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUTPUT_REFRESH).await;
            let _ = this.update(cx, |this, cx| {
                let before = this.snapshot.clone();
                this.refresh_output(before, cx);
            });
        })
        .detach();
    }

    /// Re-read the screen tail; publish only when the snapshot changed.
    fn refresh_output(&mut self, before: TerminalHostSnapshot, cx: &mut Context<Self>) {
        self.output_refresh_pending = false;
        let output = screen_tail(self.terminal.read(cx).session());
        self.snapshot.summary = output
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(str::to_owned);
        self.snapshot.last_output = output;
        if self.snapshot != before {
            cx.emit(TerminalHostEvent::Changed(self.snapshot.clone()));
            cx.notify();
        }
    }

    /// Metadata reads never resume a thread or send a prompt. A completed lookup
    /// also ends the loading animation if the TUI leaves its progress title set.
    pub fn refresh_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        let before = self.snapshot.clone();
        if let Some(title) = title
            .filter(|t| !t.trim().is_empty() && !crate::terminal_conversation::title_is_pending(t))
        {
            self.snapshot.title = title;
            self.snapshot.title_loading = false;
        }
        if self
            .snapshot
            .title_pending_since
            .is_some_and(|start| start.elapsed().as_secs() >= 30)
        {
            self.snapshot.title_loading = false;
        }
        if self.snapshot != before {
            cx.emit(TerminalHostEvent::Changed(self.snapshot.clone()));
            cx.notify();
        }
    }

    pub fn snapshot(&self) -> TerminalHostSnapshot {
        self.snapshot.clone()
    }
    pub fn status(&self) -> &TerminalHostStatus {
        &self.snapshot.status
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.terminal
            .update(cx, |terminal, cx| terminal.focus(window, cx));
    }
    /// Send text through bracketed paste when enabled; optionally submit with CR.
    pub fn send_text(
        &mut self,
        text: &str,
        submit: bool,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let terminal = self.terminal.read(cx);
        let session = terminal.session();
        if self.snapshot.status != TerminalHostStatus::Running || session.exited() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal has exited",
            ));
        }
        let mut bytes = del_terminal::keys::encode_paste(text, session.mode());
        if submit {
            bytes.push(b'\r');
        }
        session.write(bytes);
        Ok(())
    }
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.status == TerminalHostStatus::Closed {
            return;
        }
        self.terminal.read(cx).close();
        self.snapshot.status = TerminalHostStatus::Closed;
        cx.emit(TerminalHostEvent::Changed(self.snapshot.clone()));
        cx.notify();
    }
}
impl Render for TerminalHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(self.terminal.clone())
    }
}

fn spawn_session(
    spec: &TerminalLaunchSpec,
    extra_args: Vec<String>,
    env: Vec<(String, String)>,
) -> io::Result<(TerminalSession, PathBuf)> {
    let prepared = spec.prepare(extra_args, env)?;
    let cwd = prepared
        .cwd
        .clone()
        .ok_or_else(|| io::Error::other("terminal workspace missing"))?;
    Ok((TerminalSession::spawn(prepared)?, cwd))
}

fn screen_tail(session: &TerminalSession) -> String {
    session.with_term(|term| {
        let mut lines = Vec::new();
        let mut current = String::new();
        let mut row = None;
        for cell in term.grid().display_iter() {
            if row.is_some_and(|r| r != cell.point.line.0) {
                lines.push(current.trim_end().to_string());
                current.clear();
            }
            row = Some(cell.point.line.0);
            current.push(cell.c);
            if let Some(zero) = cell.zerowidth() {
                current.extend(zero);
            }
        }
        if !current.is_empty() {
            lines.push(current.trim_end().to_string());
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        let text = lines[lines.len().saturating_sub(40)..].join("\n");
        text.chars()
            .rev()
            .take(8000)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    })
}

/// Poll quickly while the CLI is mounting, then back off.
fn trust_poll_interval(elapsed: std::time::Duration) -> std::time::Duration {
    std::time::Duration::from_millis(if elapsed.as_secs() < 5 {
        100
    } else if elapsed.as_secs() < 20 {
        250
    } else {
        1000
    })
}

fn trust_confirmation_due(
    last_attempt: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    last_attempt
        .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(250))
}

fn codex_workspace_trust_key(screen: &str, cwd: &std::path::Path) -> Option<&'static [u8]> {
    let lines: Vec<_> = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let heading = lines.iter().position(|line| *line == "Folder access")?;
    if lines.get(heading + 1).copied()? != cwd.to_string_lossy()
        || !lines
            .iter()
            .any(|line| line.starts_with("Trust this folder?"))
        || !lines.contains(&"2. Quit")
        || !lines.last()?.starts_with("enter")
        || !lines.last()?.contains("continue")
        || !lines.last()?.contains("esc")
        || !lines.last()?.contains("quit")
    {
        return None;
    }
    lines
        .iter()
        .any(|line| {
            line.strip_prefix('>')
                .or_else(|| line.strip_prefix('›'))
                .is_some_and(|selected| selected.trim() == "1. Trust and continue")
        })
        .then_some(b"\r".as_slice())
}

fn claude_workspace_trust_key(screen: &str, cwd: &std::path::Path) -> Option<&'static [u8]> {
    let lines: Vec<_> = screen.lines().map(str::trim).collect();
    if !lines.contains(&"Accessing workspace:")
        || !lines.contains(&cwd.to_string_lossy().as_ref())
        || !lines.last()?.starts_with("Enter to confirm")
        || !lines.last()?.contains("Esc to cancel")
    {
        return None;
    }
    let selected = lines.iter().find_map(|line| {
        line.strip_prefix('❯')
            .or_else(|| line.strip_prefix('>'))
            .map(str::trim)
    })?;
    match selected {
        "Yes, I trust this folder" => Some(b"\r"),
        "No, exit" | "No, continue without these permissions"
            if lines.contains(&"Yes, I trust this folder") =>
        {
            Some(b"\x1b[B")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalLaunchKind, TerminalLaunchSpec};
    use std::path::PathBuf;

    #[test]
    fn saved_terminal_account_survives_restore_and_isolates_authentication() {
        for (provider, kind, key) in [
            (
                del_agent::Provider::Codex,
                TerminalLaunchKind::Codex,
                "CODEX_HOME",
            ),
            (
                del_agent::Provider::Claude,
                TerminalLaunchKind::Claude,
                "CLAUDE_CONFIG_DIR",
            ),
            (
                del_agent::Provider::OpenCode,
                TerminalLaunchKind::OpenCode,
                "OPENCODE_DB",
            ),
        ] {
            let account = crate::accounts::Account {
                id: uuid::Uuid::new_v4().to_string(),
                provider,
                number: 2,
                identity: None,
                is_default: false,
                requires_login: false,
            };
            let mut spec = TerminalLaunchSpec::shell(std::env::temp_dir());
            spec.kind = kind;
            spec.program = Some("/bin/echo".into());
            spec.account_id = Some(account.id.clone());
            let restored: TerminalLaunchSpec =
                serde_json::from_slice(&serde_json::to_vec(&spec).unwrap()).unwrap();
            assert_eq!(restored.account_id, spec.account_id);
            let prepared = restored
                .isolate_account(&account)
                .unwrap()
                .prepare(vec![], vec![])
                .unwrap();
            assert_eq!(prepared.program.as_deref(), Some("/usr/bin/env"));
            let binary = prepared.args.iter().position(|a| a == "/bin/echo").unwrap();
            assert!(
                prepared.args[..binary]
                    .iter()
                    .any(|a| a.starts_with(&format!("{key}=")) && a.contains(&account.id))
            );
            assert!(
                prepared.args[..binary]
                    .windows(2)
                    .any(|a| a == ["-u", "OPENAI_API_KEY"])
            );
            assert!(
                prepared.args[..binary]
                    .windows(2)
                    .any(|a| a == ["-u", "CLAUDE_CODE_OAUTH_TOKEN"])
            );
            if provider == del_agent::Provider::Codex {
                assert!(
                    prepared
                        .args
                        .contains(&"cli_auth_credentials_store=\"file\"".into())
                );
            }
        }
    }

    #[test]
    fn codex_confirms_only_the_selected_workspace_startup_prompt() {
        let cwd = std::path::Path::new("/work/project");
        let screen = "Folder access\n/work/project\n\nTrust this folder? Codex can read, edit, and run files here.\nContinue only if you trust these files. Your trust decision will be saved.\n› 1. Trust and continue\n  2. Quit\n\n  enter continue · esc quit\n";
        assert_eq!(
            super::codex_workspace_trust_key(screen, cwd),
            Some(b"\r".as_slice())
        );
        for unrelated in [
            screen.replace("/work/project", "/"),
            screen.replace("Folder access", "Run command"),
            screen.replace("› 1. Trust and continue", "1. Trust and continue"),
            screen.replace("› 1. Trust and continue", "› 1. Approve command"),
            format!("{screen}A later prompt"),
        ] {
            assert_eq!(super::codex_workspace_trust_key(&unrelated, cwd), None);
        }
    }

    #[test]
    fn trust_confirmation_is_immediate_with_short_bounded_retries() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        assert!(super::trust_confirmation_due(None, now));
        assert!(!super::trust_confirmation_due(
            Some(now),
            now + Duration::from_millis(100)
        ));
        assert!(super::trust_confirmation_due(
            Some(now),
            now + Duration::from_millis(250)
        ));
    }

    #[test]
    fn trust_polling_backs_off_after_startup() {
        use std::time::Duration;
        assert_eq!(
            super::trust_poll_interval(Duration::ZERO),
            Duration::from_millis(100)
        );
        assert_eq!(
            super::trust_poll_interval(Duration::from_secs(4)),
            Duration::from_millis(100)
        );
        assert_eq!(
            super::trust_poll_interval(Duration::from_secs(6)),
            Duration::from_millis(250)
        );
        assert_eq!(
            super::trust_poll_interval(Duration::from_secs(30)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn claude_only_confirms_the_selected_workspace_trust_dialog() {
        let cwd = std::path::Path::new("/work/project");
        let screen = "Accessing workspace:\n/work/project\nNo, exit\n❯ Yes, I trust this folder\nEnter to confirm · Esc to cancel";
        assert_eq!(
            super::claude_workspace_trust_key(screen, cwd),
            Some(b"\r".as_slice())
        );
        let cancel_selected = screen.replace("No, exit\n❯ Yes", "❯ No, exit\nYes");
        assert_eq!(
            super::claude_workspace_trust_key(&cancel_selected, cwd),
            Some(b"\x1b[B".as_slice())
        );
        for unrelated in [
            screen.replace("/work/project", "/work/other"),
            screen.replace("Accessing workspace:", "Allow this command?"),
            screen.replace("❯ Yes, I trust this folder", "Yes, I trust this folder"),
            format!("{screen}\nClaude is ready for a prompt"),
            screen.replace("Enter to confirm · Esc to cancel", "Loading…"),
        ] {
            assert_eq!(super::claude_workspace_trust_key(&unrelated, cwd), None);
        }
    }

    #[test]
    fn codex_publishes_conversation_name_instead_of_project_directory() {
        let spec = TerminalLaunchSpec {
            account_id: None,
            kind: TerminalLaunchKind::Codex,
            cwd: std::env::temp_dir(),
            program: Some("/bin/cat".into()),
            args: vec![],
            title: "Codex".into(),
            login_env: false,
            variant: None,
            model: None,
        };
        let prepared = spec
            .prepare(vec!["--".into(), "hello".into()], vec![])
            .unwrap();
        assert!(
            prepared
                .args
                .windows(2)
                .any(|args| args == ["-c", r#"tui.terminal_title=["session-id","thread-name"]"#])
        );
        assert_eq!(&prepared.args[prepared.args.len() - 2..], &["--", "hello"]);
    }

    #[test]
    fn provider_title_updates_survive_terminal_title_reset() {
        for kind in [TerminalLaunchKind::Codex, TerminalLaunchKind::Claude] {
            let mut snapshot = super::TerminalHostSnapshot {
                kind,
                cwd: std::env::temp_dir(),
                title: kind.label().into(),
                status: super::TerminalHostStatus::Running,
                pid: 1,
                summary: None,
                last_output: String::new(),
                conversation_hint: None,
                title_loading: false,
                title_pending_since: None,
            };
            for title in [
                Some("Build the sidebar"),
                Some("Resize sidebar smoothly"),
                Some("⠧ | renaming... ⠧"),
                None,
                Some(""),
            ] {
                snapshot.update_title(title, kind.label());
            }
            assert_eq!(snapshot.title, "Resize sidebar smoothly");
            assert!(snapshot.title_loading);
            snapshot.title_loading = false; // completed metadata refresh
            snapshot.update_title(Some("⠋ | renaming... ⠋"), kind.label());
            assert!(
                !snapshot.title_loading,
                "repeated progress must not restart loading"
            );
            snapshot.update_title(Some("Fix renaming bugs"), kind.label());
            assert_eq!(snapshot.title, "Fix renaming bugs");
            assert!(!snapshot.title_loading);
            snapshot.update_title(Some("⠋ | renaming... ⠋"), kind.label());
            assert!(snapshot.title_loading, "a new rename can animate again");
        }
    }

    #[test]
    fn composer_settings_and_prompt_reach_terminal_argv() {
        for (kind, provider, effort_args) in [
            (
                TerminalLaunchKind::Codex,
                del_agent::Provider::Codex,
                vec![
                    "-c",
                    "model_reasoning_effort=\"high\"",
                    "-c",
                    "service_tier=\"priority\"",
                ],
            ),
            (
                TerminalLaunchKind::Claude,
                del_agent::Provider::Claude,
                vec!["--effort", "high"],
            ),
        ] {
            let config = del_agent::SessionConfig {
                provider,
                cwd: std::env::temp_dir(),
                model: Some("chosen-model".into()),
                reasoning_effort: Some("high".into()),
                service_tier: Some("priority".into()),
                permission_mode: Default::default(),
            };
            let mut spec = TerminalLaunchSpec {
                account_id: None,
                kind,
                cwd: config.cwd.clone(),
                program: Some("/bin/cat".into()),
                args: vec![],
                title: "Test".into(),
                login_env: false,
                variant: None,
                model: None,
            };
            spec.apply_model_settings(&config);
            let prompt = "build this; keep it one argument";
            let prepared = spec
                .prepare(vec!["--".into(), prompt.into()], vec![])
                .unwrap();
            let mut expected = if kind == TerminalLaunchKind::Claude {
                vec!["--settings", "{\"fastMode\":false}"]
            } else {
                vec![]
            };
            expected.extend(["--model", "chosen-model"]);
            expected.extend(effort_args);
            let trust = format!(
                "projects.{}.trust_level=\"trusted\"",
                serde_json::to_string(&config.cwd.canonicalize().unwrap().to_string_lossy())
                    .unwrap()
            );
            let canonical_cwd = config
                .cwd
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if kind == TerminalLaunchKind::Codex {
                expected.push("--dangerously-bypass-approvals-and-sandbox");
                expected.push("--no-alt-screen");
                expected.extend(["--cd", &canonical_cwd]);
                expected.extend(["-c", &trust]);
                expected.extend(["-c", r#"tui.terminal_title=["session-id","thread-name"]"#]);
            } else {
                expected.push("--dangerously-skip-permissions");
            }
            expected.extend(["--", prompt]);
            assert_eq!(prepared.args, expected);
            assert_eq!(prepared.cwd, Some(config.cwd.canonicalize().unwrap()));
        }
    }

    #[test]
    fn resumed_terminal_launches_receive_one_bypass_flag() {
        for (kind, flag) in [
            (
                TerminalLaunchKind::Codex,
                "--dangerously-bypass-approvals-and-sandbox",
            ),
            (TerminalLaunchKind::Claude, "--dangerously-skip-permissions"),
        ] {
            let mut spec = TerminalLaunchSpec {
                account_id: None,
                kind,
                cwd: std::env::temp_dir(),
                program: Some("/bin/cat".into()),
                args: vec![
                    if kind == TerminalLaunchKind::Codex {
                        "resume"
                    } else {
                        "--resume"
                    }
                    .into(),
                    "saved-id".into(),
                ],
                title: "Test".into(),
                login_env: false,
                variant: None,
                model: None,
            };
            for _ in 0..2 {
                let prepared = spec.prepare(vec![], vec![]).unwrap();
                assert_eq!(prepared.args.iter().filter(|arg| *arg == flag).count(), 1);
                assert!(prepared.args.contains(&"saved-id".into()));
                spec.args.push(flag.into());
            }
        }
    }

    #[test]
    fn shell_keeps_workspace_and_login_environment() {
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let spec = TerminalLaunchSpec::shell(&cwd);
        let prepared = spec
            .prepare(Vec::new(), vec![("TERM_PROGRAM".into(), "del-test".into())])
            .unwrap();
        assert_eq!(prepared.cwd, Some(cwd));
        assert!(prepared.program.is_none());
        assert!(prepared.login_env);
        assert_eq!(prepared.env[0].1, "del-test");
    }
    #[test]
    fn injected_bridge_configuration_never_mutates_serialized_history() {
        let spec = TerminalLaunchSpec {
            account_id: None,
            kind: TerminalLaunchKind::Codex,
            cwd: std::env::temp_dir(),
            program: Some("/bin/cat".into()),
            args: vec!["original".into()],
            title: "Codex".into(),
            login_env: false,
            variant: None,
            model: None,
        };
        let before = serde_json::to_string(&spec).unwrap();
        let prepared = spec
            .prepare(
                vec!["-c".into(), "mcp_servers.del.url=loopback".into()],
                vec![("DEL_MCP_TOKEN".into(), "test-secret".into())],
            )
            .unwrap();
        assert_eq!(
            prepared.args,
            [
                "original",
                "--dangerously-bypass-approvals-and-sandbox",
                "--no-alt-screen",
                "--cd",
                &spec.cwd.canonicalize().unwrap().to_string_lossy(),
                "-c",
                &format!(
                    "projects.{}.trust_level=\"trusted\"",
                    serde_json::to_string(&spec.cwd.canonicalize().unwrap().to_string_lossy())
                        .unwrap()
                ),
                "-c",
                r#"tui.terminal_title=["session-id","thread-name"]"#,
                "-c",
                "mcp_servers.del.url=loopback"
            ]
        );
        assert_eq!(prepared.env[0].1, "test-secret");
        assert_eq!(serde_json::to_string(&spec).unwrap(), before);
        assert!(!before.contains("test-secret"));
        assert!(!before.contains("DEL_MCP_TOKEN"));
        assert_eq!(
            serde_json::from_str::<TerminalLaunchSpec>(&before).unwrap(),
            spec
        );
    }
    #[test]
    fn custom_command_preserves_shell_syntax_as_one_argument() {
        let spec =
            TerminalLaunchSpec::custom("printf '%s' 'a b' | cat", std::env::temp_dir()).unwrap();
        assert_eq!(spec.args, ["-lc", "printf '%s' 'a b' | cat"]);
        assert!(!spec.login_env);
        assert_eq!(spec.kind, TerminalLaunchKind::Custom);
        assert!(TerminalLaunchSpec::custom("  ", std::env::temp_dir()).is_err());
    }
    #[test]
    fn invalid_workspace_is_rejected_before_spawning() {
        let spec = TerminalLaunchSpec::shell(PathBuf::from("relative-workspace"));
        assert!(spec.prepare(Vec::new(), Vec::new()).is_err());
        assert!(
            TerminalLaunchSpec::agent(TerminalLaunchKind::Shell, std::env::temp_dir()).is_err()
        );
    }
    #[test]
    fn prepared_launch_reaches_real_pty_at_workspace_and_reaps_child() {
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let spec = TerminalLaunchSpec {
            account_id: None,
            kind: TerminalLaunchKind::Custom,
            cwd: cwd.clone(),
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "test \"${CLAUDECODE+x}\" != x || exit 21; test \"${CLAUDE_CODE_CHILD_SESSION+x}\" != x || exit 22; printf 'cwd=%s\\nenv=%s\\narg=%s\\n' \"$PWD\" \"$DEL_MCP_TEST_TOKEN\" \"$1\""
                    .into(),
                "terminal-host-test".into(),
            ],
            title: "Test terminal".into(),
            login_env: false,
            variant: None,
            model: None,
        };
        let (session, actual_cwd) = super::spawn_session(
            &spec,
            vec!["injected argument".into()],
            vec![
                ("DEL_MCP_TEST_TOKEN".into(), "test-only-value".into()),
                ("CLAUDECODE".into(), "worker-test".into()),
                ("CLAUDE_CODE_CHILD_SESSION".into(), "worker-test".into()),
            ],
        )
        .unwrap();
        assert_eq!(actual_cwd, cwd);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.exited() {
            assert!(
                std::time::Instant::now() < deadline,
                "terminal host child did not exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = super::screen_tail(&session);
        assert!(
            output.contains(&format!("cwd={}", cwd.display())),
            "{output}"
        );
        assert!(output.contains("env=test-only-value"), "{output}");
        assert!(output.contains("arg=injected argument"), "{output}");
        session.close(std::time::Duration::ZERO);
    }
    #[test]
    fn failed_program_returns_error_before_entity_construction() {
        let spec = TerminalLaunchSpec {
            account_id: None,
            kind: TerminalLaunchKind::Custom,
            cwd: std::env::temp_dir(),
            program: Some("/del-no-such-program/terminal-host-test".into()),
            args: Vec::new(),
            title: "Missing program".into(),
            login_env: false,
            variant: None,
            model: None,
        };
        assert!(super::spawn_session(&spec, Vec::new(), Vec::new()).is_err());
    }
}

#[cfg(test)]
mod opencode_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn native_terminal_flags_and_model_metadata_are_kept_separate() {
        let spec = TerminalLaunchSpec {
            account_id: None,
            kind: TerminalLaunchKind::OpenCode,
            cwd: std::env::temp_dir(),
            program: Some("opencode".into()),
            args: vec![],
            title: "OpenCode".into(),
            login_env: false,
            variant: Some("high".into()),
            model: Some("provider/model".into()),
        };
        let prepared = spec
            .prepare(
                vec![
                    "--session".into(),
                    "ses_fixture".into(),
                    "--prompt".into(),
                    "hello".into(),
                ],
                vec![(
                    "OPENCODE_CLI_CONFIG_CONTENT".into(),
                    r#"{"theme":"test-theme","tabs":{"mode":"on","scope":"global"}}"#.into(),
                )],
            )
            .unwrap();
        let cli_config: serde_json::Value = serde_json::from_str(
            &prepared
                .env
                .iter()
                .find(|(key, _)| key == "OPENCODE_CLI_CONFIG_CONTENT")
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(cli_config["tabs"]["mode"], "off");
        assert_eq!(cli_config["tabs"]["scope"], "global");
        assert_eq!(cli_config["theme"], "test-theme");
        assert!(prepared.args.contains(&"--standalone".into()));
        assert!(prepared.args.contains(&"--auto".into()));
        assert!(!prepared.args.contains(&"--model".into()));
        assert!(!prepared.args.contains(&"--variant".into()));
        assert_eq!(spec.provider(), Some(del_agent::Provider::OpenCode));
        let restored: TerminalLaunchSpec =
            serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(restored, spec);
    }
    #[tokio::test]
    #[ignore = "requires OpenCode v2 and a network-accessible free model; use isolated XDG data"]
    async fn real_opencode_terminal_mcp_status_and_question() {
        let cwd = tempfile::tempdir().unwrap();
        let id =
            del_opencode::create_terminal_session(cwd.path(), Some("opencode/big-pickle"), None)
                .await
                .unwrap();
        let sid = del_agent::SessionId::new(id.clone());
        let host = std::sync::Arc::new(del_mcp::MockHost::new());
        host.add_session(&id, del_agent::Provider::OpenCode);
        let server = del_mcp::McpServer::start(host.clone(), Default::default())
            .await
            .unwrap();
        let launch = del_mcp::launch::opencode(&server.register_session(&sid));
        let mut spec = TerminalLaunchSpec::agent(TerminalLaunchKind::OpenCode, cwd.path()).unwrap();
        spec.login_env = false;
        let args=vec!["--session".into(),id,"--prompt".into(),
            "Use the Del MCP server: call report_status with state working and summary DEL_TERM_7930_OK, then call ask_user with question Integration test answer? and no options. Report the answer it returns. Use the MCP ask_user tool, not the built-in question tool.".into()];
        let terminal = TerminalSession::spawn(spec.prepare(args, launch.env).unwrap()).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
            let question = host.take_question().await;
            assert_eq!(question.caller, sid);
            assert!(
                host.reports()
                    .iter()
                    .any(|(caller, _, summary)| caller == &sid && summary == "DEL_TERM_7930_OK")
            );
            question.answer(del_mcp::UserAnswer::Text {
                text: "DEL_TERM_ANSWER_6382_OK".into(),
            });
            loop {
                let screen: String =
                    terminal.with_term(|t| t.grid().display_iter().map(|c| c.c).collect());
                if screen.contains("DEL_TERM_ANSWER_6382_OK") {
                    break;
                }
                assert!(!terminal.exited(), "terminal exited: {screen}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await;
        let screen = super::screen_tail(&terminal);
        terminal.close(std::time::Duration::from_millis(300));
        server.shutdown();
        assert!(
            result.is_ok(),
            "OpenCode terminal MCP did not complete status/question round-trip: {screen}"
        );
    }

    #[tokio::test]
    #[ignore = "requires installed OpenCode v2; use isolated XDG data"]
    async fn real_opencode_new_terminal_has_only_its_own_tab() {
        let cwd = tempfile::tempdir().unwrap();
        let mut spec = TerminalLaunchSpec::agent(TerminalLaunchKind::OpenCode, cwd.path()).unwrap();
        spec.login_env = false;
        for launch in 0..2 {
            let id = del_opencode::create_terminal_session(
                cwd.path(),
                Some("opencode/big-pickle"),
                None,
            )
            .await
            .unwrap();
            let mut prepared = spec.prepare(vec!["--session".into(), id], vec![]).unwrap();
            if launch == 0 {
                // Seed the saved tab strip left by an ordinary OpenCode launch.
                prepared
                    .env
                    .retain(|(key, _)| key != "OPENCODE_CLI_CONFIG_CONTENT");
                prepared.env.push((
                    "OPENCODE_CLI_CONFIG_CONTENT".into(),
                    r#"{"tabs":{"mode":"on"}}"#.into(),
                ));
            }
            let terminal = TerminalSession::spawn(prepared).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let mut ready = None;
            let screen = loop {
                let screen: String =
                    terminal.with_term(|t| t.grid().display_iter().map(|c| c.c).collect());
                if screen.contains("Big Pickle") {
                    let since = ready.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() > std::time::Duration::from_secs(2) {
                        break screen;
                    }
                }
                if terminal.exited() || std::time::Instant::now() > deadline {
                    terminal.close(std::time::Duration::from_millis(300));
                    panic!("terminal did not become ready: {screen}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            };
            terminal.close(std::time::Duration::from_millis(300));
            assert!(
                screen.matches("New session").count() <= 1,
                "launch {launch} restored another session tab: {screen}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires OpenCode v2 and a network-accessible free model; use isolated XDG data"]
    async fn real_opencode_terminal_launch_and_exact_resume() {
        let cwd = tempfile::tempdir().unwrap();
        let id =
            del_opencode::create_terminal_session(cwd.path(), Some("opencode/big-pickle"), None)
                .await
                .unwrap();
        let mut spec = TerminalLaunchSpec::agent(TerminalLaunchKind::OpenCode, cwd.path()).unwrap();
        spec.login_env = false;
        for resume in [false, true] {
            let mut args = vec!["--session".into(), id.clone()];
            if !resume {
                args.extend(["--prompt".into(),"Reply with the concatenation of DEL_ and TUI_OK and nothing else. Do not use tools.".into()]);
            }
            let terminal = TerminalSession::spawn(spec.prepare(args, vec![]).unwrap()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
            let result = loop {
                let screen: String =
                    terminal.with_term(|t| t.grid().display_iter().map(|c| c.c).collect());
                if screen.contains("DEL_TUI_OK") {
                    break Ok(());
                }
                if terminal.exited() || std::time::Instant::now() > deadline {
                    break Err(screen);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            };
            terminal.close(std::time::Duration::from_millis(300));
            result.unwrap_or_else(|screen| panic!("OpenCode terminal resume={resume}: {screen}"));
        }
    }
}
