//! Session-scoped snapshots of Del-owned process trees. Only a runtime entry-point
//! name may be shown; full command lines are never retained.
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub identity: Option<Identity>,
    pub name: String,
    pub ports: Vec<String>,
    pub background: bool,
    pub owner: Option<String>,
}
#[derive(Clone)]
pub struct Root {
    pub pid: u32,
    /// None for a shared provider server or a global shell: never guess ownership.
    pub owner: Option<String>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub rows: Vec<Process>,
    pub error: Option<String>,
    pub ready: bool,
    pub tags: BTreeMap<String, String>,
}
fn parse(text: &str, roots: &[Root], anchors: &[Root], app: u32) -> Vec<Process> {
    let processes: BTreeMap<u32, (u32, String, bool)> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let (parent, rest) = rest.trim_start().split_once(char::is_whitespace)?;
            let (state, name) = rest.trim_start().split_once(char::is_whitespace)?;
            let name = std::path::Path::new(name.trim())
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some((
                pid.parse().ok()?,
                (parent.parse().ok()?, name, state.starts_with('Z')),
            ))
        })
        .collect();
    let mut owned: HashSet<u32> = roots.iter().chain(anchors).map(|r| r.pid).collect();
    loop {
        let before = owned.len();
        for (pid, (parent, _, _)) in &processes {
            if owned.contains(parent) {
                owned.insert(*pid);
            }
        }
        if before == owned.len() {
            break;
        }
    }
    // Codex keeps its REPL host and Node wrapper alive between tool calls.
    // Identify by executable names/parentage, without inspecting command arguments.
    let repl_hosts: HashSet<u32> = processes
        .iter()
        .filter(|(_, (_, name, _))| name == "node_repl")
        .flat_map(|(pid, (parent, _, _))| {
            let wrapper = processes
                .get(parent)
                .is_some_and(|(_, name, _)| name == "node");
            std::iter::once(*pid).chain(wrapper.then_some(*parent))
        })
        .collect();
    let owners: BTreeMap<u32, Option<String>> = processes
        .keys()
        .map(|pid| {
            let mut cursor = *pid;
            let mut visited = HashSet::new();
            let mut owner = None;
            while visited.insert(cursor) {
                let matches = roots
                    .iter()
                    .chain(anchors)
                    .filter(|r| r.pid == cursor)
                    .collect::<Vec<_>>();
                if !matches.is_empty() {
                    let first = matches[0].owner.clone();
                    if matches.iter().all(|r| r.owner == first) {
                        owner = first;
                    }
                    break;
                }
                let Some((parent, _, _)) = processes.get(&cursor) else {
                    break;
                };
                cursor = *parent;
            }
            (*pid, owner)
        })
        .collect();
    processes
        .into_iter()
        .filter(|(pid, (parent, name, exited))| {
            let command = name.trim_matches(['(', ')']);
            !exited
                && owned.contains(pid)
                && *pid != app
                && !matches!(command, "ps" | "lsof")
                // Ignore Del's Git inspection; commands beneath agents and shells remain visible.
                && !(*parent == app && command == "git")
        })
        .map(|(pid, (parent, name, _))| {
            let command = name.trim_matches(['(', ')']).trim_start_matches('-');
            let background = matches!(
                command,
                "claude"
                    | "codex"
                    | "opencode"
                    | "del"
                    | "del-mcp-stdio"
                    | "zsh"
                    | "bash"
                    | "sh"
                    | "fish"
            ) || repl_hosts.contains(&pid)
                || roots.iter().any(|r| r.pid == pid)
                || (parent == app
                    && matches!(
                        command,
                        "zsh" | "bash" | "sh" | "fish" | "claude" | "codex" | "opencode"
                    ));
            Process {
                pid,
                identity: None,
                name,
                ports: vec![],
                background,
                owner: owners.get(&pid).cloned().flatten(),
            }
        })
        .collect()
}
fn add_ports(rows: &mut [Process], text: &str) {
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(address) = line.strip_prefix('n')
            && let Some(row) = rows.iter_mut().find(|row| Some(row.pid) == pid)
            && let Some((_, port)) = address.rsplit_once(':')
            && port.parse::<u16>().is_ok()
            && !row.ports.iter().any(|p| p == port)
        {
            row.ports.push(port.into());
        }
    }
}
async fn output(program: &str, args: &[String]) -> Option<std::process::Output> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()
}
/// Process birth time prevents a stale row from targeting a recycled PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub seconds: u64,
    pub micros: u64,
}

#[cfg(target_os = "macos")]
fn identity(pid: u32) -> Option<Identity> {
    if pid <= 1 || pid > i32::MAX as u32 {
        return None;
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the OS writes at most the size of the allocated structure.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if read != size as i32 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_uid != unsafe { libc::geteuid() } || info.pbi_status == 5 {
        return None;
    }
    Some(Identity {
        seconds: info.pbi_start_tvsec,
        micros: info.pbi_start_tvusec,
    })
}
#[cfg(not(target_os = "macos"))]
fn identity(_: u32) -> Option<Identity> {
    None
}

// macOS packs argc, the executable path, alignment padding, argv and env into
// one buffer. The argc boundary keeps arguments and environment entries distinct.
fn launch_fields(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let argc = usize::try_from(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
    if argc > 65536 {
        return None;
    }
    let payload = &bytes[4..];
    let rest = &payload[payload.iter().position(|&b| b == 0)? + 1..];
    Some((argc, &rest[rest.iter().position(|&b| b != 0)?..]))
}

fn marker_from_buffer(bytes: &[u8]) -> Option<String> {
    let (argc, mut rest) = launch_fields(bytes)?;
    for _ in 0..argc {
        rest = &rest[rest.iter().position(|&b| b == 0)? + 1..];
    }
    let prefix = b"DEL_PROCESS_TAG=";
    rest.split(|&b| b == 0).find_map(|entry| {
        let value = entry.strip_prefix(prefix)?;
        (value.len() == 36 && value.iter().all(|b| b.is_ascii_hexdigit() || *b == b'-'))
            .then(|| String::from_utf8_lossy(value).into_owned())
    })
}
// Only inspect the interpreter's entry-point position. Options such as -e/-c
// contain arbitrary code and other operands can contain credentials.
#[derive(Clone, Copy)]
enum Interpreter {
    Node,
    Python,
}
impl Interpreter {
    fn from_executable(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower == "node" || lower == "nodejs" {
            Some(Self::Node)
        } else if lower
            .strip_prefix("python")
            .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
        {
            Some(Self::Python)
        } else {
            None
        }
    }

    fn entry_name(self, bytes: &[u8]) -> Option<String> {
        let (argc, fields) = launch_fields(bytes)?;
        let mut args = fields.split(|&b| b == 0).take(argc).skip(1);
        let mut arg = args.next()?;
        if arg == b"--" {
            arg = args.next()?;
        } else if matches!(self, Self::Python) && arg == b"-m" {
            let module = std::str::from_utf8(args.next()?).ok()?;
            return (module.len() <= 96
                && module.split('.').all(|part| {
                    !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                }))
            .then(|| module.to_owned());
        }
        let path = std::str::from_utf8(arg).ok()?;
        if path.starts_with('-') || path.contains(['=', '?', '#', ':']) {
            return None;
        }
        let path = std::path::Path::new(path);
        let name = path.file_name()?.to_str()?;
        let installed_launcher = path.is_absolute()
            && path
                .parent()
                .is_some_and(|parent| parent.ends_with("bin") || parent.ends_with(".bin"));
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let script = match self {
            Self::Node => matches!(ext, "js" | "mjs" | "cjs" | "ts" | "mts" | "cts"),
            Self::Python => matches!(ext, "py" | "pyw"),
        };
        (installed_launcher || script).then_some(())?;
        (name.len() <= 96
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
        .then(|| name.to_owned())
    }
}

#[cfg(target_os = "macos")]
fn inspect_launch<T>(pid: u32, extract: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut bytes = vec![0u8; 1024 * 1024];
    let mut len = bytes.len();
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    let result = if ok { extract(&bytes[..len]) } else { None };
    bytes.fill(0);
    result
}
#[cfg(not(target_os = "macos"))]
fn inspect_launch<T>(_: u32, _: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
    None
}

fn process_marker(pid: u32) -> Option<String> {
    inspect_launch(pid, marker_from_buffer)
}

async fn process_table() -> Result<String, String> {
    output("/bin/ps", &["-axo".into(), "pid=,ppid=,stat=,comm=".into()])
        .await
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .ok_or_else(|| "Could not read running processes".into())
}

pub async fn read(
    roots: Vec<Root>,
    mut tags: BTreeMap<String, String>,
    previous: Snapshot,
) -> Snapshot {
    // Keep launch markers for detached commands after their provider closes.
    for (tag, owner) in previous.tags {
        tags.entry(tag).or_insert(owner);
    }
    let text = match process_table().await {
        Ok(text) => text,
        Err(error) => {
            return Snapshot {
                error: Some(error),
                ready: true,
                tags,
                ..Default::default()
            };
        }
    };
    let mut births = BTreeMap::new();
    let mut anchors = Vec::new();
    for pid in text
        .lines()
        .filter_map(|line| line.split_whitespace().next()?.parse::<u32>().ok())
    {
        let Some(birth) = identity(pid) else { continue };
        if !tags.is_empty()
            && let Some(owner) = process_marker(pid).and_then(|tag| tags.get(&tag))
        {
            anchors.push(Root {
                pid,
                owner: Some(owner.clone()),
            });
        } else if let Some(old) = previous
            .rows
            .iter()
            .find(|r| r.pid == pid && !r.background && r.identity.as_ref() == Some(&birth))
        {
            anchors.push(Root {
                pid,
                owner: old.owner.clone(),
            });
        }
        births.insert(pid, birth);
    }
    let mut rows = parse(&text, &roots, &anchors, std::process::id());
    for row in &mut rows {
        row.identity = births.get(&row.pid).cloned();
        if !row.background
            && row.identity.is_some()
            && let Some(runtime) = Interpreter::from_executable(&row.name)
            && let Some(name) = inspect_launch(row.pid, |bytes| runtime.entry_name(bytes))
            && identity(row.pid) == row.identity
        {
            row.name = name;
        }
    }
    if !rows.is_empty() {
        let pids = rows
            .iter()
            .map(|p| p.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if let Some(ports) = output(
            "/usr/sbin/lsof",
            &[
                "-nP".into(),
                "-a".into(),
                "-p".into(),
                pids,
                "-iTCP".into(),
                "-sTCP:LISTEN".into(),
                "-Fpn".into(),
            ],
        )
        .await
        {
            add_ports(&mut rows, &String::from_utf8_lossy(&ports.stdout));
        }
    }
    rows.sort_by(|a, b| {
        b.ports
            .len()
            .cmp(&a.ports.len())
            .then(a.name.cmp(&b.name))
            .then(a.pid.cmp(&b.pid))
    });
    Snapshot {
        rows,
        error: None,
        ready: true,
        tags,
    }
}

pub const SHARED_SECTION: &str = "Shared";

impl Snapshot {
    pub fn sections(&self, selected: Option<&str>) -> Vec<(&'static str, Vec<&Process>)> {
        let Some(owner) = selected else {
            return vec![("All sessions", self.commands(None).collect())];
        };
        let mut current = Vec::new();
        let mut other = Vec::new();
        let mut unassigned = Vec::new();
        for process in self.commands(None) {
            match process.owner.as_deref() {
                Some(id) if id == owner => current.push(process),
                Some(_) => other.push(process),
                None => unassigned.push(process),
            }
        }
        let mut sections = vec![("This session", current), ("Other sessions", other)];
        if !unassigned.is_empty() {
            sections.push((SHARED_SECTION, unassigned));
        }
        sections
    }

    pub fn commands(&self, owner: Option<&str>) -> impl Iterator<Item = &Process> {
        self.rows.iter().filter(move |p| {
            !p.background && owner.is_none_or(|owner| p.owner.as_deref() == Some(owner))
        })
    }
}

/// Stop only the selected command and descendants, never its provider's process
/// group (shared groups can contain other chats). Every signal rechecks birth time.
pub async fn stop(process: Process) -> Result<(), String> {
    if process.background || process.pid == std::process::id() || process.identity.is_none() {
        return Err("This process cannot be stopped from here".into());
    }
    let Some(current) = identity(process.pid) else {
        return Ok(());
    };
    if process.identity.as_ref() != Some(&current) {
        return Err("The process changed; reopen the process list".into());
    }
    let text = process_table().await?;
    let mut targets = parse(
        &text,
        &[],
        &[Root {
            pid: process.pid,
            owner: process.owner.clone(),
        }],
        std::process::id(),
    );
    for target in &mut targets {
        target.identity = identity(target.pid);
    }
    // Do not replace the selected row's checked identity with a newer one.
    if !targets
        .iter()
        .any(|p| p.pid == process.pid && p.identity == process.identity)
    {
        return Err("The process changed; reopen the process list".into());
    }
    for target in targets.iter().rev() {
        signal(target, libc::SIGTERM)?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while targets
        .iter()
        .any(|p| p.identity.is_some() && identity(p.pid) == p.identity)
    {
        if tokio::time::Instant::now() >= deadline {
            for target in targets.iter().rev() {
                signal(target, libc::SIGKILL)?;
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    Ok(())
}
fn signal(process: &Process, signal: i32) -> Result<(), String> {
    if process.identity.is_none() || identity(process.pid) != process.identity {
        return Ok(());
    }
    if unsafe { libc::kill(process.pid as i32, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("Could not stop {}: {error}", process.name));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Real CLI contract test: creates two new isolated conversations, asks each
    /// to launch only the local fixture, and checks OS ownership and stop isolation.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "requires an authenticated provider; uses two short model turns"]
    async fn real_provider_servers_keep_session_ownership() {
        use del_agent::{
            AgentBackend, AgentEvent, PermissionMode, Provider, SessionConfig, UserInput,
        };
        let provider =
            std::env::var("DEL_PROCESS_TEST_PROVIDER").unwrap_or_else(|_| "codex".into());
        let backend: std::sync::Arc<dyn AgentBackend> = match provider.as_str() {
            "claude" => std::sync::Arc::new(del_claude::ClaudeBackend::new(Default::default())),
            "opencode" => std::sync::Arc::new(del_opencode::OpenCodeBackend::default()),
            "codex" => std::sync::Arc::new(del_codex::CodexBackend::new(Default::default())),
            _ => panic!("unknown test provider"),
        };
        let dir = tempfile::tempdir().unwrap();
        let script = "import subprocess,sys,json\ncode=\"import socket,time,os,json,sys; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); open(sys.argv[1],'w').write(json.dumps({'pid':os.getpid(),'port':s.getsockname()[1]})); time.sleep(300)\"\np=subprocess.Popen([sys.executable,'-c',code,sys.argv[1]],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\n";
        std::fs::write(dir.path().join("fixture.py"), script).unwrap();
        let mut servers = Vec::new();
        let mut roots = Vec::new();
        let mut tags = BTreeMap::new();
        let result = std::panic::AssertUnwindSafe(async {
            for label in ["a", "b"] {
                let session = backend
                    .start_session(SessionConfig {
                        cwd: dir.path().into(),
                        provider: backend.provider(),
                        model: if backend.provider() == Provider::OpenCode {
                            Some("opencode/big-pickle".into())
                        } else {
                            None
                        },
                        reasoning_effort: None,
                        service_tier: None,
                        permission_mode: PermissionMode::FullAuto,
                    })
                    .await
                    .unwrap();
                let mut rx = session.subscribe();
                backend.send_prompt(&session.id, UserInput::text(format!("Isolated Del integration test. Run exactly `python3 fixture.py {label}.json` in the current directory using your shell tool. This starts a disposable local test server. Do not inspect files, run other commands, or stop the server. Then reply READY."))).await.unwrap();
                loop {
                    let event = tokio::time::timeout(Duration::from_secs(120), rx.recv())
                        .await
                        .unwrap()
                        .unwrap();
                    if let AgentEvent::Error { message, .. } = &event {
                        panic!("{provider}: {message}");
                    }
                    if matches!(event, AgentEvent::TurnCompleted { .. }) {
                        break;
                    }
                }
                let record: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(dir.path().join(format!("{label}.json")))
                        .expect("provider ran fixture"),
                )
                .unwrap();
                let pid = record["pid"].as_u64().unwrap() as u32;
                servers.push((
                    pid,
                    record["port"].as_u64().unwrap() as u16,
                    identity(pid).unwrap(),
                ));
                let owner = format!("chat:{label}");
                let tag = backend
                    .process_tag(&session.id)
                    .expect("provider marker registered");
                assert_eq!(
                    process_marker(pid).as_deref(),
                    Some(tag.as_str()),
                    "{provider} command must inherit session marker"
                );
                tags.insert(tag, owner.clone());
                let (pid, shared) = backend.session_process(&session.id).await.unwrap();
                roots.push(Root {
                    pid,
                    owner: (!shared).then_some(owner),
                });
            }
            let snapshot = read(roots, tags, Snapshot::default()).await;
            for (i, label) in ["a", "b"].iter().enumerate() {
                let rows: Vec<_> = snapshot.commands(Some(&format!("chat:{label}"))).collect();
                assert!(
                    rows.iter().any(|p| p.pid == servers[i].0),
                    "own server appears"
                );
                assert!(
                    !rows.iter().any(|p| p.pid == servers[1 - i].0),
                    "other server never leaks into session"
                );
            }
            assert!(
                servers
                    .iter()
                    .all(|(pid, _, _)| snapshot.commands(None).any(|p| p.pid == *pid))
            );
            stop(
                snapshot
                    .rows
                    .iter()
                    .find(|p| p.pid == servers[0].0)
                    .unwrap()
                    .clone(),
            )
            .await
            .unwrap();
            assert!(
                std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, servers[1].1)).is_ok()
            );
            eprintln!(
                "{provider}: two real sessions, detached servers, exact ownership, ports and isolated stop passed"
            );
        });
        // Catch panics so even an assertion failure cleans up our own test servers.
        use futures::FutureExt;
        let outcome = result.catch_unwind().await;
        for (pid, _, birth) in servers {
            if identity(pid).as_ref() == Some(&birth) {
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
            }
        }
        let _ = backend.shutdown().await;
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
    #[test]
    fn runtime_names_show_only_the_entry_point() {
        fn label(runtime: Interpreter, args: &[&str]) -> Option<String> {
            let mut bytes = (args.len() as i32).to_ne_bytes().to_vec();
            bytes.extend_from_slice(b"/runtime\0\0");
            for arg in args {
                bytes.extend_from_slice(arg.as_bytes());
                bytes.push(0);
            }
            bytes.extend_from_slice(b"SECRET=never-display.js\0");
            runtime.entry_name(&bytes)
        }
        use Interpreter::{Node, Python};
        assert_eq!(
            label(
                Node,
                &["node", "/opt/bin/concord-mcp", "--token", "private"]
            )
            .as_deref(),
            Some("concord-mcp")
        );
        assert_eq!(
            label(Node, &["node", "/project/node_modules/.bin/vite"]).as_deref(),
            Some("vite")
        );
        assert_eq!(
            label(Node, &["node", "src/server.mjs"]).as_deref(),
            Some("server.mjs")
        );
        assert_eq!(
            label(Python, &["python3", "-m", "http.server", "8000"]).as_deref(),
            Some("http.server")
        );
        for args in [
            &["node", "-e", "private.js"][..],
            &["node", "--require", "secret.js"],
            &["node"],
        ] {
            assert!(label(Node, args).is_none());
        }
        assert!(label(Python, &["python", "-c", "private.py"]).is_none());
    }

    #[test]
    fn unassigned_helpers_are_not_other_sessions() {
        let snapshot = Snapshot {
            rows: parse(
                "10 1 S codex\n11 10 S node\n12 10 S python\n13 10 S ruby",
                &[Root {
                    pid: 10,
                    owner: None,
                }],
                &[
                    Root {
                        pid: 12,
                        owner: Some("a".into()),
                    },
                    Root {
                        pid: 13,
                        owner: Some("b".into()),
                    },
                ],
                1,
            ),
            ..Default::default()
        };
        let sections = |owner| {
            snapshot
                .sections(owner)
                .into_iter()
                .map(|(title, rows)| (title, rows.iter().map(|r| r.pid).collect::<Vec<_>>()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sections(Some("a")),
            vec![
                ("This session", vec![12]),
                ("Other sessions", vec![13]),
                ("Shared", vec![11])
            ]
        );
        assert_eq!(sections(None), vec![("All sessions", vec![11, 12, 13])]);
    }

    #[test]
    fn markers_cannot_come_from_arguments_or_other_environment_values() {
        let tag = "12345678-1234-1234-1234-123456789abc";
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(
            b"/bin/python\0\0python\0-c\0DEL_PROCESS_TAG=argument-bait\0SECRET=not-retained\0",
        );
        bytes.extend_from_slice(format!("DEL_PROCESS_TAG={tag}\0OTHER=value\0").as_bytes());
        assert_eq!(marker_from_buffer(&bytes).as_deref(), Some(tag));
        assert_eq!(marker_from_buffer(&bytes[..10]), None);
    }

    #[test]
    fn shared_provider_children_and_detached_servers_have_exact_session_owners() {
        let rows = parse(
            "10 1 S del\n20 10 S codex\n21 20 S sh\n22 21 S node\n30 20 S sh\n31 30 S python\n40 1 S ruby\n50 1 S node",
            &[Root {
                pid: 20,
                owner: None,
            }],
            &[
                Root {
                    pid: 21,
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: 30,
                    owner: Some("chat:b".into()),
                },
                Root {
                    pid: 40,
                    owner: Some("chat:a".into()),
                },
            ],
            10,
        );
        let snapshot = Snapshot {
            rows,
            ..Default::default()
        };
        assert_eq!(
            snapshot
                .commands(Some("chat:a"))
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            [22, 40]
        );
        assert_eq!(
            snapshot
                .commands(Some("chat:b"))
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            [31]
        );
        assert_eq!(
            snapshot.commands(None).map(|p| p.pid).collect::<Vec<_>>(),
            [22, 31, 40]
        );
    }

    #[cfg(target_os = "macos")]
    struct ServerFixture {
        root: std::process::Child,
        server_pid: u32,
        port: u16,
    }
    #[cfg(target_os = "macos")]
    impl ServerFixture {
        fn start(tag: &str, ignore_term: bool) -> Self {
            use std::io::BufRead;
            use std::os::unix::process::CommandExt;
            // The wrapper and its child deliberately share a group. UI stopping
            // the child must not kill the wrapper or any other fixture.
            let child_script = format!(
                "import socket,os,signal,time\ns=socket.socket();s.bind(('127.0.0.1',0));s.listen()\nprint(str(os.getpid())+' '+str(s.getsockname()[1]),flush=True)\n{}\nwhile True: time.sleep(1)",
                if ignore_term {
                    "signal.signal(signal.SIGTERM,signal.SIG_IGN)"
                } else {
                    ""
                }
            );
            let script = format!(
                "import subprocess,time\np=subprocess.Popen(['python3','-u','-c',{}])\np.wait()\ntime.sleep(300)",
                serde_json::to_string(&child_script).unwrap()
            );
            let mut root = std::process::Command::new("python3")
                .args(["-u", "-c", &script])
                .env(del_agent::backend::PROCESS_TAG_ENV, tag)
                .process_group(0)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(root.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let values: Vec<_> = line.split_whitespace().collect();
            Self {
                root,
                server_pid: values[0].parse().unwrap(),
                port: values[1].parse().unwrap(),
            }
        }
        fn connects(&self) -> bool {
            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.port)).is_ok()
        }
    }
    #[cfg(target_os = "macos")]
    impl Drop for ServerFixture {
        fn drop(&mut self) {
            unsafe {
                libc::killpg(self.root.id() as i32, libc::SIGKILL);
            }
            let _ = self.root.wait();
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn real_servers_are_scoped_and_stop_does_not_touch_other_sessions() {
        let tag_a = del_agent::backend::new_process_tag();
        let tag_b = del_agent::backend::new_process_tag();
        let mut a = ServerFixture::start(&tag_a, false);
        let b = ServerFixture::start(&tag_b, false);
        let unrelated = ServerFixture::start(&del_agent::backend::new_process_tag(), false);
        let tags = BTreeMap::from([(tag_a, "chat:a".into()), (tag_b, "chat:b".into())]);
        let snapshot = read(
            vec![
                Root {
                    pid: a.root.id(),
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: b.root.id(),
                    owner: Some("chat:b".into()),
                },
            ],
            tags,
            Snapshot::default(),
        )
        .await;
        assert_eq!(
            snapshot
                .commands(Some("chat:a"))
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            [a.server_pid]
        );
        assert_eq!(
            snapshot
                .commands(Some("chat:b"))
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            [b.server_pid]
        );
        assert_eq!(snapshot.commands(None).count(), 2);
        let row = snapshot.commands(Some("chat:a")).next().unwrap().clone();
        assert!(row.ports.contains(&a.port.to_string()));
        let mut stale = row.clone();
        stale.identity.as_mut().unwrap().micros += 1;
        assert!(stop(stale).await.is_err());
        assert!(a.connects());
        stop(row).await.unwrap();
        assert!(!a.connects());
        assert!(
            a.root.try_wait().unwrap().is_none(),
            "provider wrapper must survive"
        );
        assert!(b.connects());
        assert!(unrelated.connects());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn detached_tagged_server_is_discovered_and_stubborn_tree_stops() {
        let tag = del_agent::backend::new_process_tag();
        let mut a = ServerFixture::start(&tag, true);
        unsafe {
            libc::kill(a.root.id() as i32, libc::SIGKILL);
        }
        a.root.wait().unwrap();
        // No previous observation, and no provider root: only the launch marker
        // can identify this server after its parent already exited.
        let snapshot = read(
            vec![],
            BTreeMap::from([(tag, "chat:a".into())]),
            Snapshot::default(),
        )
        .await;
        let row = snapshot
            .commands(Some("chat:a"))
            .find(|p| p.pid == a.server_pid)
            .unwrap()
            .clone();
        assert!(a.connects());
        stop(row).await.unwrap();
        for _ in 0..20 {
            if !a.connects() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!a.connects());
    }
    #[test]
    fn command_lists_exclude_shell_wrappers_and_app_housekeeping() {
        let rows = parse(
            "10 1 S del\n20 10 S claude\n21 20 S zsh\n22 21 S python3\n30 10 S codesign\n40 1 S node",
            &[Root {
                pid: 20,
                owner: Some("chat:a".into()),
            }],
            &[],
            10,
        );
        let commands: Vec<_> = rows
            .iter()
            .filter(|p| !p.background)
            .map(|p| p.pid)
            .collect();
        assert_eq!(
            commands,
            [22],
            "only the agent's command belongs in the list"
        );
    }
    #[test]
    fn shared_provider_is_unassigned_and_nested_terminal_owns_its_commands() {
        let rows = parse(
            "10 1 S /app/del\n20 10 S codex\n21 20 S node\n30 10 S zsh\n31 30 S python\n32 31 S worker\n40 20 S sh\n41 40 S node",
            &[
                Root {
                    pid: 20,
                    owner: None,
                },
                Root {
                    pid: 30,
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: 40,
                    owner: Some("chat:b".into()),
                },
            ],
            &[],
            10,
        );
        let owner = |pid| rows.iter().find(|r| r.pid == pid).unwrap().owner.as_deref();
        assert_eq!(owner(21), None);
        assert_eq!(owner(31), Some("chat:a"));
        assert_eq!(owner(32), Some("chat:a"));
        assert_eq!(owner(41), Some("chat:b"));
    }
    #[test]
    fn owns_only_del_roots_and_descendants_and_deduplicates_ports() {
        let mut rows = parse(
            "10 1 S /app/del\n11 10 S /bin/zsh\n12 11 R /bin/node\n13 12 S /bin/worker\n20 1 S /bin/private\n30 1 S /bin/claude\n31 30 S /bin/helper\n40 10 R /bin/ps\n41 10 Z (git)\n42 10 R (git)\n43 11 S /usr/bin/git\n44 30 S /bin/node\n45 44 S /bin/node_repl",
            &[
                Root {
                    pid: 30,
                    owner: Some("chat:a".into()),
                },
                Root {
                    pid: 11,
                    owner: Some("chat:b".into()),
                },
            ],
            &[],
            10,
        );
        assert_eq!(
            rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            [11, 12, 13, 30, 31, 43, 44, 45]
        );
        add_ports(
            &mut rows,
            "p12\nn127.0.0.1:3000\nn[::1]:3000\nn*:8080\np20\nn*:9999",
        );
        assert_eq!(rows[1].ports, ["3000", "8080"]);
        assert_eq!(rows[1].owner.as_deref(), Some("chat:b"));
        assert_eq!(rows[4].owner.as_deref(), Some("chat:a"));
        assert!(rows[0].background); // terminal shell
        assert!(rows[3].background); // provider connection
        assert!(!rows[1].background); // user/agent-started command
        assert_eq!(rows.iter().filter(|r| !r.background).count(), 4);
    }
}
