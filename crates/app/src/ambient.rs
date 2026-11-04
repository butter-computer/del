//! Display-synchronized native Metal backgrounds, composited as one GPUI alpha surface.
//! Every simulation and all shading stay on the GPU, including cloth, soft bodies and
//! volumes. Rain uses analytic trajectories shared with impact feedback, without readback.
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2_core_foundation::CFRetained;
    use objc2_core_video::CVPixelBuffer;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{ffi::c_void, ptr::NonNull};
    unsafe extern "C" {
        fn del_fluid_create(
            bytes: *const u8,
            length: usize,
            source: *const u8,
            source_length: usize,
        ) -> *mut c_void;
        fn del_fluid_release(renderer: *mut c_void);
        fn del_fluid_running(renderer: *mut c_void, running: bool);
        fn del_fluid_window_view(renderer: *mut c_void, view: *mut c_void);
        fn del_fluid_blurred(renderer: *mut c_void, blurred: bool);
        fn del_fluid_frame(
            renderer: *mut c_void,
            width: f64,
            height: f64,
            scale: f64,
            zoom: f64,
            radius: f64,
            x: f64,
            y: f64,
            origin_x: f64,
            origin_y: f64,
            light: bool,
            style: u32,
            red: f32,
            green: f32,
            blue: f32,
            strength: f32,
            mouse_reactive: bool,
        ) -> *mut c_void;
        fn del_fluid_region(
            renderer: *mut c_void,
            slot: u32,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            radius: f64,
        );
        fn del_fluid_prepare(buffer: *const c_void, command_buffer: *mut c_void);
    }
    pub struct Fluid(*mut c_void);
    impl Fluid {
        pub fn new() -> Self {
            gpui_apple::metal_renderer::set_surface_prepare_callback(Some(del_fluid_prepare));
            let library = include_bytes!(concat!(env!("OUT_DIR"), "/del-fluid.metallib"));
            // Empty when the build host lacked `xcrun metal`; the source compiles at runtime.
            let source = include_str!("fluid.metal");
            Self(unsafe {
                del_fluid_create(
                    library.as_ptr(),
                    library.len(),
                    source.as_ptr(),
                    source.len(),
                )
            })
        }
        pub fn running(&self, running: bool) {
            unsafe { del_fluid_running(self.0, running) };
        }
        pub fn blurred(&self, blurred: bool) {
            unsafe { del_fluid_blurred(self.0, blurred) };
        }
        /// Forget a measured bar that the current page does not show, so rain and
        /// pudding don't land on UI that isn't there.
        pub fn clear_region(&self, slot: u32) {
            unsafe { del_fluid_region(self.0, slot, 0., 0., 0., 0., 0.) };
        }
        pub fn region(&self, slot: u32, bounds: Bounds<Pixels>, radius: Pixels) {
            unsafe {
                del_fluid_region(
                    self.0,
                    slot,
                    bounds.origin.x.as_f32() as f64,
                    bounds.origin.y.as_f32() as f64,
                    bounds.size.width.as_f32() as f64,
                    bounds.size.height.as_f32() as f64,
                    radius.as_f32() as f64,
                )
            };
        }
        pub fn frame(
            &self,
            bounds: Bounds<Pixels>,
            window: &Window,
            interactive: bool,
        ) -> Option<CFRetained<CVPixelBuffer>> {
            let mouse = window.mouse_position() - bounds.origin;
            let tint = crate::appearance::background_tint();
            let view = HasWindowHandle::window_handle(window)
                .ok()
                .and_then(|handle| {
                    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
                        Some(handle.ns_view.as_ptr())
                    } else {
                        None
                    }
                });
            let pointer = unsafe {
                del_fluid_window_view(self.0, view.unwrap_or(std::ptr::null_mut()));
                del_fluid_frame(
                    self.0,
                    bounds.size.width.as_f32() as f64,
                    bounds.size.height.as_f32() as f64,
                    window.scale_factor() as f64,
                    del_core::zoom::factor() as f64,
                    crate::zoom::px(del_core::ShellMetrics::default().page_radius).as_f32() as f64,
                    mouse.x.as_f32() as f64,
                    mouse.y.as_f32() as f64,
                    bounds.origin.x.as_f32() as f64,
                    bounds.origin.y.as_f32() as f64,
                    crate::appearance::light(),
                    crate::appearance::background() as u32,
                    tint.r,
                    tint.g,
                    tint.b,
                    tint.a,
                    interactive,
                )
            };
            NonNull::new(pointer.cast::<CVPixelBuffer>())
                .map(|pointer| unsafe { CFRetained::from_raw(pointer) })
        }
    }
    impl Drop for Fluid {
        fn drop(&mut self) {
            unsafe { del_fluid_release(self.0) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmbientSurface {
    Hidden,
    Landing,
    Chat,
    Terminal,
}

/// How much the terminal's own dark colour covers the blurred background, or `None`
/// when no background is showing and the terminal should stay fully opaque.
pub fn terminal_glass(cx: &App) -> Option<f32> {
    let showing = crate::appearance::background() != crate::appearance::BackgroundAnimation::Off
        && !cx.reduce_motion();
    // Terminals stay dark in both themes, so a light page needs far more cover.
    showing.then_some(if crate::appearance::light() {
        0.92
    } else {
        0.56
    })
}

/// The cursor only plays with the background on the new-session page. Behind a chat or
/// terminal the pointer is busy with real work, so the background ignores it.
fn surface_follows_cursor(surface: AmbientSurface, obstructed: bool) -> bool {
    surface == AmbientSurface::Landing && !obstructed
}

pub struct LifeBackground {
    visible: bool,
    surface: AmbientSurface,
    #[cfg(target_os = "macos")]
    fluid: std::rc::Rc<native::Fluid>,
}
impl LifeBackground {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_activation(window, |this, window, cx| {
            this.sync(window, cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_visibility(window, |this, _, window, cx| {
            this.sync(window, cx);
            cx.notify();
        })
        .detach();
        Self {
            visible: false,
            surface: AmbientSurface::Hidden,
            #[cfg(target_os = "macos")]
            fluid: std::rc::Rc::new(native::Fluid::new()),
        }
    }
    /// A layout-only probe: no drawing, hitbox, input handler or layout space.
    /// All prepaint callbacks finish before background painting submits a frame.
    pub fn region_probe(&self, slot: u32, radius: Pixels) -> AnyElement {
        #[cfg(target_os = "macos")]
        {
            let fluid = self.fluid.clone();
            canvas(
                move |bounds, _, _| fluid.region(slot, bounds, radius),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .size_full()
            .into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        div().absolute().inset_0().into_any_element()
    }
    fn animates(&self, window: &Window, cx: &App) -> bool {
        self.visible
            && crate::appearance::background() != crate::appearance::BackgroundAnimation::Off
            && window.is_visible()
            && window.is_window_active()
            && !cx.reduce_motion()
    }
    fn sync(&self, window: &Window, cx: &App) {
        #[cfg(target_os = "macos")]
        self.fluid.running(self.animates(window, cx));
    }
    /// Which page the background sits behind. The landing page shows it sharp; once a
    /// conversation has started it melts into a soft blur, and terminals do the same.
    pub fn set_surface(
        &mut self,
        surface: AmbientSurface,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.surface != surface;
        self.visible = surface != AmbientSurface::Hidden;
        self.surface = surface;
        #[cfg(target_os = "macos")]
        {
            self.fluid.blurred(matches!(
                surface,
                AmbientSurface::Chat | AmbientSurface::Terminal
            ));
            if surface != AmbientSurface::Landing {
                // Only the landing page has the options footer's two groups.
                self.fluid.clear_region(1);
                self.fluid.clear_region(2);
            }
            if surface == AmbientSurface::Terminal {
                self.fluid.clear_region(0); // Terminals have no input bar to land on.
            }
        }
        self.sync(window, cx);
        // Inactive windows do not schedule animation frames. Still invalidate
        // their retained surface once when chat/landing changes its blur state.
        if changed {
            cx.notify();
        }
    }
}
impl Render for LifeBackground {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        // Off and reduced motion omit the surface entirely, including pointer effects.
        if crate::appearance::background() == crate::appearance::BackgroundAnimation::Off
            || cx.reduce_motion()
        {
            return div().absolute().inset_0().into_any_element();
        }
        #[cfg(target_os = "macos")]
        {
            // GPUI schedules this view at display refresh (including ProMotion),
            // rather than a timer or shell-wide notification. Canvas only submits
            // one GPU surface; it performs no CPU simulation or per-cell painting.
            if self.animates(window, cx) {
                window.request_animation_frame();
            }
            let fluid = self.fluid.clone();
            // Native effects sample mouse buttons directly, so GPUI's modal hit testing
            // alone cannot protect controls from also grabbing the background behind them.
            let obstructed = window.has_active_dialog(cx)
                || window.has_active_sheet(cx)
                || window.has_active_prompt();
            let interactive = crate::appearance::mouse_reactive()
                && self.animates(window, cx)
                && surface_follows_cursor(self.surface, obstructed);
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if let Some(buffer) = fluid.frame(bounds, window, interactive) {
                        window.paint_surface(bounds, buffer);
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full()
            .into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        div().absolute().inset_0().into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{AmbientSurface, surface_follows_cursor};

    #[test]
    fn only_the_new_session_page_follows_the_cursor() {
        assert!(surface_follows_cursor(AmbientSurface::Landing, false));
        assert!(!surface_follows_cursor(AmbientSurface::Landing, true));
        for surface in [
            AmbientSurface::Hidden,
            AmbientSurface::Chat,
            AmbientSurface::Terminal,
        ] {
            assert!(!surface_follows_cursor(surface, false));
            assert!(!surface_follows_cursor(surface, true));
        }
    }
}
