//! PTY ownership and terminal emulation, independent of GPUI.

use crate::colors;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 16,
        }
    }
}

impl TerminalSize {
    fn pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols.saturating_mul(self.cell_width),
            pixel_height: self.rows.saturating_mul(self.cell_height),
        }
    }

    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Clone, Debug)]
pub struct TerminalSpec {
    /// None starts the user's login shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Names excluded from this child, even if also present in `env`. For
    /// login-wrapped commands they are cleared again after shell startup.
    pub env_remove: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub size: TerminalSize,
    pub scrollback: usize,
    /// Load the user's interactive login environment before executing a command.
    pub login_env: bool,
}

impl Default for TerminalSpec {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: None,
            size: TerminalSize::default(),
            scrollback: 10_000,
            login_env: true,
        }
    }
}

impl TerminalSpec {
    pub fn shell() -> Self {
        Self::default()
    }
    pub fn command(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: Some(program.into()),
            args: args.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Wakeup,
    TitleChanged(Option<String>),
    Bell,
    Exited { code: Option<i32> },
}

fn launch_command(spec: &TerminalSpec, shell: &str) -> (String, Vec<String>) {
    match &spec.program {
        None => (shell.into(), vec!["-l".into()]),
        Some(program) if spec.login_env => {
            let fish = shell.rsplit('/').next() == Some("fish");
            let script = match (fish, spec.env_remove.is_empty()) {
                (true, true) => "exec $argv",
                (false, true) => "exec \"$0\" \"$@\"",
                (true, false) => "exec /usr/bin/env $argv",
                (false, false) => "exec /usr/bin/env \"$0\" \"$@\"",
            };
            let mut args: Vec<String> = ["-l", "-i", "-c", script].map(String::from).into();
            // Pass names as argv, not shell source. The cleanup runs after
            // login/interactive startup files, immediately before the program.
            for name in &spec.env_remove {
                args.extend(["-u".into(), name.clone()]);
            }
            if !spec.env_remove.is_empty() {
                args.push("--".into());
            }
            args.push(program.clone());
            args.extend(spec.args.iter().cloned());
            (shell.into(), args)
        }
        Some(program) => (program.clone(), spec.args.clone()),
    }
}

fn base_env(spec: &TerminalSpec) -> HashMap<String, String> {
    let mut env = HashMap::from([
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "del".into()),
    ]);
    if !std::env::var("LANG").is_ok_and(|l| l.to_ascii_uppercase().contains("UTF-8")) {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }
    env.extend(spec.env.iter().cloned());
    env
}

// An embedding process (including an IDE or build runner) may disable colour
// for its captured output. A real PTY should detect its own capabilities.
fn configure_environment(command: &mut CommandBuilder, spec: &TerminalSpec) {
    for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
        command.env_remove(key);
    }
    for (key, value) in base_env(spec) {
        command.env(key, value);
    }
    for key in &spec.env_remove {
        command.env_remove(key);
    }
}

// Job-control shells put foreground jobs in separate process groups. Sweep
// groups belonging to the PTY's session, rather than only the shell's group.
struct ProcessSession {
    pid: i32,
    terminated: Mutex<bool>,
}
static LIVE: Mutex<Vec<Arc<ProcessSession>>> = Mutex::new(Vec::new());

impl ProcessSession {
    fn terminate(&self, grace: Duration) {
        let mut terminated = self.terminated.lock().unwrap();
        if *terminated {
            return;
        }
        let mut groups = session_groups(self.pid);
        for &group in &groups {
            unsafe {
                libc::killpg(group, libc::SIGHUP);
            }
        }
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline
            && groups.iter().any(|&g| unsafe { libc::kill(-g, 0) == 0 })
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        groups.extend(session_groups(self.pid));
        for group in groups {
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
        *terminated = true;
    }
}

fn session_groups(sid: i32) -> HashSet<i32> {
    let mut groups = HashSet::from([sid]);
    if let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid="])
        .output()
    {
        for pid in String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|p| p.parse::<i32>().ok())
        {
            unsafe {
                if libc::getsid(pid) == sid {
                    let group = libc::getpgid(pid);
                    if group > 0 {
                        groups.insert(group);
                    }
                }
            }
        }
    }
    groups
}

/// Call from the host's app-quit hook. Also kills foreground job-control groups.
pub fn shutdown_all(grace: Duration) {
    let sessions = std::mem::take(&mut *LIVE.lock().unwrap());
    for session in sessions {
        session.terminate(grace);
    }
}

struct Shared {
    tx: UnboundedSender<TerminalEvent>,
    input: mpsc::Sender<Option<Vec<u8>>>,
    wakeup_pending: AtomicBool,
    exited: AtomicBool,
    child_exited: AtomicBool,
    size: Mutex<TerminalSize>,
}

#[derive(Clone)]
pub struct TerminalEventProxy(Arc<Shared>);

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        let s = &self.0;
        let event = match event {
            Event::Wakeup if !s.wakeup_pending.swap(true, Ordering::AcqRel) => {
                Some(TerminalEvent::Wakeup)
            }
            Event::Title(t) => Some(TerminalEvent::TitleChanged(Some(t))),
            Event::ResetTitle => Some(TerminalEvent::TitleChanged(None)),
            Event::Bell => Some(TerminalEvent::Bell),
            Event::PtyWrite(text) => {
                let _ = s.input.send(Some(text.into_bytes()));
                None
            }
            Event::ColorRequest(index, format) => {
                let _ = s
                    .input
                    .send(Some(format(colors::palette(index)).into_bytes()));
                None
            }
            Event::TextAreaSizeRequest(format) => {
                let _ = s.input.send(Some(
                    format(s.size.lock().unwrap().window_size()).into_bytes(),
                ));
                None
            }
            // Clipboard reads/writes require explicit user copy/paste.
            _ => None,
        };
        if let Some(event) = event {
            let _ = s.tx.unbounded_send(event);
        }
    }
}

/// Owns the PTY and child. Drop closes it and reaps the child.
pub struct TerminalSession {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    master: Box<dyn MasterPty + Send>,
    shared: Arc<Shared>,
    process: Arc<ProcessSession>,
    events: Option<UnboundedReceiver<TerminalEvent>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl TerminalSession {
    pub fn spawn(spec: TerminalSpec) -> io::Result<Self> {
        if spec.size.cols == 0 || spec.size.rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal size must be nonzero",
            ));
        }
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        let (program, args) = launch_command(&spec, &shell);
        let mut command = CommandBuilder::new(program);
        command.args(args);
        if let Some(cwd) = &spec.cwd {
            command.cwd(cwd);
        }
        configure_environment(&mut command, &spec);
        let pair = native_pty_system()
            .openpty(spec.size.pty_size())
            .map_err(io::Error::other)?;
        let poll_fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("PTY has no Unix file descriptor"))?;
        let mut reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
        let mut writer = pair.master.take_writer().map_err(io::Error::other)?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(io::Error::other)?;
        drop(pair.slave);
        let pid = child.process_id().expect("Unix PTY child has a pid") as i32;
        let process = Arc::new(ProcessSession {
            pid,
            terminated: Mutex::new(false),
        });
        LIVE.lock().unwrap().push(process.clone());
        let (tx, rx) = unbounded();
        let (input, input_rx) = mpsc::channel::<Option<Vec<u8>>>();
        let shared = Arc::new(Shared {
            tx,
            input,
            wakeup_pending: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            child_exited: AtomicBool::new(false),
            size: Mutex::new(spec.size),
        });
        let proxy = TerminalEventProxy(shared.clone());
        let term = Arc::new(FairMutex::new(Term::new(
            Config {
                scrolling_history: spec.scrollback,
                ..Config::default()
            },
            &spec.size,
            proxy,
        )));
        let writer_thread = std::thread::spawn(move || {
            while let Ok(Some(bytes)) = input_rx.recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let read_term = term.clone();
        let read_shared = shared.clone();
        let reader_thread = std::thread::spawn(move || {
            let mut parser: Processor = Processor::new();
            let mut buf = [0; 65536];
            let wake = || {
                if !read_shared.wakeup_pending.swap(true, Ordering::AcqRel) {
                    let _ = read_shared.tx.unbounded_send(TerminalEvent::Wakeup);
                }
            };
            loop {
                // Poll also bounds synchronized-output buffering when the
                // program omits its ending marker or stops writing.
                if parser
                    .sync_timeout()
                    .sync_timeout()
                    .is_some_and(|deadline| deadline <= std::time::Instant::now())
                {
                    parser.stop_sync(&mut *read_term.lock());
                    wake();
                }
                let mut poll = libc::pollfd {
                    fd: poll_fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut poll, 1, 50) };
                if ready < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if ready == 0 {
                    if read_shared.child_exited.load(Ordering::Acquire) {
                        break;
                    }
                    continue;
                }
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        parser.advance(&mut *read_term.lock(), &buf[..n]);
                        if parser.sync_timeout().sync_timeout().is_none() {
                            wake();
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            parser.stop_sync(&mut *read_term.lock());
            wake();
        });
        let wait_shared = shared.clone();
        let wait_process = process.clone();
        let worker = std::thread::spawn(move || {
            let status = child.wait().ok();
            wait_process.terminate(Duration::from_millis(200));
            wait_shared.child_exited.store(true, Ordering::Release);
            let _ = reader_thread.join();
            let _ = wait_shared.input.send(None);
            let _ = writer_thread.join();
            LIVE.lock()
                .unwrap()
                .retain(|p| !Arc::ptr_eq(p, &wait_process));
            wait_shared.exited.store(true, Ordering::Release);
            let code = status.and_then(|s| (s.signal().is_none()).then_some(s.exit_code() as i32));
            let _ = wait_shared
                .tx
                .unbounded_send(TerminalEvent::Exited { code });
        });
        Ok(Self {
            term,
            master: pair.master,
            shared,
            process,
            events: Some(rx),
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn take_events(&mut self) -> Option<UnboundedReceiver<TerminalEvent>> {
        self.events.take()
    }
    pub fn ack_wakeup(&self) {
        self.shared.wakeup_pending.store(false, Ordering::Release);
    }
    pub fn with_term<R>(&self, f: impl FnOnce(&mut Term<TerminalEventProxy>) -> R) -> R {
        f(&mut self.term.lock())
    }
    pub fn pid(&self) -> u32 {
        self.process.pid as u32
    }
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }
    pub fn size(&self) -> TerminalSize {
        *self.shared.size.lock().unwrap()
    }
    /// Input is queued off the UI thread.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        if !self.exited() {
            let _ = self.shared.input.send(Some(bytes.into()));
        }
    }
    pub fn resize(&self, size: TerminalSize) -> io::Result<()> {
        if size.cols == 0 || size.rows == 0 {
            return Ok(());
        }
        let mut current = self.shared.size.lock().unwrap();
        if *current == size {
            return Ok(());
        }
        self.master
            .resize(size.pty_size())
            .map_err(io::Error::other)?;
        *current = size;
        drop(current);
        self.term.lock().resize(size);
        Ok(())
    }
    pub fn close(&self, grace: Duration) {
        self.process.terminate(grace);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pty_colour_detection_ignores_parent_overrides_but_honours_explicit_settings() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.env("NO_COLOR", "1");
        command.env("FORCE_COLOR", "0");
        command.env("CLICOLOR", "0");
        command.env("CLICOLOR_FORCE", "0");
        configure_environment(&mut command, &TerminalSpec::shell());
        for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
            assert!(command.get_env(key).is_none());
        }
        assert_eq!(
            command.get_env("TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        assert_eq!(
            command.get_env("COLORTERM"),
            Some(std::ffi::OsStr::new("truecolor"))
        );
        configure_environment(&mut command, &TerminalSpec::shell().env("NO_COLOR", "1"));
        assert_eq!(command.get_env("NO_COLOR"), Some(std::ffi::OsStr::new("1")));
    }

    #[test]
    fn command_arguments_are_not_interpolated() {
        let s = TerminalSpec::command("claude", ["a b", "$(touch nope)"]);
        let (p, a) = launch_command(&s, "/bin/zsh");
        assert_eq!(p, "/bin/zsh");
        assert_eq!(
            a,
            [
                "-l",
                "-i",
                "-c",
                "exec \"$0\" \"$@\"",
                "claude",
                "a b",
                "$(touch nope)"
            ]
        );
        let (_, a) = launch_command(&s, "/usr/local/bin/fish");
        assert_eq!(a[3], "exec $argv");
    }
    #[test]
    fn default_runs_login_shell() {
        assert_eq!(
            launch_command(&TerminalSpec::shell(), "/bin/zsh"),
            ("/bin/zsh".into(), vec!["-l".into()])
        );
    }
    #[test]
    fn environment_can_be_overridden() {
        let env = base_env(&TerminalSpec::shell().env("TERM", "dumb"));
        assert_eq!(env["TERM"], "dumb");
        assert_eq!(env["COLORTERM"], "truecolor");
    }
    #[test]
    fn removed_names_and_arguments_are_positional_after_shell_startup() {
        let spec = TerminalSpec::command("/bin/echo", ["a b", "$(touch nope)"])
            .env_remove("CLAUDECODE")
            .env_remove("DEL_WORKER_ONLY");
        let (_, args) = launch_command(&spec, "/bin/zsh");
        assert_eq!(args[3], "exec /usr/bin/env \"$0\" \"$@\"");
        assert_eq!(
            &args[4..],
            [
                "-u",
                "CLAUDECODE",
                "-u",
                "DEL_WORKER_ONLY",
                "--",
                "/bin/echo",
                "a b",
                "$(touch nope)"
            ]
        );
        let (_, args) = launch_command(&spec, "/usr/local/bin/fish");
        assert_eq!(args[3], "exec /usr/bin/env $argv");
    }
}
