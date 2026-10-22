//! Locate the user's current desktop wallpaper and make it displayable.
//!
//! macOS reports the wallpaper through `NSWorkspace.desktopImageURL(for:)`.
//! Rather than binding AppKit, this module asks for it through
//! JavaScript-for-Automation (`osascript -l JavaScript`), which keeps the core
//! crate free of platform crates.
//!
//! The reported URL is only used when it names an image the *user* chose. For
//! every system-provided wallpaper (the dynamic "Macintosh" default, aerials,
//! solid colours, the stock photo library) AppKit still returns a path, but it
//! points into `/System` or an Apple asset cache, so showing it would not match
//! what the user actually sees on their desktop. Those are reported as "no
//! wallpaper" and the caller falls back to the bundled default image.
//!
//! User-chosen HEIC photos cannot be read by plain image decoders, so those
//! are transcoded to JPEG with the system `sips` tool and cached under the
//! user's cache directory.
//!
//! Everything here is best-effort: any failure yields `None`.

use std::ffi::OsStr;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

/// Longest edge, in pixels, of the transcoded copy. Wallpapers are often 5K;
/// this keeps the GPU texture and decode time modest while still exceeding
/// the typical window size.
const MAX_EDGE_PX: u32 = 2560;

/// Image formats the UI layer decodes directly, keyed by lowercase extension.
const NATIVE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Resolve the current desktop wallpaper into a file the UI can decode.
///
/// Returns `None` when the platform is unsupported, the wallpaper is not a
/// user-chosen image file (dynamic, aerial, solid colour, stock), the
/// wallpaper cannot be determined, or transcoding fails.
pub fn current_wallpaper() -> Option<PathBuf> {
    let source = current_wallpaper_source()?;
    displayable_copy(&source, &cache_dir()?)
}

/// Path of the wallpaper as configured by the system, in whatever format the
/// system stores it, provided it is an image file the user chose themselves.
pub fn current_wallpaper_source() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let reported = macos::desktop_image_path()?;
    // Resolve symlinks: the dynamic default is reported as
    // `/System/Library/CoreServices/DefaultDesktop.heic`, a link into
    // `/System/Library/Wallpapers`.
    let path = fs::canonicalize(&reported).ok()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    is_user_image(&path, home.as_deref()).then_some(path)
}

/// Whether `path` (already canonicalized) is a regular file outside every
/// location where macOS keeps its own wallpaper assets.
///
/// `home` is the user's home directory, used to exclude Apple's per-user
/// wallpaper caches.
pub fn is_user_image(path: &Path, home: Option<&Path>) -> bool {
    if !path.is_file() {
        return false;
    }
    if path.starts_with("/System") {
        // Stock wallpapers, dynamic desktops, solid colours and the Tahoe
        // `/System/Library/Wallpapers` tree all live here.
        return false;
    }
    if path.starts_with("/Library/Application Support/com.apple.idleassetsd") {
        // Downloaded aerial wallpaper videos and their still frames.
        return false;
    }
    if let Some(home) = home {
        let user_caches = [
            home.join("Library/Application Support/com.apple.wallpaper"),
            home.join("Library/Application Support/com.apple.idleassetsd"),
        ];
        if user_caches.iter().any(|dir| path.starts_with(dir)) {
            return false;
        }
    }
    true
}

/// Return a path to `source` that a plain image decoder can read: the source
/// itself when its format is natively decodable, otherwise a JPEG transcode
/// cached in `cache_dir`.
pub fn displayable_copy(source: &Path, cache_dir: &Path) -> Option<PathBuf> {
    if !source.is_file() {
        return None;
    }
    if is_natively_decodable(source) {
        return Some(source.to_path_buf());
    }
    let target = cache_dir.join(cache_file_name(source));
    if target.is_file() {
        return Some(target);
    }
    fs::create_dir_all(cache_dir).ok()?;
    if cfg!(target_os = "macos") {
        macos::transcode_to_jpeg(source, &target, MAX_EDGE_PX).then_some(target)
    } else {
        None
    }
}

/// Whether the file extension names a format the UI decodes without help.
pub fn is_natively_decodable(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| NATIVE_EXTENSIONS.contains(&ext.as_str()))
}

/// Stable cache file name for a transcode of `source`.
///
/// The name hashes the source path and its modification time, so a changed
/// wallpaper at the same path gets a fresh transcode.
pub fn cache_file_name(source: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    let modified = fs::metadata(source)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|dur| dur.as_secs())
        .unwrap_or(0);
    modified.hash(&mut hasher);
    format!("wallpaper-{:016x}.jpg", hasher.finish())
}

/// Per-user cache directory for transcoded wallpapers.
fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
    };
    Some(base.join("del"))
}

mod macos {
    use super::*;

    /// Ask AppKit for the main screen's wallpaper URL via JavaScript for
    /// Automation, which can call Objective-C without a native binding.
    pub(super) fn desktop_image_path() -> Option<PathBuf> {
        const SCRIPT: &str = "ObjC.import('AppKit'); \
            $.NSWorkspace.sharedWorkspace\
            .desktopImageURLForScreen($.NSScreen.mainScreen).path.js";
        let output = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", SCRIPT])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?;
        let path = path.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// Transcode `source` to a JPEG at `target`, scaling its longest edge down
    /// to `max_edge` pixels. Uses the system `sips` tool, which understands
    /// HEIC and every other format macOS can display.
    pub(super) fn transcode_to_jpeg(source: &Path, target: &Path, max_edge: u32) -> bool {
        Command::new("sips")
            .args(["-s", "format", "jpeg", "-Z", &max_edge.to_string()])
            .arg(source)
            .arg("--out")
            .arg(target)
            .output()
            .is_ok_and(|output| output.status.success() && target.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_extensions_are_case_insensitive() {
        assert!(is_natively_decodable(Path::new("/a/b.PNG")));
        assert!(is_natively_decodable(Path::new("/a/b.jpeg")));
        assert!(!is_natively_decodable(Path::new("/a/b.heic")));
        assert!(!is_natively_decodable(Path::new("/a/noext")));
    }

    #[test]
    fn cache_name_is_stable_and_path_specific() {
        let a = Path::new("/nonexistent/one.heic");
        let b = Path::new("/nonexistent/two.heic");
        assert_eq!(cache_file_name(a), cache_file_name(a));
        assert_ne!(cache_file_name(a), cache_file_name(b));
        assert!(cache_file_name(a).ends_with(".jpg"));
    }

    #[test]
    fn missing_source_yields_none() {
        let cache = std::env::temp_dir().join("del-wallpaper-test-missing");
        assert_eq!(
            displayable_copy(Path::new("/nonexistent/wallpaper.heic"), &cache),
            None
        );
    }

    #[test]
    fn system_wallpaper_locations_are_not_user_images() {
        let home = Path::new("/Users/someone");
        // A user-chosen image must be a real file to get past `is_file`.
        let dir = std::env::temp_dir().join("del-wallpaper-test-system");
        fs::create_dir_all(&dir).unwrap();
        let user_file = dir.join("photo.png");
        fs::write(&user_file, b"png").unwrap();

        assert!(is_user_image(&user_file, Some(home)));
        assert!(!is_user_image(
            Path::new("/System/Library/Wallpapers/.default/DefaultAerial.heic"),
            Some(home)
        ));
        assert!(!is_user_image(
            Path::new("/System/Library/Desktop Pictures/Solid Colors/Black.png"),
            Some(home)
        ));
        assert!(!is_user_image(
            Path::new("/nonexistent/photo.png"),
            Some(home)
        ));
    }

    #[test]
    fn apple_asset_caches_are_rejected_by_prefix() {
        // Exercise the prefix rules with paths that exist: create files under
        // a fake home so `is_file` passes and only the prefix decides.
        let home = std::env::temp_dir().join("del-wallpaper-test-home");
        let aerial = home.join("Library/Application Support/com.apple.wallpaper/aerials/a.heic");
        fs::create_dir_all(aerial.parent().unwrap()).unwrap();
        fs::write(&aerial, b"heic").unwrap();
        let photo = home.join("Pictures/me.jpg");
        fs::create_dir_all(photo.parent().unwrap()).unwrap();
        fs::write(&photo, b"jpg").unwrap();

        assert!(!is_user_image(&aerial, Some(&home)));
        assert!(is_user_image(&photo, Some(&home)));
    }

    #[test]
    fn native_source_is_returned_as_is() {
        let dir = std::env::temp_dir().join("del-wallpaper-test-native");
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("wp.png");
        fs::write(&src, b"not really a png").unwrap();
        assert_eq!(displayable_copy(&src, &dir).as_deref(), Some(src.as_path()));
    }
}
