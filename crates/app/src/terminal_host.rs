//! Main-app terminal launcher and running-session host.
//!
//! The parent subscribes to `TerminalLauncherEvent::Launch`, registers the
//! session with MCP, and constructs `TerminalHost::new_prepared`. Persist only
//! the original `TerminalLaunchSpec`; injected MCP args/env never enter it.
//! Keep MCP launch resources in the parent until exit or explicit close.

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
    Custom,
}

impl TerminalLaunchKind {
    pub const ALL: [Self; 4] = [Self::Shell, Self::Codex, Self::Claude, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Custom => "Custom",
        }
    }
}

/// History-safe command description. Never append MCP config to these fields
/// before persisting it; pass that configuration to `new_prepared` instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLaunchSpec {
    pub kind: TerminalLaunchKind,
    pub cwd: PathBuf,
    pub program: Option<String>,
    pub args: Vec<String>,
    pub title: String,
    pub login_env: bool,
}

impl TerminalLaunchSpec {
    pub fn provider(&self) -> Option<del_agent::Provider> {
        match self.kind {
            TerminalLaunchKind::Codex => Some(del_agent::Provider::Codex),
            TerminalLaunchKind::Claude => Some(del_agent::Provider::Claude),
            _ => None,
        }
    }
    pub fn label(&self) -> String {
        self.title.clone()
    }

    pub fn shell(cwd: impl Into<PathBuf>) -> Self {
        Self {
            kind: TerminalLaunchKind::Shell,
            cwd: cwd.into(),
            program: None,
            args: Vec::new(),
            title: "Shell".into(),
            login_env: true,
        }
    }
    pub fn agent(kind: TerminalLaunchKind, cwd: impl Into<PathBuf>) -> io::Result<Self> {
        let name = match kind {
            TerminalLaunchKind::Codex => "codex",
            TerminalLaunchKind::Claude => "claude",
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "choose Codex or Claude",
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
            kind,
            cwd: cwd.into(),
            program: Some(path.to_string_lossy().into_owned()),
            args: Vec::new(),
            title: kind.label().into(),
            login_env: true,
        })
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
            kind: TerminalLaunchKind::Custom,
            cwd: cwd.into(),
            program: Some(shell),
            args: vec!["-lc".into(), command],
            title: "Custom terminal".into(),
            login_env: false,
        })
    }

    fn prepare(
        &self,
        extra_args: Vec<String>,
        env: Vec<(String, String)>,
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
        args.extend(extra_args);
        Ok(TerminalSpec {
            program: self.program.clone(),
            args,
            env,
            cwd: Some(self.cwd.canonicalize()?),
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
            TerminalLaunchKind::Codex | TerminalLaunchKind::Claude => {
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
                    .max_w(px(560.))
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(20.))
                            .text_color(rgb(0xf5f5f5))
                            .child("Open a terminal"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
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
                                .text_size(px(12.))
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
}
impl EventEmitter<TerminalHostEvent> for TerminalHost {}
impl Focusable for TerminalHost {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal.read(cx).focus_handle(cx)
    }
}
impl TerminalHost {
    pub fn new(
        spec: TerminalLaunchSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<Self> {
        Self::new_prepared(spec, Vec::new(), Vec::new(), window, cx)
    }
    /// Inject bridge args/env without mutating the history-safe launch spec.
    /// The parent owns and revokes the associated MCP lease/config files.
    pub fn new_prepared(
        spec: TerminalLaunchSpec,
        extra_args: Vec<String>,
        env: Vec<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<Self> {
        let prepared = spec.prepare(extra_args, env)?;
        let cwd = prepared.cwd.clone().expect("validated workspace");
        let session = TerminalSession::spawn(prepared)?;
        let pid = session.pid();
        let terminal = cx.new(|cx| TerminalView::new(session, window, cx));
        cx.subscribe(&terminal, |this, terminal, event: &TerminalEvent, cx| {
            let output = screen_tail(terminal.read(cx).session());
            this.snapshot.summary = output
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(str::to_owned);
            this.snapshot.last_output = output;
            match event {
                TerminalEvent::TitleChanged(title) => {
                    this.snapshot.title =
                        title.clone().unwrap_or_else(|| this.fallback_title.clone())
                }
                TerminalEvent::Exited { code }
                    if this.snapshot.status != TerminalHostStatus::Closed =>
                {
                    this.snapshot.status = TerminalHostStatus::Exited { code: *code }
                }
                _ => {}
            }
            cx.emit(TerminalHostEvent::Terminal(event.clone()));
            cx.emit(TerminalHostEvent::Changed(this.snapshot.clone()));
            cx.notify();
        })
        .detach();
        let fallback_title = spec.title.clone();
        Ok(Self {
            terminal,
            fallback_title,
            snapshot: TerminalHostSnapshot {
                kind: spec.kind,
                cwd,
                title: spec.title,
                status: TerminalHostStatus::Running,
                pid,
                summary: None,
                last_output: String::new(),
            },
        })
    }
    pub fn snapshot(&self) -> TerminalHostSnapshot {
        self.snapshot.clone()
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = match self.snapshot.status {
            TerminalHostStatus::Running => "Running".into(),
            TerminalHostStatus::Exited { code: Some(code) } => format!("Exited {code}"),
            TerminalHostStatus::Exited { code: None } => "Ended by signal".into(),
            TerminalHostStatus::Closed => "Closed".into(),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(0))
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .flex_none()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(rgb(0xaaaaaa))
                            .child(self.snapshot.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x888888))
                            .child(status),
                    )
                    .child(
                        Button::new("copy-terminal-output")
                            .ghost()
                            .small()
                            .label("Copy output")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.snapshot.last_output.clone(),
                                ));
                            })),
                    )
                    .child(
                        Button::new("close-terminal-process")
                            .ghost()
                            .small()
                            .label("Close")
                            .disabled(self.snapshot.status == TerminalHostStatus::Closed)
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.terminal.clone()),
            )
    }
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

#[cfg(test)]
mod tests {
    use super::{TerminalLaunchKind, TerminalLaunchSpec};
    use std::path::PathBuf;

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
            kind: TerminalLaunchKind::Codex,
            cwd: std::env::temp_dir(),
            program: Some("/bin/cat".into()),
            args: vec!["original".into()],
            title: "Codex".into(),
            login_env: false,
        };
        let before = serde_json::to_string(&spec).unwrap();
        let prepared = spec
            .prepare(
                vec!["-c".into(), "mcp_servers.del.url=loopback".into()],
                vec![("DEL_MCP_TOKEN".into(), "test-secret".into())],
            )
            .unwrap();
        assert_eq!(prepared.args.len(), 3);
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
            kind: TerminalLaunchKind::Custom,
            cwd: cwd.clone(),
            program: Some("/bin/sh".into()),
            args: vec![
                "-c".into(),
                "printf 'cwd=%s\\nenv=%s\\narg=%s\\n' \"$PWD\" \"$DEL_MCP_TEST_TOKEN\" \"$1\""
                    .into(),
                "terminal-host-test".into(),
            ],
            title: "Test terminal".into(),
            login_env: false,
        };
        let prepared = spec
            .prepare(
                vec!["injected argument".into()],
                vec![("DEL_MCP_TEST_TOKEN".into(), "test-only-value".into())],
            )
            .unwrap();
        let session = del_terminal::TerminalSession::spawn(prepared).unwrap();
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
}
