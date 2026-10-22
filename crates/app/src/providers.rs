//! Provider setup helpers: names, model choices, sign-in instructions and the
//! login launcher. Detection itself is `del_agent::detect`.
//!
//! del never reads or stores credentials. Signing in is the CLI's own login
//! flow running in the user's terminal; status comes from the CLI's status
//! command via `del_agent::detect`.

use del_agent::{LoginState, Provider, ProviderStatus};

pub fn ready(st: &ProviderStatus) -> bool {
    st.installed && st.login == LoginState::LoggedIn
}

pub fn summary(st: &ProviderStatus) -> &'static str {
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
