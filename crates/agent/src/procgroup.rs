//! Process-group helpers so a backend's whole process tree dies with it.
//!
//! `codex` is a Node wrapper around a native binary, and `claude` spawns MCP
//! servers and tools; killing only the direct child orphans those. Backends
//! start their CLI as the leader of a fresh group (`Command::process_group(0)`)
//! and signal the group.

/// Signal every process in the group led by `pid`. Returns whether the signal
/// was delivered to at least one process. No-op off Unix.
#[cfg(unix)]
pub fn signal_group(pid: u32, kill: bool) -> bool {
    let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: killpg has no memory-safety preconditions; `pid` is a group we created.
    unsafe { libc::killpg(pid as libc::pid_t, sig) == 0 }
}

#[cfg(not(unix))]
pub fn signal_group(_pid: u32, _kill: bool) -> bool {
    false
}

/// SIGTERM the group, then SIGKILL it after `grace` if `child` has not exited.
pub async fn terminate_group(
    pid: u32,
    child: &mut tokio::process::Child,
    grace: std::time::Duration,
) {
    signal_group(pid, false);
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.start_kill();
    }
    // The leader may be gone while its descendants linger; always sweep.
    signal_group(pid, true);
    let _ = child.wait().await;
}
