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

/// Prepare an independent provider child, including runtimes omitted from Finder's PATH.
/// Keep explicit launch settings and credentials; never change the app's environment.
pub fn prepare_provider_command(command: &mut std::process::Command) {
    let inherited = command.get_envs()
        .find(|(key, _)| *key == "PATH")
        .map(|(_, value)| value.unwrap_or_default().to_owned())
        .unwrap_or_else(|| std::env::var_os("PATH").unwrap_or_default());
    let mut paths = Vec::new();
    let program = std::path::Path::new(command.get_program());
    if program.is_absolute() {
        if let Some(parent) = program.parent() {
            paths.push(parent.to_path_buf());
        }
    }
    paths.extend(std::env::split_paths(&inherited).filter(|path| !path.as_os_str().is_empty()));
    paths.extend(crate::detect::common_dirs());
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
    clear_worker_session_env(command);
}

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
    fn gui_launch_resolves_runtime_beside_provider() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("del-fixture-runtime");
        let provider = dir.path().join("provider");
        std::fs::write(&runtime, "#!/bin/sh\necho provider-ready\n").unwrap();
        std::fs::write(&provider, "#!/usr/bin/env del-fixture-runtime\n").unwrap();
        for path in [&runtime, &provider] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut command = std::process::Command::new(&provider);
        command.env("PATH", "/usr/bin:/bin");
        prepare_provider_command(&mut command);
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "provider-ready");
    }

    #[test]
    fn independent_child_loses_parent_context_and_retains_user_and_launch_settings() {
        let mut command = std::process::Command::new("/usr/bin/env");
        for key in WORKER_SESSION_ENV {
            command.env(key, "parent-worker");
        }
        command.env("CLAUDE_EFFORT", "high");
        command.env("ANTHROPIC_API_KEY", "fixture-auth");
        command.env("DEL_MCP_TOKEN", "fixture-launch-token");
        prepare_provider_command(&mut command);
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
