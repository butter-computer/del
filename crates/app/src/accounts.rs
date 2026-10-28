//! Named-by-identity account profiles; provider CLIs own credentials and login flows.
use del_agent::{AgentBackend, Provider};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub number: usize,
    #[serde(default)]
    pub identity: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub requires_login: bool,
}
pub fn identity_key(identity: &str) -> Option<String> {
    let identity = identity.trim();
    let (name, domain) = identity.split_once('@')?;
    (!name.is_empty() && !domain.is_empty()).then(|| identity.to_lowercase())
}
pub fn display_identity(identity: &str) -> String {
    identity
        .split_once('@')
        .map_or(identity, |(name, _)| name)
        .to_owned()
}
impl Account {
    pub fn display(&self) -> String {
        self.identity
            .as_deref()
            .map(display_identity)
            .unwrap_or_else(|| format!("Account {}", self.number))
    }
    pub fn root(&self) -> PathBuf {
        del_agent::data_dir()
            .expect("account storage available")
            .join("accounts")
            .join(&self.id)
    }
    pub fn environment(&self) -> Vec<(String, String)> {
        let root = self.root();
        let mut env = vec![("DEL_ACCOUNT_PROFILE".into(), "1".into())];
        match self.provider {
            Provider::Claude => {
                env.push(("CLAUDE_CONFIG_DIR".into(), root.to_string_lossy().into()))
            }
            Provider::Codex => env.push(("CODEX_HOME".into(), root.to_string_lossy().into())),
            Provider::OpenCode => {
                for (key, sub) in [
                    ("XDG_DATA_HOME", "data"),
                    ("XDG_CONFIG_HOME", "config"),
                    ("XDG_STATE_HOME", "state"),
                    ("XDG_CACHE_HOME", "cache"),
                    ("OPENCODE_CONFIG_DIR", "config/opencode"),
                ] {
                    env.push((key.into(), root.join(sub).to_string_lossy().into()));
                }
                env.push((
                    "OPENCODE_DB".into(),
                    root.join("data/opencode/opencode.db")
                        .to_string_lossy()
                        .into(),
                ));
            }
            Provider::Acp => {}
        }
        env
    }
    pub fn backend(&self) -> Arc<dyn AgentBackend> {
        match self.provider {
            Provider::Codex => {
                let mut options = del_codex::CodexOptions {
                    api_key: None,
                    ..Default::default()
                };
                options.spawn.env = self.environment();
                options
                    .spawn
                    .global_args
                    .extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
                Arc::new(del_codex::CodexBackend::new(options))
            }
            Provider::Claude => {
                Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
                    config_dir: Some(self.root()),
                    ..Default::default()
                }))
            }
            Provider::OpenCode => Arc::new(del_opencode::OpenCodeBackend::with_environment(
                self.environment(),
            )),
            Provider::Acp => Arc::new(del_acp::AcpBackend::default()),
        }
    }
}
/// Copy only this conversation's native data, never credentials or account configuration.
pub async fn transfer_conversation(
    record: del_agent::SessionRecord,
    target: Option<Account>,
) -> del_agent::Result<Option<PathBuf>> {
    let source = for_record(&record);
    if record.provider == Provider::OpenCode {
        return transfer_opencode(&record, source.as_ref(), target.as_ref())
            .await
            .map(|_| None);
    }
    tokio::task::spawn_blocking(move || {
        let default_root = || match record.provider {
            Provider::Codex => crate::terminal_conversation::codex_root(),
            _ => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude")),
        };
        let from = source.as_ref().map(Account::root).unwrap_or_else(default_root);
        let to = target.as_ref().map(Account::root).unwrap_or_else(default_root);
        let find = |root: &Path| match record.provider {
            Provider::Codex => crate::terminal_conversation::rollouts(root).into_iter().find(|p|
                p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&format!("{}.jsonl", record.id.0)))),
            Provider::Claude => del_claude::history::path_from(root, &record.cwd, &record.id.0),
            _ => None,
        };
        let path = find(&from).ok_or_else(|| std::io::Error::other("The provider has not saved this conversation yet. Send a message before switching accounts."))?;
        let relative = path.strip_prefix(&from).map_err(std::io::Error::other)?;
        let destination = find(&to).unwrap_or_else(|| to.join(relative));
        copy_transcript(&path, &destination)?;
        // Claude keeps subagent transcripts alongside the main conversation.
        if record.provider == Provider::Claude {
            let children = path.with_extension("");
            if children.is_dir() { copy_transcript_tree(&children, &destination.with_extension(""))?; }
        }
        Ok(Some(destination))
    }).await.map_err(|e| del_agent::AgentError::Backend(e.to_string()))?
}
fn copy_transcript(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let parent = to
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing transcript directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&std::fs::read(from)?)?;
    temporary.persist(to).map_err(|e| e.error)?;
    Ok(())
}
fn copy_transcript_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_transcript_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            copy_transcript(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}
async fn opencode_transfer_command(
    binary: &Path,
    args: &[&str],
    cwd: &Path,
    account: Option<&Account>,
) -> del_agent::Result<std::process::Output> {
    let mut command = tokio::process::Command::new(binary);
    if let Some(account) = account {
        command.envs(account.environment());
    }
    command
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    del_agent::environment::prepare_provider_command(command.as_std_mut());
    command.process_group(0);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn()?;
    let pid = child.id();
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output()).await;
    if output.is_err() {
        if let Some(pid) = pid {
            del_agent::procgroup::signal_group(pid, true);
        }
        return Err(del_agent::AgentError::Backend(
            "OpenCode conversation transfer timed out".into(),
        ));
    }
    let output = output.unwrap()?;
    if !output.status.success() {
        return Err(del_agent::AgentError::Backend("OpenCode could not transfer this conversation. The original account is still selected.".into()));
    }
    Ok(output)
}
async fn transfer_opencode(
    record: &del_agent::SessionRecord,
    source: Option<&Account>,
    target: Option<&Account>,
) -> del_agent::Result<()> {
    use std::io::Write;
    let binary = login_binary(Provider::OpenCode)
        .await
        .map_err(del_agent::AgentError::Backend)?;
    let bytes = opencode_transfer_command(
        &binary,
        &["session", "export", "--standalone", record.id.as_str()],
        &record.cwd,
        source,
    )
    .await?
    .stdout;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| del_agent::AgentError::Backend(e.to_string()))?;
    if !value.is_object() {
        return Err(del_agent::AgentError::Backend(
            "Invalid OpenCode conversation export".into(),
        ));
    }
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(&bytes)?;
    let import_args = [
        "session",
        "import",
        "--standalone",
        file.path()
            .to_str()
            .ok_or_else(|| del_agent::AgentError::Backend("Invalid temporary path".into()))?,
    ];
    let imported = opencode_transfer_command(&binary, &import_args, &record.cwd, target).await?;
    if String::from_utf8_lossy(&imported.stderr).contains("Session already exists") {
        // Native import refuses to overwrite. Keep a private recovery export before
        // replacing the stale copy left by an earlier account switch.
        let backup_bytes = opencode_transfer_command(
            &binary,
            &["session", "export", "--standalone", record.id.as_str()],
            &record.cwd,
            target,
        )
        .await?
        .stdout;
        let mut backup = tempfile::NamedTempFile::new()?;
        backup.write_all(&backup_bytes)?;
        opencode_transfer_command(
            &binary,
            &["session", "delete", "--standalone", record.id.as_str()],
            &record.cwd,
            target,
        )
        .await?;
        if let Err(error) =
            opencode_transfer_command(&binary, &import_args, &record.cwd, target).await
        {
            let restored = opencode_transfer_command(
                &binary,
                &[
                    "session",
                    "import",
                    "--standalone",
                    backup.path().to_str().unwrap(),
                ],
                &record.cwd,
                target,
            )
            .await;
            if restored.is_err() {
                let (_, path) = backup
                    .keep()
                    .map_err(|e| del_agent::AgentError::Io(e.error))?;
                return Err(del_agent::AgentError::Backend(format!(
                    "{error}; recovery export retained at {}",
                    path.display()
                )));
            }
            return Err(error);
        }
    }
    Ok(())
}

fn registry() -> std::io::Result<PathBuf> {
    del_agent::data_dir()
        .map(|root| root.join("accounts.json"))
        .ok_or_else(|| std::io::Error::other("Account storage directory is unavailable"))
}
/// Set once `accounts.json` could not be read. Saving would replace the file
/// with only the accounts known to this run and orphan every other credentialed
/// profile, so account changes are refused until the file is repaired.
static SAVE_BLOCKED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn load() -> std::io::Result<Vec<Account>> {
    load_from(&registry()?, &SAVE_BLOCKED)
}
fn parse_registry(bytes: &[u8]) -> std::io::Result<Vec<Account>> {
    let mut accounts: Vec<Account> = serde_json::from_slice(bytes)?;
    for provider in Provider::ALL {
        if accounts
            .iter()
            .any(|a| a.provider == provider && a.number == 1)
        {
            for account in accounts.iter_mut().filter(|a| a.provider == provider) {
                account.number += 1;
            }
        }
    }
    if accounts
        .iter()
        .any(|a| uuid::Uuid::parse_str(&a.id).is_err())
    {
        return Err(std::io::Error::other("Invalid account profile ID"));
    }
    Ok(accounts)
}
fn load_from(
    path: &Path,
    blocked: &std::sync::Mutex<Option<String>>,
) -> std::io::Result<Vec<Account>> {
    let error = match std::fs::read(path) {
        Ok(bytes) => match parse_registry(&bytes) {
            Ok(accounts) => return Ok(accounts),
            Err(error) => error,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => error,
    };
    let mut blocked = blocked.lock().unwrap_or_else(|e| e.into_inner());
    if blocked.is_none() {
        // Keep the original in place (so a restart still refuses to clobber it) plus a dated copy.
        let copy = if path.exists() {
            match crate::sessions::preserve_corrupt(path, true) {
                Ok(aside) => format!(" A copy was saved as {}.", aside.display()),
                Err(copy_error) => format!(" It could not be copied aside: {copy_error}."),
            }
        } else {
            String::new()
        };
        *blocked = Some(format!(
            "{} could not be read: {error}.{copy} Account changes will not be saved until it is repaired or removed and Del is restarted.",
            path.display()
        ));
    }
    Err(std::io::Error::other(blocked.clone().unwrap_or_default()))
}
pub fn save(accounts: &[Account]) -> std::io::Result<()> {
    save_to(&registry()?, accounts, &SAVE_BLOCKED)
}
fn save_to(
    path: &Path,
    accounts: &[Account],
    blocked: &std::sync::Mutex<Option<String>>,
) -> std::io::Result<()> {
    if let Some(reason) = blocked.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Err(std::io::Error::other(reason));
    }
    crate::sessions::write_atomic(
        path,
        &serde_json::to_vec_pretty(accounts)?,
        crate::sessions::PRIVATE,
    )
}
/// Removed profiles stay recoverable this long before startup deletes them.
pub const REMOVED_RETENTION: std::time::Duration =
    std::time::Duration::from_secs(30 * 24 * 60 * 60);
pub const REMOVED_AT: &str = ".del-removed-at";

/// Move a deleted profile into `removed-accounts/`, stamped for later purging.
pub fn retire_profile(source: &Path, removed: &Path, id: &str) -> std::io::Result<Option<PathBuf>> {
    if !source.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(removed)?;
    let destination = removed.join(format!("{id}-{}", uuid::Uuid::new_v4()));
    std::fs::rename(source, &destination)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = std::fs::write(destination.join(REMOVED_AT), now.to_string());
    Ok(Some(destination))
}

/// Delete recovery copies (with their credentials) older than `retention`.
/// Only `<uuid>-<uuid>` directories Del created are considered.
pub fn purge_removed_profiles(removed: &Path, retention: std::time::Duration) -> usize {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(removed) else {
        return 0;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut purged = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.len() == 73
            && name.as_bytes()[36] == b'-'
            && uuid::Uuid::parse_str(&name[..36]).is_ok()
            && uuid::Uuid::parse_str(&name[37..]).is_ok();
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !ours || !meta.file_type().is_dir() {
            continue;
        }
        // Older removals have no stamp; their rename into place updated ctime.
        let removed_at = std::fs::read_to_string(entry.path().join(REMOVED_AT))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or_else(|| meta.ctime().max(0) as u64);
        if now.saturating_sub(removed_at) >= retention.as_secs()
            && std::fs::remove_dir_all(entry.path()).is_ok()
        {
            purged += 1;
        }
    }
    purged
}
fn next_number(provider: Provider, accounts: &[Account]) -> usize {
    accounts
        .iter()
        .filter(|a| a.provider == provider)
        .map(|a| a.number)
        .max()
        .unwrap_or(1)
        .max(1)
        + 1
}
pub fn create(provider: Provider, accounts: &[Account]) -> std::io::Result<Account> {
    use std::os::unix::fs::PermissionsExt;
    let account = Account {
        id: uuid::Uuid::new_v4().to_string(),
        provider,
        number: next_number(provider, accounts),
        identity: None,
        is_default: false,
        requires_login: true,
    };
    let root = account.root();
    std::fs::create_dir_all(&root)?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    if provider == Provider::OpenCode {
        std::fs::create_dir_all(root.join("data/opencode"))?;
    }
    Ok(account)
}
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
pub async fn login_binary(provider: Provider) -> Result<PathBuf, String> {
    del_agent::detect::find_working_binary(provider.binary()).await
        .map(|(path, _)| path)
        .ok_or_else(|| format!("No working {} installation found. Repair or reinstall its CLI, then try Sign in again.", crate::providers::display_name(provider)))
}
fn login_command(binary: &Path, args: &str, environment: &str, marker: &Path) -> String {
    let mut probe = std::process::Command::new(binary);
    del_agent::environment::prepare_provider_command(&mut probe);
    let path = probe
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .unwrap_or_default()
        .to_string_lossy();
    format!(
        "env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN -u OPENAI_API_KEY -u CODEX_API_KEY -u OPENAI_ACCESS_TOKEN -u OPENAI_IDENTITY_TOKEN_FILE -u OPENAI_IDENTITY_PROVIDER_ID PATH={} {environment} {} {args}; del_login_exit=$?; printf '%s' \"$del_login_exit\" > {}",
        quote(&path),
        quote(&binary.to_string_lossy()),
        quote(&marker.to_string_lossy())
    )
}
fn write_login_script(marker: &Path, command: &str) -> std::io::Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = marker.with_extension("sh");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let cancelled = format!(
        "printf '130' > {}; exit 130",
        quote(&marker.to_string_lossy())
    );
    writeln!(
        file,
        "#!/bin/sh\ntrap 'rm -f -- \"$0\"' EXIT\ntrap {} INT TERM\n{command}",
        quote(&cancelled)
    )?;
    Ok(format!("/bin/sh {}", quote(&path.to_string_lossy())))
}
pub fn launch_login(account: &Account, binary: &Path) -> std::io::Result<PathBuf> {
    launch_provider_login(account.provider, Some(account), binary)
}
pub fn launch_provider_login(
    provider: Provider,
    account: Option<&Account>,
    binary: &Path,
) -> std::io::Result<PathBuf> {
    let root = account
        .map(Account::root)
        .or_else(|| del_agent::data_dir().map(|p| p.join("login")))
        .ok_or_else(|| std::io::Error::other("Login storage unavailable"))?;
    std::fs::create_dir_all(&root)?;
    let marker = root.join(format!("login-{}", uuid::Uuid::new_v4()));
    let env = account
        .map(Account::environment)
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| format!("{key}={}", quote(&value)))
        .collect::<Vec<_>>()
        .join(" ");
    let args = match provider {
        Provider::Codex if account.is_none() => "login",
        Provider::Codex => "-c 'cli_auth_credentials_store=\"file\"' login",
        Provider::Claude => "auth login",
        Provider::OpenCode => "auth login --standalone",
        Provider::Acp => {
            return Err(std::io::Error::other(
                "ACP accounts are managed by their agent",
            ));
        }
    };
    let command = write_login_script(&marker, &login_command(binary, args, &env, &marker))?;
    let script = format!(
        "tell application \"Terminal\"\nactivate\nset loginTab to do script \"{}\"\nreturn tty of loginTab\nend tell",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()?;
    if output.status.success() {
        if let Err(error) = std::fs::write(marker.with_extension("tty"), &output.stdout) {
            cleanup_login(&marker);
            return Err(error);
        }
        Ok(marker)
    } else {
        cleanup_login(&marker);
        Err(std::io::Error::other(
            "Could not open provider sign-in in Terminal",
        ))
    }
}

/// Remove every file a sign-in attempt created: its exit marker, the Terminal
/// tty record and (if the script never ran) the launcher script.
pub fn cleanup_login(marker: &Path) {
    for path in [
        marker.to_path_buf(),
        marker.with_extension("tty"),
        marker.with_extension("sh"),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// Only close the idle, single-tab login window created by this attempt.
pub fn close_login_terminal(marker: &Path) {
    let path = marker.with_extension("tty");
    let Ok(tty) = std::fs::read_to_string(&path) else {
        return;
    };
    let tty = tty.trim();
    if !tty.starts_with("/dev/") || !tty.chars().all(|c| c.is_ascii_alphanumeric() || c == '/') {
        return;
    }
    let script = format!(
        "tell application \"Terminal\"\nrepeat with w in windows\nif (count of tabs of w) is 1 then\nset t to first tab of w\nif tty of t is \"{tty}\" and busy of t is false then\nclose w saving no\nexit repeat\nend if\nend if\nend repeat\nend tell"
    );
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output();
    let _ = std::fs::remove_file(path);
}

pub async fn remaining_usage(provider: Provider, account: Option<&Account>) -> Option<f64> {
    if provider == Provider::Claude {
        let binary = login_binary(provider).await.ok()?;
        let options = del_claude::ClaudeOptions {
            binary,
            config_dir: account.map(Account::root),
            setting_sources: Some(String::new()),
            ..Default::default()
        };
        let value = del_claude::account_usage(&options, &std::env::temp_dir())
            .await
            .ok()?;
        return claude_usage_remaining(&value);
    }
    if provider != Provider::Codex {
        return None;
    }
    let Ok(binary) = login_binary(Provider::Codex).await else {
        return None;
    };
    let mut options = del_codex::CodexOptions {
        api_key: None,
        ..Default::default()
    };
    options.spawn.program = binary;
    if let Some(account) = account {
        options.spawn.env = account.environment();
        options
            .spawn
            .global_args
            .extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
    }
    let backend = del_codex::CodexBackend::new(options);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        backend.account_rate_limits(),
    )
    .await;
    let _ = backend.shutdown().await;
    match result {
        Ok(Ok(value)) => usage_remaining(&value),
        _ => None,
    }
}
/// A single bar represents the most constrained reported allowance.
fn usage_remaining(value: &serde_json::Value) -> Option<f64> {
    let buckets: Vec<_> = match value["rateLimitsByLimitId"]
        .as_object()
        .filter(|map| !map.is_empty())
    {
        Some(map) => map.values().collect(),
        None => vec![&value["rateLimits"]],
    };
    buckets
        .into_iter()
        .flat_map(|bucket| ["primary", "secondary"].map(|key| &bucket[key]))
        .filter_map(|window| window["usedPercent"].as_f64().filter(|n| n.is_finite()))
        .map(|used| (100. - used).clamp(0., 100.))
        .reduce(f64::min)
}

/// Plan windows use percentages (not the stream event's 0–1 utilization).
fn claude_usage_remaining(value: &serde_json::Value) -> Option<f64> {
    if value["rate_limits_available"] != true {
        return None;
    }
    let limits = &value["rate_limits"];
    [
        "five_hour",
        "seven_day",
        "seven_day_oauth_apps",
        "seven_day_opus",
        "seven_day_sonnet",
    ]
    .into_iter()
    .map(|key| &limits[key])
    .chain(limits["model_scoped"].as_array().into_iter().flatten())
    .filter_map(|window| window["utilization"].as_f64().filter(|n| n.is_finite()))
    .map(|used| (100. - used).clamp(0., 100.))
    .reduce(f64::min)
}

/// Return only non-secret identity information, never token strings or raw CLI output.
/// A sign-in check that did not answer in time says nothing about the login itself.
pub const IDENTITY_TIMED_OUT: &str = "Sign-in check timed out";
pub fn identity_check_inconclusive(error: &str) -> bool {
    error == IDENTITY_TIMED_OUT
}

pub async fn identity(account: &Account, binary: &Path) -> Result<Option<String>, String> {
    provider_identity(account.provider, Some(account), binary).await
}
pub async fn provider_identity(
    provider: Provider,
    account: Option<&Account>,
    binary: &Path,
) -> Result<Option<String>, String> {
    if provider == Provider::Codex {
        let mut options = del_codex::CodexOptions {
            api_key: None,
            ..Default::default()
        };
        options.spawn.program = binary.to_owned();
        if let Some(account) = account {
            options.spawn.env = account.environment();
            options
                .spawn
                .global_args
                .extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
        }
        let backend = del_codex::CodexBackend::new(options);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(15), backend.check_auth()).await;
        let _ = backend.shutdown().await;
        return match result {
            Ok(Ok(del_codex::AuthStatus::ChatGpt { email, .. })) => Ok(email),
            Ok(Ok(del_codex::AuthStatus::ApiKey)) => Ok(Some("API key".into())),
            Ok(Ok(_)) => Ok(None),
            Err(_) => Err(IDENTITY_TIMED_OUT.into()),
            Ok(Err(_)) => Err("Unable to read Codex account identity".into()),
        };
    }
    let args: &[&str] = match provider {
        Provider::Claude => &["auth", "status", "--json"],
        Provider::Codex => &[
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "login",
            "status",
        ],
        Provider::OpenCode => return Ok(None),
        Provider::Acp => return Ok(None),
    };
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(args)
        .envs(account.map(|a| a.environment()).unwrap_or_default())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    del_agent::environment::prepare_provider_command(cmd.as_std_mut());
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| IDENTITY_TIMED_OUT.to_string())?
        .map_err(|e| e.to_string())?;
    if provider == Provider::Claude {
        let data: serde_json::Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| "Unable to read Claude sign-in status".to_string())?;
        if data["loggedIn"] != true {
            return Err("Sign in to connect this account".into());
        }
        return Ok(data["email"].as_str().map(str::to_owned));
    }
    if !result.status.success() {
        return Err("Sign in to connect this account".into());
    }
    Ok(None)
}

/// The profile a record names. Only a UUID can name a profile directory (same rule as `load`).
pub fn for_record(record: &del_agent::SessionRecord) -> Option<Account> {
    let id = record
        .account_id
        .clone()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())?;
    Some(Account {
        id,
        provider: record.provider,
        number: 0,
        identity: None,
        is_default: false,
        requires_login: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires the installed OpenCode CLI; isolated profiles, no inference"]
    async fn opencode_native_account_switch_roundtrip() {
        let root = del_agent::data_dir().unwrap().join("accounts");
        std::fs::create_dir_all(&root).unwrap();
        // Production profiles use UUID directory names; the transfer resolver
        // deliberately rejects arbitrary paths and non-UUID profile IDs.
        let profile_dir = || {
            tempfile::Builder::new()
                .prefix(&uuid::Uuid::new_v4().to_string())
                .rand_bytes(0)
                .tempdir_in(&root)
                .unwrap()
        };
        let a_dir = profile_dir();
        let b_dir = profile_dir();
        let profile = |dir: &tempfile::TempDir| Account {
            id: dir.path().file_name().unwrap().to_str().unwrap().into(),
            provider: Provider::OpenCode,
            number: 2,
            identity: None,
            is_default: false,
            requires_login: false,
        };
        let a = profile(&a_dir);
        let b = profile(&b_dir);
        for account in [&a, &b] {
            std::fs::create_dir_all(account.root().join("data/opencode")).unwrap();
        }
        let workspace = tempfile::tempdir().unwrap();
        let manager =
            del_agent::SessionManager::new(workspace.path().join("index.jsonl"), [a.backend()])
                .unwrap();
        manager.register_account(a.id.clone(), a.backend());
        manager.register_account(b.id.clone(), b.backend());
        let config = del_agent::SessionConfig {
            cwd: workspace.path().into(),
            provider: Provider::OpenCode,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: del_agent::PermissionMode::Ask,
        };
        let handle = manager
            .start_account_session(config, Some(a.id.clone()))
            .await
            .unwrap();
        for target in [b.clone(), a.clone(), b.clone()] {
            let id = target.id.clone();
            manager
                .switch_account(&handle.id, Some(id.clone()), |record| {
                    transfer_conversation(record, Some(target))
                })
                .await
                .unwrap();
            assert_eq!(
                manager.list_sessions()[0].account_id.as_deref(),
                Some(id.as_str())
            );
            assert_eq!(manager.list_sessions()[0].id, handle.id);
        }
        manager.shutdown().await;
    }

    #[test]
    fn unreadable_registry_is_copied_aside_and_blocks_saving() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let blocked = std::sync::Mutex::new(None);
        assert!(load_from(&path, &blocked).unwrap().is_empty());
        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            provider: Provider::Codex,
            number: 2,
            identity: None,
            is_default: false,
            requires_login: false,
        };
        save_to(&path, std::slice::from_ref(&account), &blocked).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_from(&path, &blocked).unwrap().len(), 1);
        std::fs::write(&path, "[{\"id\": truncated").unwrap();
        let error = load_from(&path, &blocked).unwrap_err().to_string();
        assert!(error.contains("copy was saved"), "{error}");
        assert!(
            save_to(&path, &[account], &blocked).is_err(),
            "saving must not orphan existing profiles"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"[{\"id\": truncated");
        let copies = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("accounts.json.corrupt-")
            })
            .count();
        assert_eq!(copies, 1);
        // Repeated loads (e.g. terminal launches) do not pile up copies.
        let _ = load_from(&path, &blocked);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn removed_profiles_are_kept_for_the_retention_window_only() {
        let dir = tempfile::tempdir().unwrap();
        let removed = dir.path().join("removed-accounts");
        let id = uuid::Uuid::new_v4().to_string();
        let profile = dir.path().join(&id);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("auth.json"), "secret").unwrap();
        let retired = retire_profile(&profile, &removed, &id).unwrap().unwrap();
        assert!(!profile.exists() && retired.join("auth.json").exists());
        std::fs::create_dir_all(removed.join("not-a-del-profile")).unwrap();
        assert_eq!(purge_removed_profiles(&removed, REMOVED_RETENTION), 0);
        assert!(retired.exists());
        std::fs::write(retired.join(REMOVED_AT), "0").unwrap();
        assert_eq!(purge_removed_profiles(&removed, REMOVED_RETENTION), 1);
        assert!(!retired.exists());
        assert!(removed.join("not-a-del-profile").exists());
    }

    #[test]
    fn login_cleanup_removes_every_attempt_file() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("login-x");
        for path in [
            marker.clone(),
            marker.with_extension("tty"),
            marker.with_extension("sh"),
        ] {
            std::fs::write(path, "1").unwrap();
        }
        cleanup_login(&marker);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn claude_usage_handles_percentages_missing_limits_and_model_windows() {
        let value = serde_json::json!({"rate_limits_available":true,"rate_limits":{
            "five_hour":{"utilization":0.5}, "seven_day":{"utilization":30},
            "seven_day_opus":null, "model_scoped":[{"utilization":45}],
            "extra_usage":{"utilization":100}
        }});
        assert_eq!(claude_usage_remaining(&value), Some(55.));
        assert_eq!(
            claude_usage_remaining(
                &serde_json::json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":0.5}}})
            ),
            Some(99.5)
        );
        assert_eq!(
            claude_usage_remaining(
                &serde_json::json!({"rate_limits_available":false,"rate_limits":null})
            ),
            None
        );
        assert_eq!(
            claude_usage_remaining(
                &serde_json::json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":null}}})
            ),
            None
        );
    }

    #[test]
    fn usage_prefers_buckets_and_never_invents_missing_values() {
        let value = serde_json::json!({"rateLimits":{"primary":{"usedPercent":99,"windowDurationMins":300}},
            "rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":25,"windowDurationMins":300},"secondary":{"usedPercent":40,"windowDurationMins":10080}}}});
        assert_eq!(usage_remaining(&value), Some(60.));
        assert_eq!(
            usage_remaining(
                &serde_json::json!({"rateLimits":{"primary":{"windowDurationMins":300}}})
            ),
            None
        );
    }

    #[test]
    fn transcript_copy_replaces_stale_data_without_copying_credentials() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.jsonl");
        let target = root.path().join("target/session.jsonl");
        std::fs::write(&source, "first turn").unwrap();
        copy_transcript(&source, &target).unwrap();
        std::fs::write(&source, "first turn\nsecond turn").unwrap();
        copy_transcript(&source, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "first turn\nsecond turn"
        );
    }

    #[test]
    fn cancelled_login_reports_status_and_removes_script() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("cancelled login");
        let launch = write_login_script(&marker, "kill -INT $$").unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &launch])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(130));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "130");
        assert!(!marker.with_extension("sh").exists());
    }

    #[test]
    fn long_login_environment_uses_a_short_terminal_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("login ' result");
        let environment = format!("DEL_TEST_LONG={}", quote(&"x".repeat(12000)));
        let body = login_command(Path::new("/usr/bin/true"), "", &environment, &marker);
        assert!(body.len() > 12000);
        let launch = write_login_script(&marker, &body).unwrap();
        assert!(launch.len() < 512);
        assert_eq!(
            std::fs::metadata(marker.with_extension("sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &launch])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "0");
        assert!(!marker.with_extension("sh").exists());
    }

    #[test]
    fn added_accounts_start_after_the_default() {
        assert_eq!(next_number(Provider::Codex, &[]), 2);
        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            provider: Provider::Codex,
            number: 2,
            identity: None,
            is_default: false,
            requires_login: false,
        };
        assert_eq!(
            next_number(Provider::Codex, std::slice::from_ref(&account)),
            3
        );
        assert_eq!(next_number(Provider::Claude, &[account]), 2);
    }
    #[test]
    fn login_records_failure_and_uses_runtime_beside_selected_binary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("provider with spaces");
        let runtime = dir.path().join("del-login-runtime");
        std::fs::write(&binary, "#!/usr/bin/env del-login-runtime\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        for code in [7, 0] {
            std::fs::write(&runtime, format!("#!/bin/sh\nexit {code}\n")).unwrap();
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
            let marker = dir.path().join("login result");
            let command = login_command(&binary, "login", "", &marker);
            let result = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .env("PATH", "/usr/bin:/bin")
                .output()
                .unwrap();
            assert!(result.status.success());
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), code.to_string());
        }
    }

    #[test]
    fn profiles_keep_provider_storage_separate() {
        for provider in [Provider::Claude, Provider::Codex, Provider::OpenCode] {
            let a = Account {
                id: uuid::Uuid::new_v4().to_string(),
                provider,
                number: 1,
                identity: None,
                is_default: false,
                requires_login: false,
            };
            let b = Account {
                id: uuid::Uuid::new_v4().to_string(),
                provider,
                number: 2,
                identity: None,
                is_default: false,
                requires_login: false,
            };
            assert_ne!(a.root(), b.root());
            assert_ne!(a.environment(), b.environment());
            let restored: Account =
                serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
            assert_eq!(restored.environment(), a.environment());
            assert_eq!(restored.display(), "Account 1");
        }
    }
    #[test]
    fn record_profiles_require_uuid_ids() {
        let mut record = del_agent::SessionRecord {
            id: del_agent::SessionId::new("s"),
            provider: Provider::Claude,
            account_id: Some("../../etc".into()),
            cwd: "/tmp".into(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: Default::default(),
            title: None,
            title_turns: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
            status: del_agent::SessionStatus::Idle,
        };
        assert!(for_record(&record).is_none());
        let id = uuid::Uuid::new_v4().to_string();
        record.account_id = Some(id.clone());
        assert_eq!(for_record(&record).unwrap().id, id);
        record.account_id = None;
        assert!(for_record(&record).is_none());
    }
    #[test]
    fn terminal_arguments_are_quoted_as_literals() {
        assert_eq!(quote("a'b $HOME `cmd`"), "'a'\\''b $HOME `cmd`'");
    }
}
