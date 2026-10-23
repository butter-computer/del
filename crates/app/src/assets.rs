//! App icons embedded in the binary, layered over the component library's own
//! assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    SidebarIcons,
    [
        Pencil,
        Archive,
        Pin,
        PinOff,
        Close,
        Coffee,
        Eye,
        Trash,
        RefreshCw,
        Target,
        Pause,
        Play,
        ChevronDown,
        ChevronUp
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = SidebarIcons.load(path)? {
            return Ok(Some(bytes));
        }
        let bytes: Option<&'static [u8]> = match path {
            "icons/skill.svg" => Some(include_bytes!("../assets/icons/skill.svg")),
            "icons/microphone.svg" => Some(include_bytes!("../assets/icons/microphone.svg")),
            "icons/claude.svg" => Some(include_bytes!("../assets/icons/claude.svg")),
            "icons/codex.svg" => Some(include_bytes!("../assets/icons/codex.svg")),
            "icons/acp.svg" => Some(include_bytes!("../assets/icons/acp.svg")),
            "icons/zap.svg" => Some(include_bytes!("../assets/icons/zap.svg")),
            "icons/pin-outline.svg" => Some(include_bytes!("../assets/icons/pin-outline.svg")),
            "icons/pin-filled.svg" => Some(include_bytes!("../assets/icons/pin-filled.svg")),
            "icons/coffee-cup.svg" => Some(include_bytes!("../assets/icons/coffee-cup.svg")),
            "icons/coffee-steam.svg" => Some(include_bytes!("../assets/icons/coffee-steam.svg")),
            "icons/steer.svg" => Some(include_bytes!("../assets/icons/steer.svg")),
            "icons/grip.svg" => Some(include_bytes!("../assets/icons/grip.svg")),
            "icons/queue.svg" => Some(include_bytes!("../assets/icons/queue.svg")),
            "icons/restore.svg" => Some(include_bytes!("../assets/icons/restore.svg")),
            "icons/archive.svg" => Some(include_bytes!("../assets/icons/archive.svg")),
            "icons/filetypes/pdf.png" => Some(include_bytes!("../assets/icons/filetypes/pdf.png")),
            "icons/filetypes/docx.png" => {
                Some(include_bytes!("../assets/icons/filetypes/docx.png"))
            }
            "icons/filetypes/xlsx.png" => {
                Some(include_bytes!("../assets/icons/filetypes/xlsx.png"))
            }
            "icons/filetypes/pptx.png" => {
                Some(include_bytes!("../assets/icons/filetypes/pptx.png"))
            }
            _ => None,
        };
        match bytes {
            Some(b) => Ok(Some(Cow::Borrowed(b))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(SidebarIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Set the Dock icon for both bundled and direct development launches.
pub fn set_app_icon() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn del_set_app_icon(bytes: *const u8, length: usize);
        }
        let icon = include_bytes!("../assets/Del.icns");
        // Called by the application startup callback on the main thread.
        unsafe { del_set_app_icon(icon.as_ptr(), icon.len()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_action_icons_are_available() {
        for icon in [
            gpui_kit::assets::IconName::Pencil,
            gpui_kit::assets::IconName::Archive,
            gpui_kit::assets::IconName::Pin,
            gpui_kit::assets::IconName::PinOff,
            gpui_kit::assets::IconName::Close,
            gpui_kit::assets::IconName::Coffee,
            gpui_kit::assets::IconName::Eye,
            gpui_kit::assets::IconName::Trash,
            gpui_kit::assets::IconName::RefreshCw,
        ] {
            let path = icon.path();
            let bytes = AppAssets.load(&path).unwrap().expect("sidebar action SVG");
            assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
        }
        for path in [
            "icons/pin-outline.svg",
            "icons/pin-filled.svg",
            "icons/archive.svg",
            "icons/restore.svg",
        ] {
            let bytes = AppAssets.load(path).unwrap().expect("custom pin SVG");
            assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
        }
    }
}
