//! Parent harness context must not label independent app sessions as children.
//! Authentication, installation paths, user preferences, and MCP launch env remain inherited.

pub const WORKER_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CODEX_CI",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
];

/// Apply to the child command only; never mutate the parent environment.
pub fn clear_worker_session_env(command: &mut std::process::Command) {
    for key in WORKER_SESSION_ENV {
        command.env_remove(key);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn independent_child_loses_parent_context_and_retains_user_and_launch_settings() {
        let mut command = std::process::Command::new("/usr/bin/env");
        for key in WORKER_SESSION_ENV {
            command.env(key, "parent-worker");
        }
        command.env("CLAUDE_EFFORT", "high");
        command.env("ANTHROPIC_API_KEY", "fixture-auth");
        command.env("DEL_MCP_TOKEN", "fixture-launch-token");
        clear_worker_session_env(&mut command);
        let result = command.output().unwrap();
        assert!(result.status.success());
        let output = String::from_utf8(result.stdout).unwrap();
        for key in WORKER_SESSION_ENV {
            assert!(
                !output
                    .lines()
                    .any(|line| line.starts_with(&format!("{key}="))),
                "parent marker {key} reached child"
            );
        }
        assert!(output.lines().any(|line| line == "CLAUDE_EFFORT=high"));
        assert!(
            output
                .lines()
                .any(|line| line == "ANTHROPIC_API_KEY=fixture-auth")
        );
        assert!(
            output
                .lines()
                .any(|line| line == "DEL_MCP_TOKEN=fixture-launch-token")
        );
    }
}
