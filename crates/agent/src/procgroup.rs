//! Process-group helpers so a backend's whole process tree dies with it.
//!
//! `codex` is a Node wrapper around a native binary, and `claude` spawns MCP
//! servers and tools; killing only the direct child orphans those. Backends
//! start their CLI as the leader of a fresh group (`Command::process_group(0)`)
//! and signal the group.

/// Signal every process in the group led by `pid`. Returns whether the signal
/// was delivered to at least one process. No-op off Unix.
///
/// Refuses pids 0 and 1 (`killpg(0)` would signal Del's own group) and pids
/// that do not fit a `pid_t`.
#[cfg(unix)]
pub fn signal_group(pid: u32, kill: bool) -> bool {
    let Some(pgid) = group_id(pid) else {
        return false;
    };
    let sig = if kill { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: killpg has no memory-safety preconditions; `pgid` is a group we created.
    unsafe { libc::killpg(pgid, sig) == 0 }
}

#[cfg(unix)]
fn group_id(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid).ok().filter(|&pgid| pgid > 1)
}

#[cfg(not(unix))]
pub fn signal_group(_pid: u32, _kill: bool) -> bool {
    false
}

/// Whether process `pid` still exists. Refuses pids 0 and 1. False off Unix.
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    let Some(pid) = group_id(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists; nothing is sent.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
pub fn process_alive(_pid: u32) -> bool {
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
        // The leader is not reaped yet, so its pid (and group id) cannot have
        // been reused: kill the whole group before reaping it.
        signal_group(pid, true);
        let _ = child.start_kill();
        let _ = child.wait().await;
    } else {
        // The leader is gone while its descendants may linger. A group id is
        // never reused while any member survives, so sweep immediately.
        signal_group(pid, true);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn refuses_own_group_init_and_out_of_range_pids() {
        assert_eq!(group_id(0), None);
        assert_eq!(group_id(1), None);
        assert_eq!(group_id(u32::MAX), None);
        assert_eq!(group_id(i32::MAX as u32 + 1), None);
        assert_eq!(group_id(42), Some(42));
        assert!(!signal_group(0, false));
        assert!(!signal_group(u32::MAX, true));
    }
}
