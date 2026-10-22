//! Provider setup: is each CLI installed, which version, and is the user
//! signed in. Also the model choices offered per provider.
//!
//! del never reads or stores credentials. Sign-in status comes from the CLI's
//! own status command (exit code and output), and signing in is the CLI's own
//! login flow running in the user's terminal.

use del_agent::Provider;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    LoggedIn,
    LoggedOut,
    /// The status command failed or printed something unexpected.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub provider: Provider,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub auth: Auth,
}

impl ProviderStatus {
    pub fn pending(provider: Provider) -> Self {
        Self { provider, path: None, version: None, auth: Auth::Unknown }
    }

    pub fn installed(&self) -> bool {
        self.path.is_some()
    }

    pub fn ready(&self) -> bool {
        self.installed() && self.auth == Auth::LoggedIn
    }
}

pub fn display_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
    }
}

/// Model choices; `None` is the provider's own default.
pub fn models(provider: Provider) -> &'static [Option<&'static str>] {
    match provider {
        Provider::Codex => &[None, Some("gpt-5-codex"), Some("gpt-5")],
        Provider::Claude => &[None, Some("sonnet"), Some("opus"), Some("haiku")],
    }
}

pub fn model_label(model: Option<&str>) -> String {
    model.unwrap_or("Default").to_string()
}

/// The model after `current` in the provider's list, wrapping around.
pub fn next_model(provider: Provider, current: Option<&str>) -> Option<String> {
    let list = models(provider);
    let at = list.iter().position(|m| *m == current).map_or(0, |i| i + 1);
    list[at % list.len()].map(str::to_string)
}

pub fn next_provider(provider: Provider) -> Provider {
    match provider {
        Provider::Codex => Provider::Claude,
        Provider::Claude => Provider::Codex,
    }
}

/// The command the user runs to sign in, shown in the panel.
pub fn login_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex login",
        Provider::Claude => "claude auth login",
    }
}

pub fn install_hint(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Install: npm i -g @openai/codex  (or brew install codex)",
        Provider::Claude => "Install: see claude.com/claude-code",
    }
}

/// Find the CLI. A GUI app often has a shorter PATH than a shell, so also try
/// the usual install locations.
pub fn find_binary(provider: Provider) -> Option<PathBuf> {
    let name = provider.binary();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".claude/local"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".bun/bin"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// First non-empty line, trimmed. `codex-cli 0.162.1` / `2.1.308 (Claude Code)`.
pub fn parse_version(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Claude: `claude auth status` prints JSON with a `loggedIn` boolean.
pub fn parse_claude_auth(output: &str) -> Auth {
    match serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|v| v.get("loggedIn").and_then(|b| b.as_bool()))
    {
        Some(true) => Auth::LoggedIn,
        Some(false) => Auth::LoggedOut,
        None => Auth::Unknown,
    }
}

/// Codex: `codex login status` exits 0 and says "Logged in ..." when signed
/// in, and exits non-zero otherwise.
pub fn parse_codex_auth(success: bool, output: &str) -> Auth {
    let text = output.to_lowercase();
    if success && text.contains("logged in") {
        Auth::LoggedIn
    } else if text.contains("not logged in") || !success {
        Auth::LoggedOut
    } else {
        Auth::Unknown
    }
}

async fn run(path: &PathBuf, args: &[&str]) -> Option<(bool, String)> {
    let child = Command::new(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(10), child).await.ok()?.ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

/// Check one provider. Runs only the CLI's version and status commands.
pub async fn detect(provider: Provider) -> ProviderStatus {
    let Some(path) = find_binary(provider) else {
        return ProviderStatus::pending(provider);
    };
    let version = run(&path, &["--version"])
        .await
        .and_then(|(_, out)| parse_version(&out));
    let auth = match provider {
        Provider::Codex => match run(&path, &["login", "status"]).await {
            Some((ok, out)) => parse_codex_auth(ok, &out),
            None => Auth::Unknown,
        },
        Provider::Claude => match run(&path, &["auth", "status"]).await {
            Some((_, out)) => parse_claude_auth(&out),
            None => Auth::Unknown,
        },
    };
    ProviderStatus { provider, path: Some(path), version, auth }
}

/// Open the CLI's official login in the user's terminal (macOS Terminal).
/// The login flow, browser step and token storage are entirely the CLI's.
pub fn launch_login(provider: Provider, path: &std::path::Path) -> std::io::Result<()> {
    let command = format!("'{}' {}", path.display(), login_command(provider).split_once(' ').map_or("", |x| x.1));
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_takes_first_line() {
        assert_eq!(parse_version("\n codex-cli 0.162.1\nmore"), Some("codex-cli 0.162.1".into()));
        assert_eq!(parse_version("  \n"), None);
    }

    #[test]
    fn claude_auth_json() {
        assert_eq!(parse_claude_auth(r#"{"loggedIn": true, "authMethod":"claude.ai"}"#), Auth::LoggedIn);
        assert_eq!(parse_claude_auth(r#"{"loggedIn": false}"#), Auth::LoggedOut);
        assert_eq!(parse_claude_auth("oops"), Auth::Unknown);
    }

    #[test]
    fn codex_auth_text() {
        assert_eq!(parse_codex_auth(true, "Logged in using ChatGPT"), Auth::LoggedIn);
        assert_eq!(parse_codex_auth(false, "Not logged in"), Auth::LoggedOut);
        assert_eq!(parse_codex_auth(true, "???"), Auth::Unknown);
    }

    #[test]
    fn model_cycle_wraps_and_starts_from_default() {
        assert_eq!(next_model(Provider::Claude, None), Some("sonnet".into()));
        assert_eq!(next_model(Provider::Claude, Some("haiku")), None);
        assert_eq!(next_model(Provider::Claude, Some("unknown")), None);
    }

    #[test]
    fn provider_cycle_alternates() {
        assert_eq!(next_provider(next_provider(Provider::Codex)), Provider::Codex);
    }
}
