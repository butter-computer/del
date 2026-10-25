//! Provider setup helpers: names, model choices, sign-in instructions and the
//! login launcher. Detection itself is `del_agent::detect`.
//!
//! del never reads or stores credentials. Signing in is the CLI's own login
//! flow running in the user's terminal; status comes from the CLI's status
//! command via `del_agent::detect`.

use del_agent::{LoginState, Provider, ProviderStatus};

/// Del starts the built-in agents with full access; ACP keeps its own policy.
pub fn permission_mode(provider: Provider) -> del_agent::PermissionMode {
    match provider {
        Provider::Codex | Provider::Claude | Provider::OpenCode => {
            del_agent::PermissionMode::FullAuto
        }
        Provider::Acp => del_agent::PermissionMode::Ask,
    }
}

pub fn prepare_session_permissions(
    manager: &del_agent::SessionManager,
    id: &del_agent::SessionId,
) -> del_agent::Result<()> {
    if let Some(record) = manager
        .list_sessions()
        .into_iter()
        .find(|record| &record.id == id)
        && matches!(
            record.provider,
            Provider::Codex | Provider::Claude | Provider::OpenCode
        )
    {
        manager.set_permission_mode(id, permission_mode(record.provider))?;
    }
    Ok(())
}

pub fn ready(st: &ProviderStatus) -> bool {
    st.installed
        && (matches!(st.provider, Provider::Acp | Provider::OpenCode)
            || st.login == LoginState::LoggedIn)
}

pub fn summary(st: &ProviderStatus) -> &'static str {
    if matches!(st.provider, Provider::Acp | Provider::OpenCode) && st.installed {
        return "Available";
    }
    match (st.installed, st.login) {
        (false, _) => "Not installed",
        (true, LoginState::LoggedIn) => "Signed in",
        (true, LoginState::LoggedOut) => "Signed out",
        (true, LoginState::Unknown) => "Status unknown",
    }
}

pub fn display_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
        Provider::OpenCode => "OpenCode",
        Provider::Acp => "ACP",
    }
}

/// The command the user runs to sign in, shown in the panel.
pub fn login_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex login",
        Provider::Claude => "claude auth login",
        Provider::OpenCode => "opencode auth login",
        Provider::Acp => "Sign in with the selected agent’s CLI",
    }
}

/// Installed ACP agents authenticate through their own CLI when launched.
pub async fn detect_all() -> Vec<ProviderStatus> {
    let mut statuses = del_agent::detect_all().await;
    statuses.push(detect(Provider::Acp).await);
    statuses
}

pub async fn detect(provider: Provider) -> ProviderStatus {
    if provider != Provider::Acp {
        return del_agent::detect::detect(provider).await;
    }
    let (agents, warning) = del_acp::registry::load();
    let installed: Vec<_> = del_acp::registry::detect(&agents)
        .into_iter()
        .filter(|s| s.installed())
        .collect();
    let names = installed
        .iter()
        .map(|s| s.agent.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    ProviderStatus {
        provider: Provider::Acp,
        installed: !installed.is_empty(),
        path: None,
        version: Some(warning.unwrap_or(names)),
        login: LoginState::Unknown,
    }
}

/// Open the CLI's official login in the user's terminal (macOS Terminal).
/// The login flow, browser step and token storage are entirely the CLI's.
pub fn launch_login(provider: Provider, path: &std::path::Path) -> std::io::Result<()> {
    let command = format!(
        "'{}' {}",
        path.display(),
        login_command(provider).split_once(' ').map_or("", |x| x.1)
    );
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

    #[tokio::test]
    async fn reopening_builtin_sessions_persists_full_access_and_leaves_acp_alone() {
        use del_agent::{MockBackend, PermissionMode, SessionConfig, SessionManager};
        let directory = tempfile::tempdir().unwrap();
        let backends = Provider::ALL.iter().map(|&provider| {
            std::sync::Arc::new(MockBackend::new(provider))
                as std::sync::Arc<dyn del_agent::AgentBackend>
        });
        let manager =
            SessionManager::new(directory.path().join("sessions.jsonl"), backends).unwrap();
        for provider in Provider::ALL {
            let config = SessionConfig {
                provider,
                cwd: directory.path().into(),
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: PermissionMode::Ask,
            };
            let session = manager.start_session(config).await.unwrap();
            manager.close(&session.id).await.unwrap();
            prepare_session_permissions(&manager, &session.id).unwrap();
            prepare_session_permissions(&manager, &session.id).unwrap();
            let record = manager
                .list_sessions()
                .into_iter()
                .find(|record| record.id == session.id)
                .unwrap();
            assert_eq!(record.permission_mode, permission_mode(provider));
            assert_eq!(manager.resume(&session.id).await.unwrap().id, session.id);
        }
        manager.shutdown().await;
    }

    #[test]
    fn summary_covers_each_state() {
        let mut st = ProviderStatus {
            provider: Provider::Codex,
            installed: false,
            path: None,
            version: None,
            login: LoginState::Unknown,
        };
        assert_eq!(summary(&st), "Not installed");
        st.installed = true;
        assert_eq!(summary(&st), "Status unknown");
        st.login = LoginState::LoggedOut;
        assert_eq!(summary(&st), "Signed out");
        st.login = LoginState::LoggedIn;
        assert_eq!(summary(&st), "Signed in");
    }
}
