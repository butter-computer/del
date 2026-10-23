//! Persisted zoom. Both rem-based components and explicit pixel layouts scale.
use gpui_kit::{App, Pixels};
use std::path::{Path, PathBuf};

pub fn px(value: f32) -> Pixels {
    gpui_kit::px(del_core::zoom::scaled(value))
}
fn path() -> PathBuf {
    del_agent::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("zoom.json")
}
fn read(path: &Path) -> u16 {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<i16>(&bytes).ok())
        .map(del_core::zoom::clamp)
        .unwrap_or(del_core::zoom::DEFAULT)
}
fn save(path: &Path, percent: u16) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, percent.to_string())?;
    std::fs::rename(temporary, path)
}
pub fn load() {
    del_core::zoom::set(read(&path()) as i16);
}

#[cfg(test)]
mod tests {
    #[test]
    fn persists_resets_clamps_and_recovers_invalid_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("zoom.json");
        assert_eq!(super::read(&path), 100);
        super::save(&path, 140).unwrap();
        assert_eq!(super::read(&path), 140);
        super::save(&path, 100).unwrap();
        assert_eq!(super::read(&path), 100);
        std::fs::write(&path, "999").unwrap();
        assert_eq!(super::read(&path), del_core::zoom::MAX);
        std::fs::write(&path, "broken").unwrap();
        assert_eq!(super::read(&path), 100);
    }
    struct ZoomFixture {
        terminal: gpui::Entity<del_terminal::TerminalView>,
        input: gpui::Entity<gpui_kit::component::input::InputState>,
    }
    impl gpui::Render for ZoomFixture {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{InteractiveElement, ParentElement, Styled};
            use gpui_kit::component::ActiveTheme;
            window.set_rem_size(cx.theme().font_size);
            gpui::div()
                .flex()
                .flex_col()
                .w(gpui::px(600.))
                .h(gpui::px(500.))
                .child(
                    gpui::div()
                        .debug_selector(|| "zoom-box".into())
                        .flex_none()
                        .w(super::px(100.))
                        .h(super::px(20.)),
                )
                .child(gpui_kit::component::input::Input::new(&self.input))
                .child(
                    gpui::div()
                        .w_full()
                        .h(gpui::px(200.))
                        .child(self.terminal.clone()),
                )
        }
    }

    #[gpui::test]
    fn shortcuts_scale_layout_and_terminal_even_with_input_focus(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("zoom.json");
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            gpui_kit::component::Theme::update(cx, |theme| {
                theme.font_size = super::px(del_core::typography::TEXT_MD)
            });
            del_terminal::init(cx);
            super::install(cx, path.clone());
        });
        let (root, cx) = cx.add_window_view(|window, cx| {
            let session = del_terminal::TerminalSession::spawn(del_terminal::TerminalSpec {
                program: Some("/bin/cat".into()),
                login_env: false,
                ..Default::default()
            })
            .unwrap();
            // Finish the real PTY before attaching it to GPUI's deterministic
            // scheduler; the retained terminal grid still exercises sizing,
            // rendering and terminal-focused shortcut dispatch.
            session.close(std::time::Duration::ZERO);
            let terminal = cx.new(|cx| del_terminal::TerminalView::new(session, window, cx));
            let input = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
            terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
            ZoomFixture { terminal, input }
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let terminal = root.read_with(cx, |root, _| root.terminal.clone());
        let original = terminal.read_with(cx, |view, _| view.session().size());
        cx.simulate_keystrokes("cmd-shift-=");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(del_core::zoom::percent(), 110);
        assert!((cx.debug_bounds("zoom-box").unwrap().size.width.as_f32() - 110.).abs() < 0.01);
        let enlarged = terminal.read_with(cx, |view, _| view.session().size());
        assert!(enlarged.cols < original.cols && enlarged.rows < original.rows);
        assert_eq!(super::read(&path), 110);
        cx.simulate_keystrokes("cmd-+");
        assert_eq!(del_core::zoom::percent(), 120);
        let input = root.read_with(cx, |root, _| root.input.clone());
        cx.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
        cx.simulate_keystrokes("cmd--");
        assert_eq!(del_core::zoom::percent(), 110);
        cx.simulate_keystrokes("cmd-=");
        assert_eq!(del_core::zoom::percent(), 110);
        cx.simulate_keystrokes("cmd-0");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(del_core::zoom::percent(), 100);
        assert_eq!(super::read(&path), 100);
        assert_eq!(
            cx.debug_bounds("zoom-box").unwrap().size.width,
            gpui::px(100.)
        );
        assert_eq!(
            terminal.read_with(cx, |view, _| view.session().size()),
            original
        );
        terminal.read_with(cx, |view, _| view.close());
    }
}

gpui_kit::actions!(del_zoom, [ZoomIn, ZoomOut, ResetZoom]);

pub fn init(cx: &mut App) {
    install(cx, path());
}
fn install(cx: &mut App, path: PathBuf) {
    use gpui_kit::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd-shift-=", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ResetZoom, None),
    ]);
    let apply = move |percent, cx: &mut App| {
        del_core::zoom::set(percent);
        gpui_kit::component::Theme::update(cx, |theme| {
            theme.font_size = px(del_core::typography::TEXT_MD);
        });
        cx.refresh_windows();
        if let Err(error) = save(&path, del_core::zoom::percent()) {
            eprintln!("[del zoom] Could not save zoom: {error}");
        }
    };
    let increase = apply.clone();
    cx.on_action(move |_: &ZoomIn, cx| {
        increase(del_core::zoom::percent() as i16 + del_core::zoom::STEP, cx)
    });
    let decrease = apply.clone();
    cx.on_action(move |_: &ZoomOut, cx| {
        decrease(del_core::zoom::percent() as i16 - del_core::zoom::STEP, cx)
    });
    cx.on_action(move |_: &ResetZoom, cx| apply(del_core::zoom::DEFAULT as i16, cx));
}
