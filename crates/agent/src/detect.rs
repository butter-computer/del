//! Detect installed provider CLIs, their versions and login state.
//!
//! Only exit codes and the non-secret login flag are read; credentials are
//! never read, logged or stored.

use crate::types::Provider;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

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
    let (a, b) = tokio::join!(detect(Provider::Codex), detect(Provider::Claude));
    vec![a, b]
}

pub async fn detect(provider: Provider) -> ProviderStatus {
    let Some(path) = find_binary(provider.binary()) else {
        return ProviderStatus {
            provider,
            installed: false,
            path: None,
            version: None,
            login: LoginState::Unknown,
        };
    };
    let (version, login) = tokio::join!(probe_version(&path), probe_login(provider, &path));
    ProviderStatus {
        provider,
        installed: true,
        path: Some(path),
        version,
        login,
    }
}

/// Search `PATH` first, then common install locations (GUI apps often get a minimal PATH).
pub fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    dirs.extend(common_dirs());
    find_in(name, &dirs)
}

fn common_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
        .map(PathBuf::from)
        .into();
    if let Some(h) = dirs::home_dir() {
        for rel in [
            ".local/bin",
            ".claude/local",
            ".cargo/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".volta/bin",
        ] {
            v.push(h.join(rel));
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
    text: String,
}

async fn run(bin: &Path, args: &[&str]) -> Option<Output> {
    let child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let out = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(Output {
        ok: out.status.success(),
        text,
    })
}

async fn probe_version(bin: &Path) -> Option<String> {
    let out = run(bin, &["--version"]).await?;
    out.ok.then(|| parse_version(&out.text)).flatten()
}

async fn probe_login(provider: Provider, bin: &Path) -> LoginState {
    let args: &[&str] = match provider {
        Provider::Codex => &["login", "status"],
        Provider::Claude => &["auth", "status"],
    };
    match run(bin, args).await {
        Some(o) => parse_login(provider, o.ok, &o.text),
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

pub(crate) fn parse_login(provider: Provider, exit_ok: bool, text: &str) -> LoginState {
    let lower = text.to_lowercase();
    if provider == Provider::Claude {
        // Only the boolean is read from the JSON; everything else is discarded.
        if let Some(v) = text
            .find('{')
            .and_then(|i| serde_json::from_str::<serde_json::Value>(&text[i..]).ok())
        {
            return match v.get("loggedIn").and_then(|b| b.as_bool()) {
                Some(true) => LoginState::LoggedIn,
                Some(false) => LoginState::LoggedOut,
                None => LoginState::Unknown,
            };
        }
    }
    if lower.contains("not logged in") || lower.contains("not authenticated") {
        LoginState::LoggedOut
    } else if exit_ok && (lower.contains("logged in") || provider == Provider::Claude) {
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

    #[test]
    fn login_parsing() {
        use LoginState::*;
        assert_eq!(
            parse_login(Provider::Codex, true, "Logged in using ChatGPT"),
            LoggedIn
        );
        assert_eq!(
            parse_login(Provider::Codex, false, "Not logged in"),
            LoggedOut
        );
        assert_eq!(
            parse_login(Provider::Codex, true, "Not logged in"),
            LoggedOut
        );
        assert_eq!(parse_login(Provider::Codex, false, "boom"), LoggedOut);
        assert_eq!(parse_login(Provider::Codex, true, ""), Unknown);
        assert_eq!(
            parse_login(
                Provider::Claude,
                true,
                "{\"loggedIn\": true, \"email\": \"x\"}"
            ),
            LoggedIn
        );
        assert_eq!(
            parse_login(Provider::Claude, false, "{\"loggedIn\": false}"),
            LoggedOut
        );
        assert_eq!(
            parse_login(Provider::Claude, true, "{\"other\": 1}"),
            Unknown
        );
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
    async fn non_executable_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("codex"), "x").unwrap();
        assert!(find_in("codex", &[dir.path().to_path_buf()]).is_none());
    }
}
