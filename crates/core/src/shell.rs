//! Geometry and colour of the application shell, independent of any UI
//! toolkit.
//!
//! The window is a dark top bar over a single "page" that is inset from the
//! window edges: the page's top edge sits directly under the top bar, and the
//! left, right and bottom edges share one gutter. All lengths are in logical
//! pixels.

/// The one colour of the shell surface: the top bar and the gutter around the
/// page are painted with exactly this value so they read as a single
/// continuous surface. Packed `0xRRGGBBAA`; the window is blurred behind it,
/// so the alpha lets the blur show through.
pub const SHELL_SURFACE_RGBA: u32 = 0x111113a6;

/// Colour of the rounded page: a deeper black than the shell around it, with
/// the same translucency. Packed `0xRRGGBBAA`.
pub const PAGE_SURFACE_RGBA: u32 = 0x000000a6;

/// Diameter of a macOS traffic light button, in logical pixels.
pub const TRAFFIC_LIGHT_SIZE: f32 = 12.0;

/// Layout metrics for the shell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellMetrics {
    /// Height of the top bar. On macOS this must leave room for the traffic
    /// light buttons, which the UI layer positions inside it.
    pub topbar_height: f32,
    /// Gutter between the page and the window's left, right and bottom edges.
    pub page_inset: f32,
    /// Corner radius of the page.
    pub page_radius: f32,
}

impl Default for ShellMetrics {
    fn default() -> Self {
        Self {
            topbar_height: 38.0,
            page_inset: 8.0,
            page_radius: 12.0,
        }
    }
}

impl ShellMetrics {
    /// Y offset at which the page begins: directly beneath the top bar.
    pub fn page_top(&self) -> f32 {
        self.topbar_height
    }

    /// Top-left position of the macOS traffic lights so they sit vertically
    /// centred in the top bar, inset from the left edge.
    pub fn traffic_light_origin(&self) -> (f32, f32) {
        (12.0, (self.topbar_height - TRAFFIC_LIGHT_SIZE) / 2.0)
    }

    /// Size of the page for a window of the given logical size.
    pub fn page_size(&self, window_width: f32, window_height: f32) -> (f32, f32) {
        let width = (window_width - 2.0 * self.page_inset).max(0.0);
        let height = (window_height - self.topbar_height - self.page_inset).max(0.0);
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sits_under_topbar_with_equal_side_and_bottom_gutters() {
        let m = ShellMetrics {
            topbar_height: 40.0,
            page_inset: 10.0,
            page_radius: 8.0,
        };
        assert_eq!(m.page_top(), 40.0);
        assert_eq!(m.page_size(800.0, 600.0), (780.0, 550.0));
    }

    #[test]
    fn traffic_lights_are_centred_in_topbar() {
        let m = ShellMetrics {
            topbar_height: 40.0,
            ..ShellMetrics::default()
        };
        assert_eq!(m.traffic_light_origin(), (12.0, 14.0));
    }

    #[test]
    fn page_size_never_goes_negative() {
        let m = ShellMetrics::default();
        assert_eq!(m.page_size(1.0, 1.0), (0.0, 0.0));
    }
}
