//! Provider setup helpers: names, model choices, sign-in instructions and the
//! login launcher. Detection itself is `del_agent::detect`.
//!
//! del never reads or stores credentials. Signing in is the CLI's own login
//! flow running in the user's terminal; status comes from the CLI's status
//! command via `del_agent::detect`.

use del_agent::{LoginState, Provider, ProviderStatus};

/// Providers offered for new chats and account setup.
pub const VISIBLE: [Provider; 3] = [Provider::Codex, Provider::Claude, Provider::OpenCode];

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
    let id = manager.resolve_peer(id);
    if let Ok(record) = manager.session_record(&id)
        && matches!(
            record.provider,
            Provider::Codex | Provider::Claude | Provider::OpenCode
        )
    {
        manager.set_permission_mode(&id, permission_mode(record.provider))?;
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

/// The provider's own npm install command, offered when its CLI is missing.
pub fn install_command(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Codex => Some("npm i -g @openai/codex"),
        Provider::Claude => Some("npm i -g @anthropic-ai/claude-code"),
        Provider::OpenCode => Some("npm i -g opencode-ai"),
        Provider::Acp => None,
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
            let fork = manager
                .fork(
                    del_agent::ForkSource::Session(session.id.clone()),
                    del_agent::ForkLifetime::Temporary,
                )
                .await
                .unwrap();
            prepare_session_permissions(&manager, &fork.handle.id).unwrap();
            assert_eq!(
                manager
                    .session_record(&fork.handle.id)
                    .unwrap()
                    .permission_mode,
                permission_mode(provider)
            );
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
    fn every_offered_provider_has_an_install_command() {
        for provider in VISIBLE {
            let command = install_command(provider).unwrap();
            assert!(command.starts_with("npm i -g "), "{command}");
        }
        assert_eq!(install_command(Provider::Acp), None);
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
