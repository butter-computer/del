//! Update the installation that actually owns a harness, before launching it.
//! No installer commands are taken from terminal output or registry install hints.
use del_agent::Provider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Installer {
    Npm {
        npm: PathBuf,
        prefix: PathBuf,
        package: String,
        version: String,
    },
    Brew {
        brew: PathBuf,
        package: String,
        cask: bool,
    },
    ClaudeNative,
    Unmanaged,
}

/// Agent terminals launched per provider, by process id. Entries are pruned
/// once their process has ended.
static TERMINALS: OnceLock<std::sync::Mutex<Vec<(Provider, u32)>>> = OnceLock::new();

/// Record an agent terminal running `provider`'s CLI as process `pid`.
pub fn terminal_started(provider: Provider, pid: u32) {
    TERMINALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push((provider, pid));
}

fn terminal_uses(provider: Provider, alive: impl Fn(u32) -> bool) -> bool {
    let mut terminals = TERMINALS.get_or_init(Default::default).lock().unwrap();
    terminals.retain(|(_, pid)| alive(*pid));
    terminals.iter().any(|(p, _)| *p == provider)
}

/// Whether replacing `provider`'s CLI now could break something running on
/// it: a live chat or a running agent terminal. Package managers can remove
/// the old version (e.g. Homebrew's Cellar) mid-turn, so updates wait.
pub fn provider_in_use(manager: &del_agent::SessionManager, provider: Provider) -> bool {
    manager
        .list_sessions()
        .iter()
        .any(|record| record.provider == provider && manager.is_live(&record.id))
        || terminal_uses(provider, del_agent::procgroup::process_alive)
}

/// Last outcome per harness binary. Failures (typically offline) back off so a
/// config change or terminal launch does not rerun `brew update` each time.
#[derive(Clone)]
struct Checked {
    at: Instant,
    result: Result<bool, String>,
    failures: u32,
}

impl Checked {
    fn fresh(&self) -> bool {
        self.at.elapsed() < retry_after(self.result.is_ok(), self.failures)
    }
}

const SUCCESS_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// 5, 10, 20 … minutes after consecutive failures, capped at six hours.
fn retry_after(succeeded: bool, failures: u32) -> Duration {
    if succeeded {
        return SUCCESS_INTERVAL;
    }
    let exponent = failures.saturating_sub(1).min(7);
    (Duration::from_secs(5 * 60) * 2u32.pow(exponent)).min(Duration::from_secs(6 * 60 * 60))
}

type UpdateLocks = Mutex<HashMap<PathBuf, Arc<Mutex<Option<Checked>>>>>;
static LOCKS: OnceLock<UpdateLocks> = OnceLock::new();

/// A global npm package name: optional `@scope/`, then lowercase URL-safe characters.
fn valid_npm_package(name: &str) -> bool {
    let segment = |s: &str| {
        !s.is_empty()
            && s.len() <= 214
            && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    };
    match name.strip_prefix('@') {
        Some(scoped) => scoped.split_once('/').is_some_and(|(scope, pkg)| segment(scope) && segment(pkg)),
        None => segment(name),
    }
}

fn version(value: &str) -> Option<(u64, u64, u64)> {
    let parts = value
        .trim()
        .trim_start_matches('v')
        .split('.')
        .collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn installer(program: &Path, provider: Provider) -> Installer {
    let Ok(real) = program.canonicalize() else {
        return Installer::Unmanaged;
    };
    // Only global npm prefixes, never a project-local package or an install hint.
    for parent in real.ancestors().skip(1).take(6) {
        let Ok(bytes) = std::fs::read(parent.join("package.json")) else {
            continue;
        };
        let Ok(package) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let Some(installed) = package["version"].as_str() else {
            continue;
        };
        let modules = if name.starts_with('@') {
            parent.parent().and_then(Path::parent)
        } else {
            parent.parent()
        };
        let Some(modules) = modules.filter(|p| p.file_name().is_some_and(|n| n == "node_modules"))
        else {
            continue;
        };
        let Some(prefix) = modules
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "lib"))
            .and_then(Path::parent)
        else {
            continue;
        };
        let owns_binary = match &package["bin"] {
            serde_json::Value::String(bin) => {
                parent.join(bin).canonicalize().is_ok_and(|p| p == real)
            }
            serde_json::Value::Object(bins) => bins
                .values()
                .filter_map(|v| v.as_str())
                .any(|bin| parent.join(bin).canonicalize().is_ok_and(|p| p == real)),
            _ => false,
        };
        let npm = prefix.join("bin/npm");
        if owns_binary && npm.is_file() && version(installed).is_some() && valid_npm_package(name) {
            return Installer::Npm {
                npm,
                prefix: prefix.to_path_buf(),
                package: name.to_owned(),
                version: installed.to_owned(),
            };
        }
    }
    for prefix in [Path::new("/opt/homebrew"), Path::new("/usr/local")] {
        for (directory, cask) in [("Caskroom", true), ("Cellar", false)] {
            if let Ok(relative) = real.strip_prefix(prefix.join(directory))
                && let Some(package) = relative.components().next()
            {
                let brew = prefix.join("bin/brew");
                if brew.is_file() {
                    return Installer::Brew {
                        brew,
                        package: package.as_os_str().to_string_lossy().into_owned(),
                        cask,
                    };
                }
            }
        }
    }
    if provider == Provider::Claude
        && dirs::home_dir().is_some_and(|home| real.starts_with(home.join(".local/share/claude")))
    {
        Installer::ClaudeNative
    } else {
        Installer::Unmanaged
    }
}

async fn output(program: &Path, args: &[String], timeout: Duration) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    command.current_dir(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
    if let Some(bin) = program.parent() {
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    for key in del_agent::environment::WORKER_SESSION_ENV {
        command.env_remove(key);
    }
    command.env("CI", "1");
    #[cfg(unix)]
    command.process_group(0);
    let child = command
        .spawn()
        .map_err(|e| format!("Could not start updater: {e}"))?;
    let pid = child.id();
    let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
    match result {
        Ok(Ok(result)) if result.status.success() => {
            Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
        }
        Ok(Ok(result)) => Err(format!(
            "Update failed: {}",
            String::from_utf8_lossy(&result.stderr)
                .chars()
                .take(800)
                .collect::<String>()
        )),
        Ok(Err(error)) => Err(format!("Update failed: {error}")),
        Err(_) => {
            if let Some(pid) = pid {
                del_agent::procgroup::signal_group(pid, true);
            }
            Err("Update timed out. Your conversation and prompt are preserved; retry when connected.".into())
        }
    }
}

pub async fn ensure_program(
    program: PathBuf,
    provider: Provider,
    progress: impl Fn(String),
) -> Result<bool, String> {
    let lock = LOCKS
        .get_or_init(Default::default)
        .lock()
        .await
        .entry(program.clone())
        .or_default()
        .clone();
    progress(format!(
        "Checking {} for updates…",
        crate::providers::display_name(provider)
    ));
    let mut checked = lock.lock().await;
    if let Some(previous) = checked.as_ref().filter(|checked| checked.fresh()) {
        return previous.result.clone();
    }
    let target = program.clone();
    let plan = tokio::task::spawn_blocking(move || installer(&target, provider))
        .await
        .map_err(|e| e.to_string())?;
    let managed = plan != Installer::Unmanaged;
    let result = apply_update(&program, &plan, &progress).await.map(|()| managed);
    let failures = match (&result, checked.as_ref()) {
        (Ok(_), _) => 0,
        (Err(_), Some(previous)) => previous.failures + 1,
        (Err(_), None) => 1,
    };
    *checked = Some(Checked { at: Instant::now(), result: result.clone(), failures });
    result
}

async fn apply_update(
    program: &Path,
    plan: &Installer,
    progress: &impl Fn(String),
) -> Result<(), String> {
    match plan {
        Installer::Npm {
            npm,
            prefix,
            package,
            version: installed,
        } => {
            let latest = output(
                npm,
                &[
                    "view".into(),
                    format!("{package}@latest"),
                    "version".into(),
                    "--json".into(),
                    "--fetch-retries=0".into(),
                    "--fetch-timeout=10000".into(),
                ],
                Duration::from_secs(15),
            )
            .await?;
            let latest = serde_json::from_str::<String>(&latest)
                .map_err(|_| "Package registry returned an invalid version".to_owned())?;
            let latest_version =
                version(&latest).ok_or("Package registry returned an unsupported version")?;
            if Some(latest_version) > version(installed) {
                progress(format!("Updating {package} to {latest}…"));
                output(
                    npm,
                    &[
                        "install".into(),
                        "--global".into(),
                        "--prefix".into(),
                        prefix.display().to_string(),
                        format!("{package}@{latest}"),
                        "--no-audit".into(),
                        "--no-fund".into(),
                    ],
                    Duration::from_secs(300),
                )
                .await?;
                // Verify the same launch path works after the package manager replaces it.
                output(program, &["--version".into()], Duration::from_secs(15)).await?;
            }
        }
        Installer::Brew {
            brew,
            package,
            cask,
        } => {
            output(
                brew,
                &["update".into(), "--quiet".into()],
                Duration::from_secs(120),
            )
            .await?;
            let outdated = output(
                brew,
                &[
                    "outdated".into(),
                    if *cask { "--cask" } else { "--formula" }.into(),
                    package.clone(),
                ],
                Duration::from_secs(60),
            )
            .await?;
            if !outdated.trim().is_empty() {
                progress(format!("Updating {package}…"));
                output(
                    brew,
                    &[
                        "upgrade".into(),
                        if *cask { "--cask" } else { "--formula" }.into(),
                        package.clone(),
                    ],
                    Duration::from_secs(300),
                )
                .await?;
                output(program, &["--version".into()], Duration::from_secs(15)).await?;
            }
        }
        Installer::ClaudeNative => {
            progress("Updating Claude Code…".into());
            output(program, &["update".into()], Duration::from_secs(300)).await?;
        }
        Installer::Unmanaged => {}
    }
    Ok(())
}

pub async fn ensure_provider(
    provider: Provider,
    model: Option<String>,
    progress: impl Fn(String),
) -> Result<(), String> {
    if std::env::var("DEL_BACKEND").as_deref() == Ok("mock") {
        return Ok(());
    }
    let programs = match provider {
        Provider::Codex | Provider::OpenCode => {
            del_agent::detect::find_working_binary(provider.binary())
                .await
                .map(|(path, _)| path)
                .into_iter()
                .collect::<Vec<_>>()
        }
        Provider::Claude => {
            tokio::task::spawn_blocking(|| del_agent::detect::find_binary("claude"))
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect()
        }
        Provider::Acp => tokio::task::spawn_blocking(move || {
            let (id, _) = del_acp::registry::split_model(model.as_deref());
            let (agents, _) = del_acp::registry::load();
            agents
                .into_iter()
                .filter(|agent| id.is_none_or(|id| id == agent.id))
                .find_map(|agent| agent.resolve())
                .into_iter()
                .collect()
        })
        .await
        .map_err(|e| e.to_string())?,
    };
    for program in programs {
        ensure_program(program, provider, &progress).await?;
    }
    Ok(())
}

pub fn completed_cli_update(provider: Provider, output: &str, exit: Option<i32>) -> bool {
    // PTY line wrapping may split status sentences across display rows.
    let output = output.split_whitespace().collect::<Vec<_>>().join(" ");
    exit == Some(0)
        && match provider {
            Provider::Codex => {
                output.contains("Updating Codex via")
                    && output.contains("Update ran successfully!")
                    && output.contains("Please restart Codex")
            }
            Provider::Claude => {
                output.contains("Update installed") && output.contains("Restart to apply")
            }
            Provider::Acp | Provider::OpenCode => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_terminals_hold_off_their_provider_until_they_exit() {
        terminal_started(Provider::OpenCode, 4242);
        assert!(terminal_uses(Provider::OpenCode, |_| true));
        assert!(!terminal_uses(Provider::Acp, |_| true));
        // An ended process is pruned and no longer blocks updates.
        assert!(!terminal_uses(Provider::OpenCode, |_| false));
        assert!(!terminal_uses(Provider::OpenCode, |_| true));
    }

    #[test]
    fn failed_checks_back_off_instead_of_rerunning_immediately() {
        assert_eq!(retry_after(true, 0), SUCCESS_INTERVAL);
        assert_eq!(retry_after(false, 1), Duration::from_secs(300));
        assert_eq!(retry_after(false, 3), Duration::from_secs(1200));
        assert_eq!(retry_after(false, 40), Duration::from_secs(6 * 3600));
        let failed = Checked { at: Instant::now(), result: Err("offline".into()), failures: 1 };
        assert!(failed.fresh());
        let stale = Checked { at: Instant::now() - Duration::from_secs(301), ..failed };
        assert!(!stale.fresh());
    }
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn executable(path: &Path, source: &str) {
        std::fs::write(path, source).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn installation(installed: &str, latest: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path();
        let package = prefix.join("lib/node_modules/@test/harness");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name": "@test/harness", "version": installed, "bin": {"harness": "cli"}
            })
            .to_string(),
        )
        .unwrap();
        executable(&package.join("cli"), "#!/bin/sh\necho 'harness 2.0.0'\n");
        let program = prefix.join("bin/harness");
        symlink(package.join("cli"), &program).unwrap();
        std::fs::write(prefix.join("latest"), format!("\"{latest}\"\n")).unwrap();
        executable(
            &prefix.join("bin/npm"),
            r#"#!/bin/sh
root="$(dirname "$0")/.."
printf '%s\n' "$*" >> "$root/calls"
case "$1" in
view) cat "$root/latest" ;;
install)
    if [ -f "$root/fail" ]; then echo 'fixture install failure' >&2; exit 1; fi
    touch "$root/installed"
    ;;
*) exit 99 ;;
esac
"#,
        );
        (root, program)
    }

    #[tokio::test]
    async fn updates_once_before_concurrent_launches_and_keeps_own_prefix() {
        let (root, program) = installation("1.0.0", "2.0.0");
        let (first, second) = tokio::join!(
            ensure_program(program.clone(), Provider::Codex, |_| {}),
            ensure_program(program.clone(), Provider::Codex, |_| {})
        );
        assert_eq!(first, Ok(true));
        assert_eq!(second, Ok(true));
        assert!(root.path().join("installed").is_file());
        let calls = std::fs::read_to_string(root.path().join("calls")).unwrap();
        assert_eq!(
            calls.lines().count(),
            2,
            "one check and one install, never two competing installers"
        );
        assert!(calls.contains(&format!(
            "install --global --prefix {} @test/harness@2.0.0",
            root.path().canonicalize().unwrap().display()
        )));
    }

    #[tokio::test]
    async fn failed_update_backs_off_then_retries_and_is_never_cached_as_success() {
        let (root, program) = installation("1.0.0", "2.0.0");
        std::fs::write(root.path().join("fail"), "").unwrap();
        assert!(
            ensure_program(program.clone(), Provider::Codex, |_| {})
                .await
                .unwrap_err()
                .contains("fixture install failure")
        );
        assert!(!root.path().join("installed").exists());
        std::fs::remove_file(root.path().join("fail")).unwrap();
        // Within the backoff the failure is reported without rerunning the installer.
        let calls = std::fs::read_to_string(root.path().join("calls")).unwrap();
        assert!(
            ensure_program(program.clone(), Provider::Codex, |_| {})
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(root.path().join("calls")).unwrap(), calls);
        // Once the backoff passes, the update is retried and succeeds.
        let lock = LOCKS.get().unwrap().lock().await.get(&program).unwrap().clone();
        if let Some(checked) = lock.lock().await.as_mut() {
            checked.at = Instant::now() - Duration::from_secs(301);
        }
        assert_eq!(
            ensure_program(program, Provider::Codex, |_| {}).await,
            Ok(true)
        );
        assert!(root.path().join("installed").is_file());
    }

    #[test]
    fn npm_package_names_follow_the_registry_grammar() {
        for ok in ["@openai/codex", "opencode-ai", "@anthropic-ai/claude-code", "a.b_c"] {
            assert!(valid_npm_package(ok), "{ok}");
        }
        for bad in ["", "@", "@scope", "@scope/", "/x", "a/b", "@a/b/c", "Upper", "a b", "a;b", "@a@b/c", "../x", "x\n"] {
            assert!(!valid_npm_package(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn equal_or_newer_installations_are_never_downgraded() {
        for installed in ["2.0.0", "3.0.0"] {
            let (root, program) = installation(installed, "2.0.0");
            assert_eq!(
                ensure_program(program, Provider::Codex, |_| {}).await,
                Ok(true)
            );
            assert!(!root.path().join("installed").exists());
        }
    }

    #[tokio::test]
    async fn invalid_registry_version_never_becomes_an_install_argument() {
        let (root, program) = installation("1.0.0", "2.0.0;touch bad");
        assert!(
            ensure_program(program, Provider::Codex, |_| {})
                .await
                .is_err()
        );
        assert!(!root.path().join("installed").exists());
    }

    #[test]
    fn only_the_binary_owner_can_choose_the_package() {
        let (root, program) = installation("1.0.0", "2.0.0");
        assert!(matches!(
            installer(&program, Provider::Acp),
            Installer::Npm { .. }
        ));
        let path = root
            .path()
            .join("lib/node_modules/@test/harness/package.json");
        std::fs::write(
            path,
            r#"{"name":"@test/harness","version":"1.0.0","bin":"unrelated"}"#,
        )
        .unwrap();
        assert_eq!(installer(&program, Provider::Codex), Installer::Unmanaged);
        // A custom Claude wrapper must not receive an invented update command.
        assert_eq!(installer(&program, Provider::Claude), Installer::Unmanaged);
    }

    #[test]
    fn restart_requires_successful_provider_update_not_an_ordinary_exit() {
        let codex = "Updating Codex via `npm install -g @openai/codex`...\nUpdate ran successfully! Please restart Codex.";
        assert!(completed_cli_update(Provider::Codex, codex, Some(0)));
        assert!(completed_cli_update(
            Provider::Codex,
            &codex.replace("Please restart", "Please\nrestart"),
            Some(0)
        ));
        assert!(!completed_cli_update(Provider::Codex, codex, Some(1)));
        assert!(!completed_cli_update(Provider::Codex, codex, None));
        assert!(!completed_cli_update(
            Provider::Codex,
            "Done. Please restart Codex.",
            Some(0)
        ));
        assert!(!completed_cli_update(Provider::Claude, codex, Some(0)));
        assert!(!completed_cli_update(Provider::Acp, codex, Some(0)));
        assert!(completed_cli_update(
            Provider::Claude,
            "Update installed. Restart to apply",
            Some(0)
        ));
    }

    #[tokio::test]
    async fn stalled_updaters_have_a_bounded_failure() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("stall");
        executable(&program, "#!/bin/sh\nsleep 10\n");
        let start = Instant::now();
        assert!(
            output(&program, &[], Duration::from_millis(50))
                .await
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
