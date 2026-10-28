//! Detect installed provider CLIs, their versions and login state.
//!
//! Only exit codes and the non-secret login flag are read; credentials are
//! never read, logged or stored.

use crate::types::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::process::Command;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// How long install locations and lookups are reused. A CLI installed while
/// the app runs is found after at most this long, or at the next detection.
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(30);

static COMMON_DIRS: Mutex<Option<(Instant, Arc<[PathBuf]>)>> = Mutex::new(None);
/// Binary name → when it was looked up and where it was found.
type Lookups = HashMap<String, (Instant, Option<PathBuf>)>;
static FOUND: Mutex<Option<Lookups>> = Mutex::new(None);

/// Forget cached install locations and lookups so the next search rescans.
pub fn refresh_search_cache() {
    *COMMON_DIRS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *FOUND.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginState {
    LoggedIn,
    LoggedOut,
    /// Not installed, probe failed or timed out.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: Provider,
    pub installed: bool,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub login: LoginState,
}

/// Probe every provider concurrently.
pub async fn detect_all() -> Vec<ProviderStatus> {
    refresh_search_cache();
    let (a, b, c) = tokio::join!(
        detect(Provider::Codex),
        detect(Provider::Claude),
        detect(Provider::OpenCode)
    );
    vec![a, b, c]
}

/// `Provider::Acp` has no single CLI; it always reports not installed here and
/// `del-acp` supplies the real status from its agent registry.
pub async fn detect(provider: Provider) -> ProviderStatus {
    let found = if provider == Provider::Acp {
        None
    } else {
        find_working_binary(provider.binary()).await
    };
    let Some((path, version)) = found else {
        return ProviderStatus {
            provider,
            installed: false,
            path: None,
            version: None,
            login: LoginState::Unknown,
        };
    };
    let login = probe_login(provider, &path).await;
    ProviderStatus {
        provider,
        installed: true,
        path: Some(path),
        version: Some(version),
        login,
    }
}

/// Search `PATH` first, then common install locations (GUI apps often get a minimal PATH).
/// Results are cached briefly; a cached hit is rechecked so removal is noticed at once.
pub fn find_binary(name: &str) -> Option<PathBuf> {
    {
        let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, path)) = found.get_or_insert_with(HashMap::new).get(name)
            && at.elapsed() < SEARCH_CACHE_TTL
            && path.as_deref().is_none_or(is_executable)
        {
            return path.clone();
        }
    }
    let path = find_in(name, &search_dirs());
    FOUND
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(name.to_owned(), (Instant::now(), path.clone()));
    path
}

/// Absolute `PATH` entries, then common install locations. Relative entries
/// (`.`, `node_modules/.bin`) would resolve against an untrusted project.
fn search_dirs() -> Vec<PathBuf> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path_var)
        .filter(|dir| dir.is_absolute())
        .collect();
    dirs.extend(common_dirs().iter().cloned());
    dirs
}

/// A launcher may be executable even when its bundled binary is missing.
/// Use the first installation that actually answers the version probe.
pub async fn find_working_binary(name: &str) -> Option<(PathBuf, String)> {
    find_working_in(name, &search_dirs()).await
}

async fn find_working_in(name: &str, dirs: &[PathBuf]) -> Option<(PathBuf, String)> {
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let path = dir.join(name);
        if is_executable(&path)
            && seen.insert(path.clone())
            && let Some(version) = probe_version(&path).await
        {
            return Some((path, version));
        }
    }
    None
}

/// Common install locations, rescanned at most every [`SEARCH_CACHE_TTL`].
pub(crate) fn common_dirs() -> Arc<[PathBuf]> {
    let mut cached = COMMON_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, dirs)) = cached.as_ref()
        && at.elapsed() < SEARCH_CACHE_TTL
    {
        return dirs.clone();
    }
    let dirs: Arc<[PathBuf]> = scan_common_dirs().into();
    *cached = Some((Instant::now(), dirs.clone()));
    dirs
}

fn scan_common_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
        .map(PathBuf::from)
        .into();
    if let Some(h) = dirs::home_dir() {
        for rel in [
            ".local/bin",
            ".opencode/bin",
            ".claude/local",
            ".cargo/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".volta/bin",
        ] {
            v.push(h.join(rel));
        }
        let nvm = std::env::var_os("NVM_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| h.join(".nvm"));
        if let Ok(entries) = std::fs::read_dir(nvm.join("versions/node")) {
            let mut versions: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
            versions.sort_by_cached_key(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .trim_start_matches('v')
                    .split('.')
                    .map(|part| part.parse::<u32>().unwrap_or(0))
                    .collect::<Vec<_>>()
            });
            v.extend(versions.into_iter().rev().map(|path| path.join("bin")));
        }
    }
    v
}

pub(crate) fn find_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

struct Output {
    ok: bool,
    stdout: String,
    /// stdout followed by stderr.
    text: String,
}

async fn run(bin: &Path, args: &[&str]) -> Option<Output> {
    let mut command = Command::new(bin);
    crate::environment::prepare_provider_command(command.as_std_mut());
    // Own group: a Node wrapper's native grandchild must die with it on timeout.
    let child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let pid = child.id();
    let output = child.wait_with_output();
    tokio::pin!(output);
    let out = match tokio::time::timeout(PROBE_TIMEOUT, &mut output).await {
        Ok(out) => out.ok()?,
        Err(_) => {
            // The leader is still unreaped (owned by `output`), so its group id is ours.
            if let Some(pid) = pid {
                crate::procgroup::signal_group(pid, true);
            }
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut text = stdout.clone();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(Output {
        ok: out.status.success(),
        stdout,
        text,
    })
}

async fn probe_version(bin: &Path) -> Option<String> {
    let out = run(bin, &["--version"]).await?;
    out.ok
        .then(|| parse_version(&out.stdout).or_else(|| parse_version(&out.text)))
        .flatten()
}

async fn probe_login(provider: Provider, bin: &Path) -> LoginState {
    let args: &[&str] = match provider {
        Provider::Codex => &["login", "status"],
        Provider::Claude => &["auth", "status"],
        Provider::Acp | Provider::OpenCode => return LoginState::Unknown,
    };
    match run(bin, args).await {
        Some(o) => parse_login(provider, o.ok, &o.stdout, &o.text),
        None => LoginState::Unknown,
    }
}

/// First dotted-number token, e.g. `codex-cli 0.162.1` → `0.162.1`.
pub(crate) fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|t| t.trim_start_matches('v'))
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('.'))
        .map(str::to_owned)
}

/// `stdout` alone carries Claude's JSON; `text` is stdout followed by stderr.
pub(crate) fn parse_login(
    provider: Provider,
    exit_ok: bool,
    stdout: &str,
    text: &str,
) -> LoginState {
    let lower = text.to_lowercase();
    if provider == Provider::Claude {
        // Only the boolean is read from the first JSON value; everything else is discarded.
        if let Some(v) = stdout.find('{').and_then(|i| {
            serde_json::Deserializer::from_str(&stdout[i..])
                .into_iter::<serde_json::Value>()
                .next()?
                .ok()
        }) {
            return match v.get("loggedIn").and_then(|b| b.as_bool()) {
                Some(true) => LoginState::LoggedIn,
                Some(false) => LoginState::LoggedOut,
                None => LoginState::Unknown,
            };
        }
    }
    if lower.contains("not logged in") || lower.contains("not authenticated") {
        LoginState::LoggedOut
    } else if exit_ok && lower.contains("logged in") {
        LoginState::LoggedIn
    } else if !exit_ok {
        LoginState::LoggedOut
    } else {
        LoginState::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn versions() {
        assert_eq!(
            parse_version("codex-cli 0.162.1\n").as_deref(),
            Some("0.162.1")
        );
        assert_eq!(
            parse_version("2.1.308 (Claude Code)").as_deref(),
            Some("2.1.308")
        );
        assert_eq!(parse_version("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(parse_version("no digits"), None);
    }

    fn login(provider: Provider, ok: bool, text: &str) -> LoginState {
        parse_login(provider, ok, text, text)
    }

    #[test]
    fn claude_login_ignores_stderr_and_trailing_output() {
        use LoginState::*;
        let stdout = "{\"loggedIn\": false}\n{\"extra\": 1}\n";
        let text = format!("{stdout}Warning: {{noise}} logged in elsewhere\n");
        assert_eq!(
            parse_login(Provider::Claude, true, stdout, &text),
            LoggedOut
        );
        // No JSON and no explicit text: exit 0 alone is not proof of a login.
        assert_eq!(
            parse_login(Provider::Claude, true, "", "{bad json"),
            Unknown
        );
    }

    #[test]
    fn search_skips_relative_path_entries() {
        assert!(search_dirs().iter().all(|dir| dir.is_absolute()));
        assert!(Arc::ptr_eq(&common_dirs(), &common_dirs()));
    }

    #[test]
    fn login_parsing() {
        use LoginState::*;
        assert_eq!(
            login(Provider::Codex, true, "Logged in using ChatGPT"),
            LoggedIn
        );
        assert_eq!(login(Provider::Codex, false, "Not logged in"), LoggedOut);
        assert_eq!(login(Provider::Codex, true, "Not logged in"), LoggedOut);
        assert_eq!(login(Provider::Codex, false, "boom"), LoggedOut);
        assert_eq!(login(Provider::Codex, true, ""), Unknown);
        assert_eq!(
            login(
                Provider::Claude,
                true,
                "{\"loggedIn\": true, \"email\": \"x\"}"
            ),
            LoggedIn
        );
        assert_eq!(
            login(Provider::Claude, false, "{\"loggedIn\": false}"),
            LoggedOut
        );
        assert_eq!(login(Provider::Claude, true, "{\"other\": 1}"), Unknown);
    }

    #[tokio::test]
    async fn finds_and_probes_a_fake_cli() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        std::fs::write(&bin, "#!/bin/sh\ncase \"$1\" in --version) echo 'codex-cli 9.9.9';; login) echo 'Logged in using ChatGPT';; esac\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = find_in(
            "codex",
            &[PathBuf::from("/nonexistent"), dir.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(found, bin);
        assert_eq!(probe_version(&found).await.as_deref(), Some("9.9.9"));
        assert_eq!(
            probe_login(Provider::Codex, &found).await,
            LoginState::LoggedIn
        );
        assert!(find_in("claude", &[dir.path().to_path_buf()]).is_none());
    }

    #[tokio::test]
    async fn detects_provider_with_runtime_in_its_install_directory() {
        let dir = tempfile::tempdir().unwrap();
        for (name, script) in [
            ("codex", "#!/usr/bin/env del-detection-fixture-runtime\n"),
            (
                "del-detection-fixture-runtime",
                "#!/bin/sh\necho 'codex-cli 9.9.9'\n",
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            find_working_in("codex", &[dir.path().to_owned()]).await,
            Some((dir.path().join("codex"), "9.9.9".into()))
        );
    }

    #[tokio::test]
    async fn non_executable_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("codex"), "x").unwrap();
        assert!(find_in("codex", &[dir.path().to_path_buf()]).is_none());
    }

    #[tokio::test]
    async fn skips_broken_launcher_before_working_installation() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken");
        let working = dir.path().join("working");
        for (folder, script) in [
            (&broken, "#!/bin/sh\nexit 1\n"),
            (&working, "#!/bin/sh\necho 'codex-cli 1.2.3'\n"),
        ] {
            std::fs::create_dir(folder).unwrap();
            let bin = folder.join("codex");
            std::fs::write(&bin, script).unwrap();
            std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = find_working_in("codex", &[broken, working.clone()])
            .await
            .unwrap();
        assert_eq!(found, (working.join("codex"), "1.2.3".into()));
    }
}
