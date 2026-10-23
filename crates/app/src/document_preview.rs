//! Native, interactive document preview. The panel explicitly hides cached or
//! inactive native views; dropping the owner closes and releases Quick Look.
use std::path::Path;

pub fn supports(path: &Path) -> bool {
    cfg!(target_os = "macos")
        && path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "pdf"
                        | "doc"
                        | "docx"
                        | "rtf"
                        | "rtfd"
                        | "odt"
                        | "pages"
                        | "xls"
                        | "xlsx"
                        | "numbers"
                        | "ppt"
                        | "pptx"
                        | "key"
                        | "png"
                        | "jpg"
                        | "jpeg"
                        | "webp"
                        | "gif"
                        | "heic"
                        | "tif"
                        | "tiff"
                )
            })
}

#[cfg(target_os = "macos")]
mod native {
    use gpui_kit::prelude::*;
    use gpui_kit::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::Cell,
        ffi::{CString, c_void},
        os::unix::ffi::OsStrExt,
        path::Path,
        rc::Rc,
    };

    unsafe extern "C" {
        fn del_preview_create(parent: *mut c_void, path: *const std::ffi::c_char) -> *mut c_void;
        fn del_preview_frame(view: *mut c_void, x: f64, y: f64, width: f64, height: f64);
        fn del_preview_hide(view: *mut c_void);
        fn del_preview_release(view: *mut c_void);
    }

    #[derive(Clone)]
    pub struct DocumentPreview(Rc<NativeView>);
    struct NativeView {
        path: CString,
        pointer: Cell<*mut c_void>,
    }
    impl Drop for NativeView {
        fn drop(&mut self) {
            // Rc confines ownership and destruction to the UI thread.
            unsafe { del_preview_release(self.pointer.get()) };
        }
    }
    impl DocumentPreview {
        pub fn new(path: &Path) -> Self {
            Self(Rc::new(NativeView {
                // File system paths cannot contain NUL.
                path: CString::new(path.as_os_str().as_bytes()).expect("valid file path"),
                pointer: Cell::new(std::ptr::null_mut()),
            }))
        }
        pub fn hide(&self) {
            unsafe { del_preview_hide(self.0.pointer.get()) };
        }
        pub fn element(&self) -> impl IntoElement {
            let view = self.0.clone();
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if view.pointer.get().is_null() {
                        let Ok(handle) = HasWindowHandle::window_handle(window) else {
                            return;
                        };
                        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                            return;
                        };
                        // GPUI supplies its live NSView; the bridge retains only our child.
                        let pointer = unsafe {
                            del_preview_create(handle.ns_view.as_ptr(), view.path.as_ptr())
                        };
                        view.pointer.set(pointer);
                    }
                    let bounds = bounds.intersect(&window.content_mask().bounds);
                    unsafe {
                        del_preview_frame(
                            view.pointer.get(),
                            bounds.origin.x.as_f32() as f64,
                            bounds.origin.y.as_f32() as f64,
                            bounds.size.width.as_f32() as f64,
                            bounds.size.height.as_f32() as f64,
                        );
                    }
                },
            )
            .size_full()
        }
    }
}
#[cfg(target_os = "macos")]
pub use native::DocumentPreview;

#[cfg(not(target_os = "macos"))]
pub struct DocumentPreview;
#[cfg(not(target_os = "macos"))]
impl DocumentPreview {
    pub fn new(_: &Path) -> Self {
        Self
    }
    pub fn hide(&self) {}
    pub fn element(&self) -> impl gpui_kit::IntoElement {
        use gpui_kit::ParentElement;
        gpui_kit::div().child("Document previews require macOS.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documents_use_native_preview_without_treating_source_code_as_documents() {
        for name in [
            "report.PDF",
            "proposal.docx",
            "legacy.doc",
            "slides.pptx",
            "book.xlsx",
        ] {
            assert_eq!(supports(Path::new(name)), cfg!(target_os = "macos"));
        }
        for name in ["main.rs", "readme.md", "data.json", "binary", "archive.zip"] {
            assert!(!supports(Path::new(name)));
        }
    }
}
