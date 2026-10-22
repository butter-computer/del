//! Load the page background into GPU-ready pixels before the window opens.
//!
//! `del_core::wallpaper` decides *which* file to show; this module turns it
//! into a [`RenderImage`] that `img()` can paint on its very first frame, so
//! the page never pops in. The bundled fallback is used whenever the user's
//! desktop is not an image they chose themselves.

use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use del_core::wallpaper::current_wallpaper;
use gpui_kit::RenderImage;
use image::imageops::FilterType;
use image::{DynamicImage, Frame, ImageReader};

/// Shipped default, shown when the desktop wallpaper is dynamic, an aerial,
/// a solid colour, or cannot be determined. Encoded as lossy WebP at its
/// native 1400x834 so the binary only grows by a quarter megabyte.
const FALLBACK_WEBP: &[u8] = include_bytes!("../assets/wallpaper-fallback.webp");

/// Longest edge, in device pixels, kept after decoding. Enough for a retina
/// window while bounding the GPU texture that a 5K user wallpaper would
/// otherwise occupy.
const MAX_EDGE_PX: u32 = 2560;

/// Start resolving and decoding the wallpaper on a worker thread.
///
/// Call this first thing in `main` so the work overlaps GPUI's own startup,
/// then [`Preload::finish`] right before opening the window.
pub fn preload() -> Preload {
    Preload(thread::spawn(|| {
        current_wallpaper()
            .and_then(|path| decode_file(&path))
            .or_else(|| decode_bytes(FALLBACK_WEBP))
            .map(Arc::new)
    }))
}

/// Handle to an in-flight wallpaper decode.
pub struct Preload(JoinHandle<Option<Arc<RenderImage>>>);

impl Preload {
    /// Block until the wallpaper is decoded. `None` only if even the bundled
    /// fallback failed to decode, which indicates a broken build.
    pub fn finish(self) -> Option<Arc<RenderImage>> {
        self.0.join().ok().flatten()
    }
}

fn decode_file(path: &Path) -> Option<RenderImage> {
    let image = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    Some(to_render_image(image))
}

fn decode_bytes(bytes: &[u8]) -> Option<RenderImage> {
    let image = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    Some(to_render_image(image))
}

/// Downscale to [`MAX_EDGE_PX`] if needed and convert to the BGRA layout GPUI
/// uploads directly.
fn to_render_image(mut image: DynamicImage) -> RenderImage {
    if image.width().max(image.height()) > MAX_EDGE_PX {
        image = image.resize(MAX_EDGE_PX, MAX_EDGE_PX, FilterType::Triangle);
    }
    let mut pixels = image.into_rgba8();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    RenderImage::new([Frame::new(pixels)])
}
