//! Window close can retain the live runtime and PTYs; explicit Quit always exits.
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static KEEP_RUNNING: AtomicBool = AtomicBool::new(true);
static EXIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static BACKGROUND: Mutex<Option<UnboundedSender<()>>> = Mutex::new(None);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct Preferences {
    keep_running: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { keep_running: true }
    }
}

fn read(path: &Path) -> std::io::Result<Preferences> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error),
    }
}
fn write(path: &Path, enabled: bool) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&Preferences {
        keep_running: enabled,
    })?;
    crate::sessions::write_atomic(path, &bytes, crate::sessions::PRIVATE)
}
fn path() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        None
    } else {
        del_agent::data_dir().map(|dir| dir.join("session-lifecycle.json"))
    }
}

pub fn load() {
    if let Some(path) = path() {
        match read(&path) {
            Ok(preferences) => KEEP_RUNNING.store(preferences.keep_running, Ordering::SeqCst),
            Err(error) => eprintln!("Could not read session lifecycle preference: {error}"),
        }
    }
}
pub fn keep_running() -> bool {
    KEEP_RUNNING.load(Ordering::SeqCst)
}
pub fn set_keep_running(enabled: bool) -> std::io::Result<()> {
    if let Some(path) = path() {
        write(&path, enabled)?;
    }
    KEEP_RUNNING.store(enabled, Ordering::SeqCst);
    Ok(())
}

fn should_background(keep: bool, explicit_exit: bool, system_shutdown: bool) -> bool {
    keep && !explicit_exit && !system_shutdown
}
pub fn can_background() -> bool {
    should_background(keep_running(), EXIT_REQUESTED.load(Ordering::SeqCst), false)
}
extern "C" fn native_should_background(system_shutdown: bool) -> bool {
    // Dock Quit and native termination must mean the same thing as Command-Q.
    // The saved background preference applies to window close, never Quit.
    EXIT_REQUESTED.store(true, Ordering::SeqCst);
    should_background(
        keep_running(),
        EXIT_REQUESTED.load(Ordering::SeqCst),
        system_shutdown,
    )
}
extern "C" fn background_requested() {
    if let Some(sender) = BACKGROUND.lock().unwrap().as_ref() {
        let _ = sender.send(());
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn del_session_lifecycle_install(
        decide: extern "C" fn(bool) -> bool,
        notify: extern "C" fn(),
    ) -> bool;
    fn del_session_lifecycle_background();
    fn del_session_lifecycle_foreground();
}

/// Register after AppKit creates the delegate. The notification is delivered on
/// GPUI's executor, avoiding a reentrant borrow inside a native delegate call.
pub fn install() -> UnboundedReceiver<()> {
    let (sender, receiver) = unbounded_channel();
    if cfg!(test) {
        return receiver;
    }
    *BACKGROUND.lock().unwrap() = Some(sender);
    #[cfg(target_os = "macos")]
    if !unsafe { del_session_lifecycle_install(native_should_background, background_requested) } {
        eprintln!("Could not install native background-quit handler");
    }
    receiver
}

pub fn request_quit(cx: &App) {
    quit_completely(cx);
}
pub fn run_in_background(_cx: &App) {
    #[cfg(target_os = "macos")]
    unsafe {
        del_session_lifecycle_background();
    }
    #[cfg(not(target_os = "macos"))]
    {
        background_requested();
        _cx.hide();
    }
}
pub fn quit_completely(cx: &App) {
    EXIT_REQUESTED.store(true, Ordering::SeqCst);
    cx.quit();
}
pub fn reopen(cx: &mut App) {
    #[cfg(target_os = "macos")]
    unsafe {
        del_session_lifecycle_foreground();
    }
    cx.activate(true);
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    #[test]
    fn window_close_policy_preserves_work_but_allows_real_shutdown() {
        assert!(should_background(true, false, false));
        assert!(!should_background(false, false, false));
        assert!(!should_background(true, true, false));
        assert!(!should_background(true, false, true));
    }
    #[::gpui::test]
    fn explicit_quit_exits_even_with_background_sessions_enabled(cx: &mut TestAppContext) {
        let previous_keep = KEEP_RUNNING.swap(true, Ordering::SeqCst);
        let previous_exit = EXIT_REQUESTED.swap(false, Ordering::SeqCst);
        cx.update(|cx| request_quit(cx));
        let menu_requested_exit = EXIT_REQUESTED.load(Ordering::SeqCst);
        EXIT_REQUESTED.store(false, Ordering::SeqCst);
        let dock_would_background = native_should_background(false);
        let dock_requested_exit = EXIT_REQUESTED.load(Ordering::SeqCst);
        KEEP_RUNNING.store(previous_keep, Ordering::SeqCst);
        EXIT_REQUESTED.store(previous_exit, Ordering::SeqCst);
        assert!(menu_requested_exit, "Command-Q must request shutdown");
        assert!(!dock_would_background, "Dock Quit must not hide the app");
        assert!(dock_requested_exit, "Dock Quit must request shutdown");
    }
    #[test]
    fn preference_defaults_on_and_persists_both_choices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-lifecycle.json");
        assert!(read(&path).unwrap().keep_running);
        std::fs::write(&path, "{}").unwrap();
        assert!(read(&path).unwrap().keep_running);
        for enabled in [false, true] {
            write(&path, enabled).unwrap();
            assert_eq!(read(&path).unwrap().keep_running, enabled);
        }
        std::fs::write(&path, "broken").unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
    }
}
