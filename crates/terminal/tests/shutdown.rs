use del_terminal::{TerminalSession, TerminalSpec, shutdown_all};
use std::time::{Duration, Instant};

#[test]
fn app_quit_shutdown_kills_all_live_sessions() {
    let sessions: Vec<_> = (0..2)
        .map(|_| {
            TerminalSession::spawn(TerminalSpec {
                login_env: false,
                ..TerminalSpec::command(
                    "/bin/sh",
                    ["-c", "trap '' HUP TERM; while :; do sleep 1; done"],
                )
            })
            .unwrap()
        })
        .collect();
    shutdown_all(Duration::from_millis(50));
    let deadline = Instant::now() + Duration::from_secs(5);
    while sessions.iter().any(|s| !s.exited()) {
        assert!(
            Instant::now() < deadline,
            "shutdown did not finish reaping children"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    for session in &sessions {
        assert_eq!(unsafe { libc::kill(session.pid() as i32, 0) }, -1);
    }
    shutdown_all(Duration::ZERO);
}
