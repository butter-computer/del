use del_terminal::{TerminalEvent, TerminalSession, TerminalSize, TerminalSpec};
use std::time::{Duration, Instant};

fn spawn(script: &str) -> TerminalSession {
    TerminalSession::spawn(TerminalSpec {
        login_env: false,
        ..TerminalSpec::command("/bin/sh", ["-c", script])
    })
    .unwrap()
}
fn screen(s: &TerminalSession) -> String {
    s.with_term(|t| t.grid().display_iter().map(|c| c.c).collect())
}
fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn pty_preserves_ansi_truecolour_and_explicit_backgrounds() {
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
    let session = spawn(r"printf '\033[31mR\033[38;2;210;110;75mT\033[48;2;0;0;0mB\033[0mD'");
    wait_for(|| session.exited());
    session.with_term(|term| {
        let cells: Vec<_> = term.grid().display_iter().take(4).collect();
        assert_eq!(cells[0].fg, Color::Named(NamedColor::Red));
        assert_eq!(
            cells[1].fg,
            Color::Spec(Rgb {
                r: 210,
                g: 110,
                b: 75
            })
        );
        assert_eq!(cells[2].bg, Color::Spec(Rgb { r: 0, g: 0, b: 0 }));
        assert_eq!(cells[3].bg, Color::Named(NamedColor::Background));
    });
}

#[test]
fn pty_reports_environment_title_bell_and_exit_after_final_output() {
    let mut s = spawn(
        r#"printf '\033]2;demo\007\007%s/%s' "$TERM" "$COLORTERM"; printf '\033[?2026hFINAL'; exit 7"#,
    );
    let mut events = s.take_events().unwrap();
    wait_for(|| s.exited());
    assert!(screen(&s).contains("xterm-256color/truecolorFINAL"));
    let mut collected = Vec::new();
    while let Ok(e) = events.try_recv() {
        collected.push(e);
    }
    assert!(collected.contains(&TerminalEvent::TitleChanged(Some("demo".into()))));
    assert!(collected.contains(&TerminalEvent::Bell));
    assert_eq!(
        collected.last(),
        Some(&TerminalEvent::Exited { code: Some(7) })
    );
}

#[test]
fn resize_reaches_the_child_and_input_roundtrips() {
    let s = spawn("read answer; stty size; printf 'input=%s' \"$answer\"");
    s.resize(TerminalSize {
        cols: 101,
        rows: 33,
        ..Default::default()
    })
    .unwrap();
    s.write(b"hello\r".to_vec());
    wait_for(|| s.exited());
    let out = screen(&s);
    assert!(out.contains("33 101"), "{out}");
    assert!(out.contains("input=hello"), "{out}");
}

#[test]
fn close_kills_jobs_in_other_process_groups_and_reaps_shell() {
    let s = spawn(
        "set -m; /bin/sh -c 'trap \"\" HUP TERM; echo JOB=$$; while :; do sleep 1; done' & wait",
    );
    wait_for(|| screen(&s).contains("JOB="));
    let out = screen(&s);
    let job: i32 = out
        .split("JOB=")
        .nth(1)
        .unwrap()
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap();
    s.close(Duration::from_millis(50));
    assert!(s.exited());
    assert_eq!(unsafe { libc::kill(s.pid() as i32, 0) }, -1);
    wait_for(|| unsafe { libc::kill(job, 0) == -1 });
    s.close(Duration::ZERO);
}

#[test]
fn natural_exit_cleans_stubborn_background_jobs() {
    let s = spawn(
        "/bin/sh -c 'trap \"\" HUP TERM; echo CHILD=$$; while :; do sleep 1; done' & sleep 0.1; exit 0",
    );
    wait_for(|| screen(&s).contains("CHILD="));
    let out = screen(&s);
    let job: i32 = out
        .split("CHILD=")
        .nth(1)
        .unwrap()
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap()
        .parse()
        .unwrap();
    wait_for(|| s.exited());
    wait_for(|| unsafe { libc::kill(job, 0) == -1 });
}

#[test]
fn synchronized_updates_flush_on_timeout_without_more_output() {
    let s = spawn(r#"printf '\033[?2026hBUFFERED'; read ignored"#);
    wait_for(|| screen(&s).contains("BUFFERED"));
    assert!(!s.exited());
    s.close(Duration::from_millis(50));
}

#[test]
fn close_returns_while_a_detached_descendant_floods_the_pty() {
    let pidfile = std::env::temp_dir().join(format!("del-pty-flood-{}", std::process::id()));
    let _ = std::fs::remove_file(&pidfile);
    // setsid moves the writer out of the PTY's session, so termination
    // cannot reach it while it holds the PTY open and keeps writing.
    let s = spawn(&format!(
        "/usr/bin/perl -MPOSIX -e 'POSIX::setsid(); open(F, \">\", $ARGV[0]); print F $$; close F; $|=1; print \"x\" x 79, \"\\n\" while 1' '{}' & sleep 30",
        pidfile.display()
    ));
    wait_for(|| std::fs::read_to_string(&pidfile).is_ok_and(|p| p.trim().parse::<i32>().is_ok()));
    let flood: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    s.close(Duration::ZERO);
    let elapsed = started.elapsed();
    unsafe {
        libc::kill(flood, libc::SIGKILL);
    }
    let _ = std::fs::remove_file(&pidfile);
    assert!(s.exited());
    assert!(
        elapsed < Duration::from_secs(3),
        "close blocked for {elapsed:?}"
    );
}
