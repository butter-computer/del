//! Provider setup helpers: names, model choices, sign-in instructions and the
//! login launcher. Detection itself is `del_agent::detect`.
//!
//! del never reads or stores credentials. Signing in is the CLI's own login
//! flow running in the user's terminal; status comes from the CLI's status
//! command via `del_agent::detect`.

use del_agent::{LoginState, Provider, ProviderStatus};

pub fn ready(st: &ProviderStatus) -> bool {
    st.installed && (st.provider == Provider::Acp || st.login == LoginState::LoggedIn)
}

pub fn summary(st: &ProviderStatus) -> &'static str {
    if st.provider == Provider::Acp && st.installed {
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
        Provider::Acp => "ACP",
    }
}

/// Model choices; `None` is the provider's own default.
pub fn models(provider: Provider) -> Vec<Option<String>> {
    match provider {
        Provider::Codex => vec![None, Some("gpt-5-codex".into()), Some("gpt-5".into())],
        Provider::Claude => vec![
            None,
            Some("sonnet".into()),
            Some("opus".into()),
            Some("haiku".into()),
        ],
        Provider::Acp => std::iter::once(None)
            .chain(del_acp::registry::installed_ids().into_iter().map(Some))
            .collect(),
    }
}

pub fn model_label(model: Option<&str>) -> String {
    model.unwrap_or("Default").to_string()
}

/// The command the user runs to sign in, shown in the panel.
pub fn login_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex login",
        Provider::Claude => "claude auth login",
        Provider::Acp => "Sign in with the selected agent’s CLI",
    }
}

pub fn install_hint(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Install: npm i -g @openai/codex  (or brew install codex)",
        Provider::Claude => "Install: see claude.com/claude-code",
        Provider::Acp => {
            "Install an ACP adapter or configure acp-agents.json in del’s data directory"
        }
    }
}

/// Installed ACP agents authenticate through their own CLI when launched.
pub async fn detect_all() -> Vec<ProviderStatus> {
    let mut statuses = del_agent::detect_all().await;
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
    statuses.push(ProviderStatus {
        provider: Provider::Acp,
        installed: !installed.is_empty(),
        path: None,
        version: Some(warning.unwrap_or(names)),
        login: LoginState::Unknown,
    });
    statuses
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
