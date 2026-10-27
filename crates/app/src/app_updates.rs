//! Updates for the public macOS bundle, using the existing GitHub DMG release.
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const RELEASES: &str = "https://api.github.com/repos/butter-computer/del/releases/latest";
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
const BUNDLE_ID: &str = "computer.butter.del";
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Clone, Debug)]
struct Update {
    version: String,
    dmg: Asset,
    checksum: Asset,
}
struct Staged {
    directory: tempfile::TempDir,
    destination: PathBuf,
    version: String,
}
enum Status {
    Idle,
    Checking,
    Current,
    Available(Update),
    Downloading(u8),
    Verifying,
    Ready(Staged),
    Failed(String),
    Unsupported,
}
enum Event {
    Checked(Result<Option<Update>>),
    Progress(u8),
    Verifying,
    Staged(Result<Staged>),
}

pub struct AppUpdater {
    status: Status,
    bundle: Option<PathBuf>,
    last_check: Option<Instant>,
    receiver: Option<mpsc::Receiver<Event>>,
    restart_error: Option<String>,
}

impl AppUpdater {
    pub fn new(_: &mut Context<Self>) -> Self {
        let bundle = public_bundle();
        let supported = bundle.is_some();
        Self {
            status: if supported {
                Status::Idle
            } else {
                Status::Unsupported
            },
            bundle,
            last_check: None,
            receiver: None,
            restart_error: None,
        }
    }

    // Poll only while a worker is active; idle tab switches do not add a timer.
    fn watch(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.drain(cx);
                        this.receiver.is_some()
                    })
                    .map(|active| !active)
                    .unwrap_or(true)
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        loop {
            let Some(receiver) = self.receiver.as_ref() else {
                break;
            };
            match receiver.try_recv() {
                Ok(event) => {
                    self.status = match event {
                        Event::Checked(result) => {
                            self.receiver = None;
                            match result {
                                Ok(Some(update)) => Status::Available(update),
                                Ok(None) => Status::Current,
                                Err(error) => Status::Failed(error),
                            }
                        }
                        Event::Progress(percent) => Status::Downloading(percent),
                        Event::Verifying => Status::Verifying,
                        Event::Staged(result) => {
                            self.receiver = None;
                            match result {
                                Ok(staged) => Status::Ready(staged),
                                Err(error) => Status::Failed(error),
                            }
                        }
                    };
                    cx.notify();
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.status =
                        Status::Failed("Update worker stopped unexpectedly. Please retry.".into());
                    cx.notify();
                    break;
                }
            }
        }
    }

    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.bundle.is_none()
            || self.receiver.is_some()
            || matches!(self.status, Status::Ready(_) | Status::Available(_))
            || (!manual
                && self
                    .last_check
                    .is_some_and(|time| time.elapsed() < CHECK_INTERVAL))
        {
            return;
        }
        self.last_check = Some(Instant::now());
        self.status = Status::Checking;
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        Self::watch(cx);
        std::thread::spawn(move || {
            let _ = tx.send(Event::Checked(check_release()));
        });
        cx.notify();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        let Status::Available(update) = &self.status else {
            return;
        };
        let update = update.clone();
        let Some(bundle) = self.bundle.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        Self::watch(cx);
        self.status = Status::Downloading(0);
        std::thread::spawn(move || {
            let result = stage_update(update, bundle, &tx);
            let _ = tx.send(Event::Staged(result));
        });
        cx.notify();
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        let Status::Ready(staged) = &self.status else {
            return;
        };
        match install_and_relaunch(staged) {
            Ok(()) => cx.quit(),
            Err(error) => {
                // Keep a verified download when the directory is temporarily locked
                // or not writable; the user can retry without downloading again.
                self.restart_error = Some(error);
                cx.notify();
            }
        }
    }
}

pub fn render(updater: &Entity<AppUpdater>, cx: &App) -> AnyElement {
    let state = updater.read(cx);
    let status = &state.status;
    let (label, mut tooltip, disabled) = match status {
        Status::Idle => (
            "Check for updates".into(),
            "Check for a newer version of Del".into(),
            false,
        ),
        Status::Checking => (
            "Checking for updates…".into(),
            "Checking GitHub releases".into(),
            true,
        ),
        Status::Current => (
            "Del is up to date".into(),
            "Click to check again".into(),
            false,
        ),
        Status::Available(update) => (
            format!("Update to {}", update.version),
            "Download the latest version of Del".into(),
            false,
        ),
        Status::Downloading(percent) => (
            format!("Downloading… {percent}%"),
            "You can keep working while Del downloads".into(),
            true,
        ),
        Status::Verifying => (
            "Preparing update…".into(),
            "Verifying and staging the downloaded app".into(),
            true,
        ),
        Status::Ready(staged) => (
            if state.restart_error.is_some() {
                "Retry restart".into()
            } else {
                "Restart to update".into()
            },
            format!(
                "Del {} is ready. Restart whenever you're ready.",
                staged.version
            ),
            false,
        ),
        Status::Failed(error) => (
            "Retry update check".into(),
            format!("{error}\nClick to retry."),
            false,
        ),
        Status::Unsupported => return div().into_any_element(),
    };
    if let Some(error) = &state.restart_error {
        tooltip = format!("{error}\nClick to retry restarting.");
    }
    let updater = updater.clone();
    div().debug_selector(|| "del-update".into()).flex_none().w_full().child(
        Button::new("del-update").ghost().compact().w_full().accessibility_label(label.clone()).label(label)
            .tooltip(tooltip).disabled(disabled)
            .on_click(move |_, window, cx| {
                if matches!(updater.read(cx).status, Status::Ready(_)) {
                    let updater = updater.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let updater = updater.clone();
                        dialog.title("Restart to update Del?")
                            .child(div().child("Your saved sessions will remain available. Running agents and terminals will stop when Del restarts."))
                            .footer(gpui_kit::component::dialog::DialogFooter::new()
                                .child(Button::new("update-later").label("Later").on_click(|_, window, cx| window.close_dialog(cx)))
                                .child(Button::new("update-restart").label("Restart now").on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let error = updater.update(cx, |updater, cx| {
                                        updater.restart(cx);
                                        updater.restart_error.clone()
                                    });
                                    if let Some(error) = error {
                                        window.push_notification(gpui_kit::component::notification::Notification::error(error)
                                            .title("Could not restart Del").autohide(false), cx);
                                    }
                                })))
                    });
                } else {
                    updater.update(cx, |updater, cx| {
                        if matches!(updater.status, Status::Available(_)) { updater.download(cx); }
                        else { updater.check(true, cx); }
                    });
                }
            })
    ).into_any_element()
}

fn version(value: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<_> = value
        .strip_prefix('v')
        .unwrap_or(value)
        .split('.')
        .collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    bounded_output(command, Duration::from_secs(60))
}

fn bounded_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>> {
    use std::io::{Read, Seek};
    // Files avoid a full pipe deadlocking a subprocess while we enforce a deadline.
    let mut stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = command
        .stdout(stdout.try_clone().map_err(|e| e.to_string())?)
        .stderr(stderr.try_clone().map_err(|e| e.to_string())?)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => e.to_string(),
                    _ => format!(
                        "{} timed out. Please retry.",
                        command.get_program().to_string_lossy()
                    ),
                });
            }
        }
    };
    stdout.rewind().map_err(|e| e.to_string())?;
    stderr.rewind().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    if !status.success() {
        stderr
            .take(4096)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        return Err(format!(
            "{} failed: {}",
            command.get_program().to_string_lossy(),
            String::from_utf8_lossy(&bytes).trim()
        ));
    }
    if stdout.metadata().map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("Update command returned too much data".into());
    }
    stdout.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn curl(url: &str) -> Command {
    let mut command = Command::new("/usr/bin/curl");
    // Ignore ~/.curlrc so updater behavior cannot silently inherit proxy,
    // output-path, or certificate-validation overrides from an interactive setup.
    command.args([
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--max-time",
        "600",
        "--user-agent",
        "Del-updater",
        url,
    ]);
    command
}

fn check_release() -> Result<Option<Update>> {
    let bytes = output(curl(RELEASES).args(["--max-time", "30"]))?;
    let release: Release =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid release metadata: {e}"))?;
    select_release(release, env!("CARGO_PKG_VERSION"))
}

fn select_release(release: Release, installed: &str) -> Result<Option<Update>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let latest = version(&release.tag_name).ok_or("Unsupported release version")?;
    let current = version(installed).ok_or("Unsupported installed version")?;
    if latest <= current {
        return Ok(None);
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_owned();
    let filename = format!("Del-{version}-arm64.dmg");
    let asset = |name: &str| -> Result<Asset> {
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("Release is missing {name}"))?;
        let expected = format!(
            "https://github.com/butter-computer/del/releases/download/{}/{name}",
            release.tag_name
        );
        let maximum = if name.ends_with(".sha256") {
            4096
        } else {
            1024 * 1024 * 1024
        };
        if asset.browser_download_url != expected || asset.size == 0 || asset.size > maximum {
            return Err("Invalid release asset".into());
        }
        Ok(asset.clone())
    };
    Ok(Some(Update {
        dmg: asset(&filename)?,
        checksum: asset(&format!("{filename}.sha256"))?,
        version,
    }))
}

fn plist_value(bundle: &Path, key: &str) -> Result<String> {
    let bytes = output(
        Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw", "-o", "-"])
            .arg(bundle.join("Contents/Info.plist")),
    )?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

fn public_bundle() -> Option<PathBuf> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return None;
    }
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = executable.parent()?.parent()?.parent()?;
    if bundle.extension()? != "app" || plist_value(bundle, "CFBundleIdentifier").ok()? != BUNDLE_ID
    {
        return None;
    }
    Some(bundle.to_owned())
}

fn checksum_matches(sidecar: &str, filename: &str, actual: &str) -> bool {
    let fields: Vec<_> = sidecar.split_whitespace().collect();
    fields.len() == 2
        && fields[1] == filename
        && fields[0].len() == 64
        && fields[0].bytes().all(|b| b.is_ascii_hexdigit())
        && fields[0].eq_ignore_ascii_case(actual)
}

struct Mounted(PathBuf);
impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = output(
            Command::new("/usr/bin/hdiutil")
                .args(["detach", "-quiet"])
                .arg(&self.0),
        );
    }
}

fn stage_update(update: Update, destination: PathBuf, tx: &mpsc::Sender<Event>) -> Result<Staged> {
    let download = tempfile::tempdir().map_err(|e| e.to_string())?;
    let dmg = download.path().join(&update.dmg.name);
    let stderr =
        std::fs::File::create(download.path().join("download-error")).map_err(|e| e.to_string())?;
    let mut child = curl(&update.dmg.browser_download_url)
        .arg("--output")
        .arg(&dmg)
        .stderr(stderr)
        .spawn()
        .map_err(|e| e.to_string())?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err("Download failed. Check your connection and try again.".into());
                }
                break;
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
        let bytes = std::fs::metadata(&dmg).map(|m| m.len()).unwrap_or(0);
        let progress = ((bytes as f64 / update.dmg.size as f64) * 100.).min(99.) as u8;
        if bytes > update.dmg.size || tx.send(Event::Progress(progress)).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Update download cancelled or exceeded the expected size".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    tx.send(Event::Verifying).map_err(|_| "Update cancelled")?;
    if std::fs::metadata(&dmg).map_err(|e| e.to_string())?.len() != update.dmg.size {
        return Err("Downloaded update has the wrong size".into());
    }
    let sidecar = output(&mut curl(&update.checksum.browser_download_url))?;
    let hash = output(
        Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&dmg),
    )?;
    let hash = String::from_utf8_lossy(&hash);
    if !checksum_matches(
        &String::from_utf8_lossy(&sidecar),
        &update.dmg.name,
        hash.split_whitespace().next().unwrap_or(""),
    ) {
        return Err("Update checksum verification failed".into());
    }
    let mount = download.path().join("mounted");
    std::fs::create_dir(&mount).map_err(|e| e.to_string())?;
    output(
        Command::new("/usr/bin/hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg),
    )?;
    let _mounted = Mounted(mount.clone());
    let source = mount.join("Del.app");
    verify_bundle(&source, &update.version)?;
    // Same filesystem as the installed app: replacement is a rename, never a partial copy.
    let parent = destination
        .parent()
        .ok_or("Cannot locate the installation directory")?;
    let directory = tempfile::Builder::new().prefix(".del-update-").tempdir_in(parent)
        .map_err(|e| format!("Cannot update this location. Move Del to a writable Applications folder and retry: {e}"))?;
    let staged = directory.path().join("Del.app");
    output(Command::new("/usr/bin/ditto").arg(&source).arg(&staged))?;
    verify_bundle(&staged, &update.version)?;
    Ok(Staged {
        directory,
        destination,
        version: update.version,
    })
}

fn verify_bundle(bundle: &Path, version: &str) -> Result<()> {
    if plist_value(bundle, "CFBundleIdentifier")? != BUNDLE_ID
        || plist_value(bundle, "CFBundleShortVersionString")? != version
        || plist_value(bundle, "CFBundleExecutable")? != "del"
        || !bundle.join("Contents/MacOS/del").is_file()
    {
        return Err("Downloaded app does not match the selected Del release".into());
    }
    output(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(bundle),
    )?;
    Ok(())
}

fn replace_bundle(source: &Path, destination: &Path, retired: &Path) -> Result<()> {
    std::fs::rename(destination, retired)
        .map_err(|e| format!("Cannot move the installed app: {e}"))?;
    if let Err(error) = std::fs::rename(source, destination) {
        std::fs::rename(retired, destination).map_err(|rollback| {
            format!(
                "Update failed: {error}; restore {} to {}: {rollback}",
                retired.display(),
                destination.display()
            )
        })?;
        return Err(format!(
            "Cannot install update: {error}. The previous app was restored."
        ));
    }
    Ok(())
}

fn validate_installation(staged: &Staged, current_version: &str) -> Result<()> {
    verify_bundle(&staged.destination, current_version).map_err(|_| {
        "The installed app changed since Del started. Quit and reopen Del before updating."
            .to_string()
    })?;
    verify_bundle(&staged.directory.path().join("Del.app"), &staged.version)?;
    if version(&staged.version) <= version(current_version) {
        return Err("Refusing to install an older or identical version".into());
    }
    Ok(())
}

fn install_and_relaunch(staged: &Staged) -> Result<()> {
    let parent = staged
        .destination
        .parent()
        .ok_or("Missing installation directory")?;
    // Share the installer lock with scripts/install-macos.py.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(parent.join(".del-install.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "Another Del installation is in progress. Try again shortly.")?;
    // Another running copy or an external installer may have replaced the app
    // since this download began. Never overwrite a different/newer installation.
    validate_installation(staged, env!("CARGO_PKG_VERSION"))?;
    let retired_dir = parent.join(".del-retired");
    std::fs::create_dir_all(&retired_dir).map_err(|e| e.to_string())?;
    let retired = retired_dir.join(format!("del-{}.app", uuid::Uuid::new_v4()));
    let source = staged.directory.path().join("Del.app");
    replace_bundle(&source, &staged.destination, &retired)?;
    // Retain the running bundle (including its signed resources) until after exit.
    // Arguments, not shell interpolation, carry all paths and the process ID.
    match Command::new("/bin/sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 1; done; /usr/bin/open -n \"$2\"",
            "del-relaunch",
        ])
        .arg(std::process::id().to_string())
        .arg(&staged.destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(error) => {
            std::fs::rename(&staged.destination, &source)
                .map_err(|e| format!("Relaunch failed: {error}; rollback failed: {e}"))?;
            std::fs::rename(&retired, &staged.destination)
                .map_err(|e| format!("Relaunch failed: {error}; rollback failed: {e}"))?;
            Err(format!("Could not schedule restart: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppUpdater, Asset, Event, RELEASES, Release, Status, checksum_matches, curl, output,
        replace_bundle, select_release, stage_update, verify_bundle, version,
    };
    use gpui::AppContext;
    use std::{path::PathBuf, sync::mpsc, time::Instant};
    fn release(tag: &str) -> Release {
        let v = tag.trim_start_matches('v');
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: [
                format!("Del-{v}-arm64.dmg"),
                format!("Del-{v}-arm64.dmg.sha256"),
            ]
            .into_iter()
            .map(|name| Asset {
                browser_download_url: format!(
                    "https://github.com/butter-computer/del/releases/download/{tag}/{name}"
                ),
                name,
                size: 10,
            })
            .collect(),
        }
    }
    #[test]
    #[ignore = "downloads and verifies the public DMG; does not replace an installed app"]
    fn public_release_download_and_staging() {
        let bytes = output(&mut curl(RELEASES)).unwrap();
        let release = serde_json::from_slice(&bytes).unwrap();
        let update = select_release(release, "0.0.0").unwrap().unwrap();
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("Del.app");
        let (tx, rx) = mpsc::channel();
        let staged = stage_update(update, destination.clone(), &tx).unwrap();
        assert!(
            !destination.exists(),
            "staging must not install before consent"
        );
        verify_bundle(&staged.directory.path().join("Del.app"), &staged.version).unwrap();
        assert!(rx.try_iter().any(|event| matches!(event, Event::Verifying)));
    }

    #[gpui::test]
    fn checks_are_throttled_and_never_interrupt_a_pending_update(cx: &mut gpui::TestAppContext) {
        let updater = cx.new(AppUpdater::new);
        updater.update(cx, |updater, cx| {
            // A fake bundle enables the checks without modifying any installation.
            updater.bundle = Some(PathBuf::from("/fake/Del.app"));
            updater.status = Status::Current;
            updater.last_check = Some(Instant::now());
            updater.check(false, cx);
            assert!(matches!(updater.status, Status::Current));
            assert!(updater.receiver.is_none());
            updater.status =
                Status::Available(select_release(release("v0.2.0"), "0.1.0").unwrap().unwrap());
            updater.check(true, cx);
            assert!(matches!(updater.status, Status::Available(_)));
            let (_tx, rx) = mpsc::channel();
            updater.receiver = Some(rx);
            updater.status = Status::Downloading(42);
            updater.check(true, cx);
            assert!(matches!(updater.status, Status::Downloading(42)));
        });
    }

    #[gpui::test]
    fn update_control_renders_each_actionable_state(cx: &mut gpui::TestAppContext) {
        use gpui::{Context, Entity, IntoElement, ParentElement, Render, Styled, Window};
        struct Fixture(Entity<AppUpdater>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                gpui::div()
                    .w(gpui::px(180.))
                    .child(super::render(&self.0, cx))
            }
        }
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, cx| Fixture(cx.new(AppUpdater::new)));
        let pending = select_release(release("v0.2.0"), "0.1.0").unwrap().unwrap();
        for status in [
            Status::Idle,
            Status::Checking,
            Status::Current,
            Status::Available(pending),
            Status::Downloading(42),
            Status::Verifying,
            Status::Ready(super::Staged {
                directory: tempfile::tempdir().unwrap(),
                destination: PathBuf::from("/fake/Del.app"),
                version: "0.2.0".into(),
            }),
            Status::Failed("Network unavailable".into()),
        ] {
            cx.update(|window, cx| {
                view.update(cx, |fixture, cx| {
                    fixture.0.update(cx, |updater, cx| {
                        updater.status = status;
                        cx.notify();
                    });
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(cx.debug_bounds("del-update").is_some());
        }
    }

    #[gpui::test]
    fn worker_disconnect_and_network_errors_are_retryable(cx: &mut gpui::TestAppContext) {
        let updater = cx.new(AppUpdater::new);
        updater.update(cx, |updater, cx| {
            let (tx, rx) = mpsc::channel();
            updater.receiver = Some(rx);
            updater.status = Status::Checking;
            tx.send(Event::Checked(Err("Offline".into()))).unwrap();
            updater.drain(cx);
            assert!(matches!(&updater.status, Status::Failed(message) if message == "Offline"));
            assert!(updater.receiver.is_none());
            let (tx, rx) = mpsc::channel();
            updater.receiver = Some(rx);
            updater.status = Status::Downloading(20);
            drop(tx);
            updater.drain(cx);
            assert!(matches!(updater.status, Status::Failed(_)));
            assert!(updater.receiver.is_none());
        });
    }

    #[gpui::test]
    fn locked_installer_keeps_the_staged_update_for_retry(cx: &mut gpui::TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.path().join(".del-install.lock"))
            .unwrap();
        lock.lock().unwrap();
        let updater = cx.new(AppUpdater::new);
        updater.update(cx, |updater, cx| {
            updater.status = Status::Ready(super::Staged {
                directory: tempfile::tempdir_in(root.path()).unwrap(),
                destination: root.path().join("Del.app"),
                version: "0.2.0".into(),
            });
            updater.restart(cx);
            assert!(matches!(updater.status, Status::Ready(_)));
            assert!(
                updater
                    .restart_error
                    .as_ref()
                    .unwrap()
                    .contains("Another Del installation")
            );
        });
    }

    #[test]
    fn external_commands_are_bounded_and_errors_are_reported() {
        use std::{process::Command, time::Duration};
        let error = super::bounded_output(
            Command::new("/bin/sleep").arg("5"),
            Duration::from_millis(30),
        )
        .unwrap_err();
        assert!(error.contains("timed out"));
        let error =
            super::output(Command::new("/bin/sh").args(["-c", "printf failure >&2; exit 7"]))
                .unwrap_err();
        assert!(error.contains("failure"));
    }

    #[test]
    fn malformed_versions_drafts_and_oversized_assets_are_rejected() {
        for invalid in [
            "v1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "../1.2.3",
            "1.2.-3",
            "1.2.99999999999999999999999999",
        ] {
            assert!(version(invalid).is_none());
        }
        let mut draft = release("v0.2.0");
        draft.draft = true;
        assert!(select_release(draft, "0.1.0").unwrap().is_none());
        for index in [0, 1] {
            let mut oversized = release("v0.2.0");
            oversized.assets[index].size = u64::MAX;
            assert!(select_release(oversized, "0.1.0").is_err());
            let mut empty = release("v0.2.0");
            empty.assets[index].size = 0;
            assert!(select_release(empty, "0.1.0").is_err());
        }
    }

    #[test]
    fn release_selection_rejects_old_incomplete_and_untrusted_releases() {
        assert!(
            select_release(release("v0.1.0"), "0.1.0")
                .unwrap()
                .is_none()
        );
        assert!(
            select_release(release("v0.1.0"), "0.2.0")
                .unwrap()
                .is_none()
        );
        assert!(
            select_release(release("v0.10.0"), "0.9.0")
                .unwrap()
                .is_some()
        );
        let mut r = release("v0.2.0");
        r.prerelease = true;
        assert!(select_release(r, "0.1.0").unwrap().is_none());
        let mut r = release("v0.2.0");
        r.assets.pop();
        assert!(select_release(r, "0.1.0").is_err());
        let mut r = release("v0.2.0");
        r.assets[0].browser_download_url = "https://example.org/app.dmg".into();
        assert!(select_release(r, "0.1.0").is_err());
        assert!(version("0.2.0/../app").is_none());
    }
    #[test]
    fn checksum_requires_matching_digest_and_filename() {
        let digest = "a".repeat(64);
        assert!(checksum_matches(
            &format!("{digest}  Del.dmg\n"),
            "Del.dmg",
            &digest
        ));
        assert!(!checksum_matches(
            &format!("{digest}  Other.dmg"),
            "Del.dmg",
            &digest
        ));
        assert!(!checksum_matches(
            &format!("{digest}  Del.dmg"),
            "Del.dmg",
            &"b".repeat(64)
        ));
        assert!(!checksum_matches("bad Del.dmg", "Del.dmg", "bad"));
    }
    #[test]
    fn replacement_preserves_old_bundle_and_rolls_back_failed_install() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Del.app");
        let source = root.path().join("New.app");
        let retired = root.path().join("Old.app");
        std::fs::create_dir(&installed).unwrap();
        std::fs::write(installed.join("version"), "old").unwrap();
        assert!(replace_bundle(&source, &installed, &retired).is_err());
        assert_eq!(
            std::fs::read_to_string(installed.join("version")).unwrap(),
            "old"
        );
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("version"), "new").unwrap();
        replace_bundle(&source, &installed, &retired).unwrap();
        assert_eq!(
            std::fs::read_to_string(installed.join("version")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(retired.join("version")).unwrap(),
            "old"
        );
    }
}
