//! App icons embedded in the binary, layered over the component library's own
//! assets.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
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
        gpui_kit::assets::Assets.list(path)
    }
}
