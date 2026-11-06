//! The main window's frame, remembered across launches.
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Smallest window Del supports.
pub const MIN_SIZE: (f32, f32) = (960., 640.);
/// Never restore a window with less than this much of it on a display.
const MIN_VISIBLE: f32 = 120.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// The restore frame (in the display's coordinates, as GPUI reports it) and
/// which display it was on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub display: Option<String>,
}

impl Saved {
    pub fn from_window(bounds: WindowBounds, display: Option<String>) -> Self {
        let (kind, frame) = match bounds {
            WindowBounds::Windowed(frame) => (Kind::Windowed, frame),
            WindowBounds::Maximized(frame) => (Kind::Maximized, frame),
            WindowBounds::Fullscreen(frame) => (Kind::Fullscreen, frame),
        };
        Self {
            x: frame.origin.x.as_f32(),
            y: frame.origin.y.as_f32(),
            width: frame.size.width.as_f32(),
            height: frame.size.height.as_f32(),
            kind,
            display,
        }
    }

    fn frame(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }

    /// The frame to open with on a display whose usable area is `visible`:
    /// no larger than the display, fully on it, and never below the minimum
    /// size. `None` when the saved frame is unusable or mostly off this display,
    /// so the caller falls back to its centred default.
    pub fn restore_on(&self, visible: Bounds<Pixels>) -> Option<WindowBounds> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|v| !v.is_finite()) || self.width <= 0. || self.height <= 0. {
            return None;
        }
        let frame = self.frame();
        let overlap = frame.intersect(&visible);
        if overlap.size.width.as_f32() < MIN_VISIBLE.min(self.width)
            || overlap.size.height.as_f32() < MIN_VISIBLE.min(self.height)
        {
            return None;
        }
        let max_w = visible.size.width.as_f32();
        let max_h = visible.size.height.as_f32();
        let width = self.width.max(MIN_SIZE.0).min(max_w);
        let height = self.height.max(MIN_SIZE.1).min(max_h);
        let left = visible.origin.x.as_f32();
        let top = visible.origin.y.as_f32();
        let x = self.x.clamp(left, left + max_w - width);
        let y = self.y.clamp(top, top + max_h - height);
        let frame = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
        Some(match self.kind {
            Kind::Windowed => WindowBounds::Windowed(frame),
            Kind::Maximized => WindowBounds::Maximized(frame),
            Kind::Fullscreen => WindowBounds::Fullscreen(frame),
        })
    }
}

pub fn path() -> Option<PathBuf> {
    del_agent::data_dir().map(|dir| dir.join("window.json"))
}

pub fn read(path: &Path) -> Option<Saved> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn write(path: &Path, saved: &Saved) -> std::io::Result<()> {
    crate::sessions::write_atomic(path, &serde_json::to_vec(saved)?, 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    fn saved(x: f32, y: f32, width: f32, height: f32) -> Saved {
        Saved {
            x,
            y,
            width,
            height,
            kind: Kind::Windowed,
            display: None,
        }
    }

    fn display() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(25.)), size(px(1512.), px(920.)))
    }

    #[test]
    fn keeps_a_frame_that_fits() {
        let restored = saved(100., 80., 1200., 800.).restore_on(display()).unwrap();
        assert_eq!(
            restored,
            WindowBounds::Windowed(Bounds::new(
                point(px(100.), px(80.)),
                size(px(1200.), px(800.))
            ))
        );
    }

    #[test]
    fn clamps_size_and_position_onto_the_display() {
        let WindowBounds::Windowed(frame) = saved(900., 400., 3000., 2000.)
            .restore_on(display())
            .unwrap()
        else {
            panic!("windowed");
        };
        assert_eq!(frame.size, size(px(1512.), px(920.)));
        assert_eq!(frame.origin, point(px(0.), px(25.)));

        let WindowBounds::Windowed(frame) = saved(1000., 600., 1000., 700.)
            .restore_on(display())
            .unwrap()
        else {
            panic!("windowed");
        };
        assert_eq!(frame.origin, point(px(512.), px(245.)));
        // Too small frames grow to the minimum size.
        let WindowBounds::Windowed(frame) =
            saved(10., 30., 300., 200.).restore_on(display()).unwrap()
        else {
            panic!("windowed");
        };
        assert_eq!(frame.size, size(px(960.), px(640.)));
    }

    #[test]
    fn falls_back_when_off_screen_or_invalid() {
        assert!(
            saved(4000., 80., 1200., 800.)
                .restore_on(display())
                .is_none()
        );
        assert!(
            saved(1450., 80., 1200., 800.)
                .restore_on(display())
                .is_none()
        );
        assert!(
            saved(f32::NAN, 80., 1200., 800.)
                .restore_on(display())
                .is_none()
        );
        assert!(saved(0., 80., 0., 800.).restore_on(display()).is_none());
    }

    #[test]
    fn keeps_maximized_and_fullscreen_and_round_trips() {
        let mut maximized = saved(100., 80., 1200., 800.);
        maximized.kind = Kind::Maximized;
        assert!(matches!(
            maximized.restore_on(display()),
            Some(WindowBounds::Maximized(_))
        ));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window.json");
        maximized.display = Some("display".into());
        write(&path, &maximized).unwrap();
        assert_eq!(read(&path), Some(maximized));
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(read(&path), None);
    }
}
