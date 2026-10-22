//! Opt-in smoke tests: never send prompts or accept trust/configuration dialogs.
use alacritty_terminal::{grid::Dimensions, term::TermMode};
use del_terminal::{TerminalSession, TerminalSpec};
use std::time::{Duration, Instant};

fn screen(s: &TerminalSession) -> String {
    s.with_term(|t| t.grid().display_iter().map(|c| c.c).collect())
}
fn wait_for(s: &TerminalSession, label: &str, mut predicate: impl FnMut(&str) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        let text = screen(s);
        if predicate(&text) {
            return;
        }
        assert!(!s.exited(), "{label} exited early: {text}");
        assert!(Instant::now() < end, "{label} timed out: {text}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "requires local vim"]
fn vim_renders_and_accepts_input() {
    let s = TerminalSession::spawn(TerminalSpec::command(
        "/usr/bin/vim",
        ["-Nu", "NONE", "-n", "-i", "NONE"],
    ))
    .unwrap();
    wait_for(&s, "vim", |text| text.contains("VIM") || text.contains("~"));
    assert!(s.mode().contains(TermMode::ALT_SCREEN));
    s.write(b"iDEL_VIM_INPUT\x1b".to_vec());
    wait_for(&s, "vim input", |text| text.contains("DEL_VIM_INPUT"));
    s.write(b":q!\r".to_vec());
    let end = Instant::now() + Duration::from_secs(5);
    while !s.exited() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(s.exited());
    println!("vim: alternate screen, insert input, clean exit verified");
}

#[test]
#[ignore = "requires local top"]
fn top_redraws_after_resize() {
    let s = TerminalSession::spawn(TerminalSpec::command(
        "/usr/bin/top",
        ["-s", "1", "-o", "cpu"],
    ))
    .unwrap();
    wait_for(&s, "top", |text| {
        text.contains("Processes:") && text.contains("CPU usage:")
    });
    s.resize(del_terminal::TerminalSize {
        cols: 100,
        rows: 30,
        ..Default::default()
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(s.with_term(|t| (t.columns(), t.screen_lines())), (100, 30));
    assert!(screen(&s).contains("CPU usage:"));
    s.write(b"q".to_vec());
    s.close(Duration::from_millis(100));
    println!("top: live redraw and resized grid verified");
}

fn agent(program: &str, marker: &str) {
    let s = TerminalSession::spawn(
        TerminalSpec::command(program, std::iter::empty::<&str>()).cwd("/private/tmp"),
    )
    .unwrap();
    wait_for(&s, program, |text| {
        text.to_ascii_lowercase().contains(marker)
    });
    let initial = screen(&s);
    if program == "claude" && initial.contains("Esc to cancel") {
        std::thread::sleep(Duration::from_secs(1));
        s.write(b"\x1b".to_vec());
        let end = Instant::now() + Duration::from_secs(5);
        while !s.exited() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !s.exited() {
            s.write(b"\x03".to_vec());
            std::thread::sleep(Duration::from_millis(300));
            s.write(b"\x03".to_vec());
            let end = Instant::now() + Duration::from_secs(5);
            while !s.exited() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let cancelled = s.exited();
        s.close(Duration::from_millis(200));
        assert!(
            cancelled,
            "Claude rendered trust dialog but did not respond to Escape or Ctrl-C"
        );
        println!(
            "claude: trust dialog rendered; cancel/interrupt exited; no workspace trust granted"
        );
        return;
    }
    s.write(
        del_terminal::keys::encode_key(&gpui::Keystroke::parse("down").unwrap(), s.mode(), true)
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(200));
    let moved = screen(&s);
    let initial_cursor = s.with_term(|t| t.grid().cursor.point);
    // A printable character may enter a prompt, but is never submitted.
    s.write(b"z".to_vec());
    std::thread::sleep(Duration::from_millis(200));
    let typed = screen(&s);
    let typed_cursor = s.with_term(|t| t.grid().cursor.point);
    let changed = initial != moved || moved != typed || initial_cursor != typed_cursor;
    s.close(Duration::from_millis(200));
    assert!(
        changed,
        "{program} rendered but no input response was observed: {typed}"
    );
    println!("{program}: rendered and responded to input; no prompt submitted or dialog accepted");
}

#[test]
#[ignore = "requires installed/authenticated Claude CLI"]
fn claude_renders_and_accepts_input() {
    agent("claude", "claude");
}

#[test]
#[ignore = "requires installed/authenticated Codex CLI"]
fn codex_renders_and_accepts_input() {
    agent("codex", "codex");
}
