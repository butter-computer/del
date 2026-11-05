//! Normal Quit detaches the UI while retaining its live runtime and PTYs.
//! Force Quit, crashes and OS shutdown are intentionally outside this contract.
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
    if should_background(keep_running(), EXIT_REQUESTED.load(Ordering::SeqCst), false) {
        #[cfg(target_os = "macos")]
        unsafe {
            del_session_lifecycle_background();
        }
        #[cfg(not(target_os = "macos"))]
        {
            background_requested();
            cx.hide();
        }
    } else {
        quit_completely(cx);
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
    fn quit_policy_preserves_normal_work_but_allows_real_shutdown() {
        assert!(should_background(true, false, false));
        assert!(!should_background(false, false, false));
        assert!(!should_background(true, true, false));
        assert!(!should_background(true, false, true));
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
