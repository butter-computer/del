//! `cargo run -p del-terminal --example terminal -- [program [args...]]`
use del_terminal::{TerminalSession, TerminalSpec, TerminalView, shutdown_all};
use gpui::{prelude::*, *};
use std::time::Duration;

actions!(terminal_demo, [Quit]);

fn main() {
    let mut args = std::env::args().skip(1);
    let spec = match args.next() {
        Some(program) => TerminalSpec::command(program, args),
        None => TerminalSpec::shell(),
    }
    .cwd(std::env::current_dir().expect("working directory"));
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        del_terminal::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_app_quit(|_| async {
            shutdown_all(Duration::from_millis(200));
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("del terminal".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(960.), px(640.)),
                cx,
            ))),
            window_min_size: Some(size(px(320.), px(160.))),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            let session = TerminalSession::spawn(spec).expect("spawn terminal");
            let view = cx.new(|cx| TerminalView::new(session, window, cx));
            view.update(cx, |view, cx| view.focus(window, cx));
            view
        })
        .expect("open terminal");
        cx.activate(true);
    });
}
