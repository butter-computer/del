//! App icons embedded in the binary, layered over the component library's own
//! assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(SidebarIcons, [Pencil, Archive]);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = SidebarIcons.load(path)? {
            return Ok(Some(bytes));
        }
        let bytes: Option<&'static [u8]> = match path {
            "icons/claude.svg" => Some(include_bytes!("../assets/icons/claude.svg")),
            "icons/codex.svg" => Some(include_bytes!("../assets/icons/codex.svg")),
            "icons/acp.svg" => Some(include_bytes!("../assets/icons/acp.svg")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_action_icons_are_available() {
        for icon in [
            gpui_kit::assets::IconName::Pencil,
            gpui_kit::assets::IconName::Archive,
        ] {
            let path = icon.path();
            let bytes = AppAssets.load(&path).unwrap().expect("sidebar action SVG");
            assert!(std::str::from_utf8(&bytes).unwrap().contains("<svg"));
        }
    }
}
