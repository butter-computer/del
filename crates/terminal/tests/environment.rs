//! Use subprocesses rather than mutating the test runner's environment.
use del_terminal::{TerminalEvent, TerminalSession, TerminalSpec};
use std::process::Command;
use std::time::{Duration, Instant};

fn run_case(case: &str) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "environment_child", "--ignored", "--nocapture"])
        .env("DEL_ENV_TEST_CASE", case)
        .env("SHELL", "/bin/zsh")
        .env("PATH", "/usr/bin:/bin")
        .env("CLAUDECODE", "worker-test")
        .env("CLAUDE_CODE_CHILD_SESSION", "worker-test")
        .env("DEL_WORKER_ONLY", "worker-test")
        .env("CLAUDE_CONFIG_DIR", "user-config-test")
        .env("CLAUDE_EFFORT", "user-effort-test")
        .env("CODEX_HOME", "user-codex-config-test")
        .env("ANTHROPIC_AUTH_TOKEN", "user-auth-test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn direct_exec_removes_markers_preserves_user_and_mcp_environment() {
    run_case("direct");
}
#[test]
fn login_startup_cannot_restore_removed_markers_in_command() {
    run_case("login");
}
#[test]
fn interactive_shell_starts_without_inherited_worker_markers() {
    run_case("shell");
}

#[test]
#[ignore = "subprocess fixture, invoked by parent tests"]
fn environment_child() {
    let Ok(case) = std::env::var("DEL_ENV_TEST_CASE") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("del-terminal-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    if case == "login" {
        std::fs::write(dir.join(".zshrc"), "export CLAUDECODE=restored-by-startup\nexport CLAUDE_CODE_CHILD_SESSION=restored-by-startup\nexport DEL_WORKER_ONLY=restored-by-startup\nexport PATH=/usr/bin:/bin\n").unwrap();
    }
    let check = r#"
        test "${CLAUDECODE+x}" != x || exit 11
        test "${CLAUDE_CODE_CHILD_SESSION+x}" != x || exit 12
        test "${DEL_WORKER_ONLY+x}" != x || exit 13
        test "$CLAUDE_CONFIG_DIR" = user-config-test || exit 14
        test "$CLAUDE_EFFORT" = user-effort-test || exit 15
        test "$CODEX_HOME" = user-codex-config-test || exit 16
        test "$ANTHROPIC_AUTH_TOKEN" = user-auth-test || exit 17
        test "$DEL_MCP_TOKEN" = explicit-mcp-test || exit 18
        if test "$DEL_ENV_TEST_CASE" != shell; then
            test "$PATH" = /usr/bin:/bin || exit 19
        fi
        printf 'SANITIZED'
    "#;
    let mut spec = if case == "shell" {
        TerminalSpec::shell()
    } else {
        TerminalSpec::command("/bin/sh", ["-c", check])
    };
    spec.login_env = case != "direct";
    spec = spec
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_CHILD_SESSION")
        .env_remove("DEL_WORKER_ONLY")
        .env("ZDOTDIR", dir.to_string_lossy())
        .env("DEL_MCP_TOKEN", "explicit-mcp-test");
    // Removal wins even over an accidental explicit marker overlay.
    spec = spec.env("CLAUDE_CODE_CHILD_SESSION", "explicit-worker-test");
    let mut session = TerminalSession::spawn(spec).unwrap();
    let mut events = session.take_events().unwrap();
    if case == "shell" {
        session.write(
            format!("{}\nexit\n", check.replace('\n', ";"))
                .replace(";        ", "; ")
                .trim_start_matches(';')
                .as_bytes()
                .to_vec(),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    while !session.exited() {
        assert!(Instant::now() < deadline, "environment fixture timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut exit_event = None;
    while exit_event.is_none() {
        while let Ok(event) = events.try_recv() {
            if let TerminalEvent::Exited { code } = event {
                exit_event = Some(code);
            }
        }
        assert!(
            Instant::now() < deadline,
            "environment fixture exit event timed out"
        );
        if exit_event.is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(exit_event, Some(Some(0)));
    let screen: String =
        session.with_term(|term| term.grid().display_iter().map(|c| c.c).collect());
    assert!(screen.contains("SANITIZED"));
    session.close(Duration::ZERO);
}
