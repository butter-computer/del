//! Display-synchronized native Metal Conway Life, composited as one GPUI alpha surface.
//! CPU work is O(1) pointer/uniform updates; simulation and pixel shading are GPU-only.
use gpui_kit::prelude::*;
use gpui_kit::*;

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2_core_foundation::CFRetained;
    use objc2_core_video::CVPixelBuffer;
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
        ) -> Option<CFRetained<CVPixelBuffer>> {
            let mouse = window.mouse_position() - bounds.origin;
            let pointer = unsafe {
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

pub struct LifeBackground {
    visible: bool,
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
        self.visible && window.is_visible() && window.is_window_active() && !cx.reduce_motion()
    }
    fn sync(&self, window: &Window, cx: &App) {
        #[cfg(target_os = "macos")]
        self.fluid.running(self.animates(window, cx));
    }
    pub fn set_visible(&mut self, visible: bool, window: &Window, cx: &mut Context<Self>) {
        self.visible = visible;
        self.sync(window, cx);
    }
}
impl Render for LifeBackground {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        #[cfg(target_os = "macos")]
        {
            // GPUI schedules this view at display refresh (including ProMotion),
            // rather than a timer or shell-wide notification. Canvas only submits
            // one GPU surface; it performs no CPU simulation or per-cell painting.
            if self.animates(window, cx) {
                window.request_animation_frame();
            }
            let fluid = self.fluid.clone();
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if let Some(buffer) = fluid.frame(bounds, window) {
                        window.paint_surface(bounds, buffer);
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full()
        }
        #[cfg(not(target_os = "macos"))]
        div().absolute().inset_0()
    }
}
