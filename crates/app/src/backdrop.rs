//! Native in-app modal backdrop: GPU scene blur at the surface draw boundary.
use gpui_kit::prelude::*;
use gpui_kit::*;
#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2_core_foundation::CFRetained;
    use objc2_core_video::CVPixelBuffer;
    use std::{ffi::c_void, ptr::NonNull};
    unsafe extern "C" {
        fn del_backdrop_create(
            bytes: *const u8,
            length: usize,
            source: *const u8,
            source_length: usize,
        ) -> *mut c_void;
        fn del_backdrop_release(context: *mut c_void);
        fn del_backdrop_marker(context: *mut c_void, scale: f64, zoom: f64) -> *mut c_void;
        fn del_backdrop_encode(
            buffer: *const c_void,
            command: *mut c_void,
            texture: *mut c_void,
        ) -> bool;
    }
    struct Backdrop(*mut c_void);
    impl Backdrop {
        fn new() -> Self {
            gpui_apple::metal_renderer::set_surface_backdrop_callback(Some(del_backdrop_encode));
            let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/del-fluid.metallib"));
            // Empty when the build host lacked `xcrun metal`; the source compiles at runtime.
            let source = include_str!("fluid.metal");
            Self(unsafe {
                del_backdrop_create(bytes.as_ptr(), bytes.len(), source.as_ptr(), source.len())
            })
        }
        fn marker(&self, window: &Window) -> Option<CFRetained<CVPixelBuffer>> {
            let pointer = unsafe {
                del_backdrop_marker(
                    self.0,
                    window.scale_factor() as f64,
                    del_core::zoom::factor() as f64,
                )
            };
            NonNull::new(pointer.cast::<CVPixelBuffer>())
                .map(|pointer| unsafe { CFRetained::from_raw(pointer) })
        }
    }
    impl Drop for Backdrop {
        fn drop(&mut self) {
            unsafe { del_backdrop_release(self.0) }
        }
    }
    thread_local! {static BACKDROP:Backdrop=Backdrop::new();}
    pub fn layer() -> AnyElement {
        canvas(
            |_, _, _| (),
            |bounds, _, window, _| {
                BACKDROP.with(|backdrop| {
                    if let Some(marker) = backdrop.marker(window) {
                        window.paint_surface(bounds, marker)
                    }
                });
            },
        )
        .absolute()
        .inset_0()
        .size_full()
        .into_any_element()
    }
}
pub fn blur_layer() -> AnyElement {
    #[cfg(target_os = "macos")]
    {
        native::layer()
    }
    #[cfg(not(target_os = "macos"))]
    {
        div().absolute().inset_0().into_any_element()
    }
}
