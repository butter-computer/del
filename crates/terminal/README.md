# del-terminal

An embedded Unix terminal with portable-pty, alacritty_terminal, and a GPUI entity.
The view uses Menlo at 13 logical pixels and leaves the default terminal background
transparent so the host's chat surface shows through. Font size, cell geometry,
and input coordinates follow the host's interface zoom. The cursor is hidden when
the terminal or its window is unfocused. Explicit cell backgrounds,
inverse colors, and selection remain visible, including explicit black backgrounds.

```rust,ignore
// In app initialization:
del_terminal::init(cx);
cx.on_app_quit(|_| async {
    del_terminal::shutdown_all(std::time::Duration::from_millis(200));
}).detach();

// In a window, retain this entity for as long as the tab is open:
let session = del_terminal::TerminalSession::spawn(
    del_terminal::TerminalSpec::shell().cwd(project_directory),
)?;
let terminal = cx.new(|cx| del_terminal::TerminalView::new(session, window, cx));
terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
// Render terminal.clone() as a child. Drop all entity references on tab close.
```

`TerminalSpec::shell()` starts `$SHELL` with `-l`, falling back to `/bin/zsh`.

`TerminalSpec::command("claude", ["--help"])` runs a program through the user's interactive login shell so it receives the user's PATH. Set `login_env: false` for direct execution. `env`, `cwd`, initial grid size, and scrollback capacity are configurable. Inherited environment is retained with TERM, COLORTERM, and TERM_PROGRAM defaults. Parent colour overrides (`NO_COLOR`, `FORCE_COLOR`, `CLICOLOR`, and `CLICOLOR_FORCE`) are cleared so applications detect the real PTY and its truecolour support. Explicit `TerminalSpec.env` settings override those defaults. Environment variables that a login startup file reassigns follow that shell's normal behavior.

`TerminalView` emits `TerminalEvent` for title changes, bell, exit status, and repaint wakeups. Subscribe with GPUI's `cx.subscribe` or `cx.subscribe_in`. Alternatively, drive a `TerminalSession` without a view using `take_events`, `write`, `resize`, and `with_term`. Call `ack_wakeup` when handling a repaint. The final exit event follows PTY output draining.

The app host owns provider-specific launch settings, workspace trust, updates,
saved conversation identities, and automatic reopening after a successful CLI
update. Those behaviors are not applied by this generic terminal crate or its
standalone demo. See the [application README](../../README.md#files-and-terminals).

Cmd-C/Cmd-V and Ctrl-Shift-C/Ctrl-Shift-V copy/paste. Drag selects, double-click selects a word, triple-click selects a line. Shift overrides application mouse reporting. Shift-PageUp/PageDown/Home/End scroll the history. The wheel scrolls history or reports to mouse-enabled applications; alternate-screen wheel events map to cursor keys when alternate scrolling is enabled. Option sends an ESC prefix by default; set `option_as_meta = false` for OS Option character composition. Printable text and IME preedit use GPUI's input handler; only committed text reaches the child. Applications cannot read or overwrite the clipboard through OSC 52.

Dropping or explicitly closing a session hangs up its Unix session, including foreground job-control process groups, escalates to SIGKILL after the grace period, and reaps the direct child. The host must call `shutdown_all` on application quit. Programs that deliberately detach into a new Unix session are outside this terminal's session ownership.

The standalone demo accepts a command and arguments, or opens the user's shell:

```sh
cargo run -p del-terminal --example terminal
cargo run -p del-terminal --example terminal -- vim
cargo run -p del-terminal --example terminal -- claude
cargo run -p del-terminal --example terminal -- codex
```

Run the crate checks with:

```sh
cargo test -p del-terminal
cargo clippy -p del-terminal --all-targets -- -D warnings
cargo fmt --all --check
```

Tests cover parsing, colors, Unicode, alternate screen, scrollback, key/mouse encoding, live PTY input/resize/events, and cleanup. Ignored program smoke tests require locally installed tools and explicitly run with `cargo test -p del-terminal --test programs -- --ignored --nocapture`. They do not submit agent prompts. GUI composition, selection, rendering, clipboard, and resize still require interactive validation in the host app.
