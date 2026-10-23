//! UI-thread-owned native WebKit view. Cached panels explicitly hide their views.
#[cfg(target_os = "macos")]
mod native {
    use gpui_kit::prelude::*;
    use gpui_kit::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::Cell,
        ffi::{CStr, CString, c_char, c_void},
        rc::Rc,
    };

    unsafe extern "C" {
        fn del_browser_create(
            parent: *mut c_void,
            url: *const c_char,
            profile: *const c_char,
        ) -> *mut c_void;
        fn del_browser_frame(
            view: *mut c_void,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            zoom: f64,
            light: bool,
            mouse_passthrough: bool,
        );
        fn del_browser_url(view: *mut c_void) -> *const c_char;
        fn del_browser_hide(view: *mut c_void);
        fn del_browser_release(view: *mut c_void);
        fn del_browser_command(
            view: *mut c_void,
            command: *const c_char,
            reply: extern "C" fn(*mut c_void, *const c_char),
            context: *mut c_void,
        );
        fn del_browser_delete_profile(
            profile: *const c_char,
            reply: extern "C" fn(*mut c_void, *const c_char),
            context: *mut c_void,
        );
    }

    #[derive(Clone)]
    pub struct Browser(Rc<NativeView>);
    struct NativeView {
        initial_url: CString,
        profile: CString,
        pointer: Cell<*mut c_void>,
    }
    impl Drop for NativeView {
        fn drop(&mut self) {
            unsafe { del_browser_release(self.pointer.get()) };
        }
    }
    impl Browser {
        pub fn new(url: &str, profile: &str) -> Self {
            Self(Rc::new(NativeView {
                initial_url: CString::new(url)
                    .unwrap_or_else(|_| CString::new("about:blank").unwrap()),
                profile: CString::new(profile).expect("UUID profile"),
                pointer: Cell::new(std::ptr::null_mut()),
            }))
        }
        pub fn hide(&self) {
            unsafe { del_browser_hide(self.0.pointer.get()) };
        }
        pub fn url(&self) -> String {
            let url = unsafe { del_browser_url(self.0.pointer.get()) };
            if url.is_null() {
                self.0.initial_url.to_string_lossy().into_owned()
            } else {
                unsafe { CStr::from_ptr(url) }
                    .to_string_lossy()
                    .into_owned()
            }
        }
        pub fn command(&self, request: serde_json::Value, window: &Window) -> super::BrowserReply {
            ensure_created(&self.0, window);
            let (tx, rx) = tokio::sync::oneshot::channel();
            let context = Box::into_raw(Box::new(tx)) as *mut c_void;
            let json = CString::new(request.to_string()).expect("JSON has no literal NUL");
            unsafe { del_browser_command(self.0.pointer.get(), json.as_ptr(), receive, context) };
            rx
        }
        pub fn element(&self) -> impl IntoElement {
            let view = self.0.clone();
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    ensure_created(&view, window);
                    let bounds = bounds.intersect(&window.content_mask().bounds);
                    unsafe {
                        del_browser_frame(
                            view.pointer.get(),
                            bounds.origin.x.as_f32() as f64,
                            bounds.origin.y.as_f32() as f64,
                            bounds.size.width.as_f32() as f64,
                            bounds.size.height.as_f32() as f64,
                            del_core::zoom::factor() as f64,
                            crate::appearance::light(),
                            cx.has_active_drag(),
                        )
                    };
                },
            )
            .size_full()
        }
    }
    fn ensure_created(view: &NativeView, window: &Window) {
        if !view.pointer.get().is_null() {
            return;
        }
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        view.pointer.set(unsafe {
            del_browser_create(
                handle.ns_view.as_ptr(),
                view.initial_url.as_ptr(),
                view.profile.as_ptr(),
            )
        });
    }
    extern "C" fn receive(context: *mut c_void, json: *const c_char) {
        let sender = unsafe {
            Box::from_raw(
                context as *mut tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
            )
        };
        let result = if json.is_null() {
            Err("Empty browser response".into())
        } else {
            serde_json::from_slice::<serde_json::Value>(unsafe { CStr::from_ptr(json) }.to_bytes())
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                        Err(error.to_owned())
                    } else {
                        Ok(value["result"].clone())
                    }
                })
        };
        let _ = sender.send(result);
    }
    pub fn delete_profile(profile: &str) -> super::BrowserReply {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let id = CString::new(profile).expect("UUID profile");
        unsafe {
            del_browser_delete_profile(
                id.as_ptr(),
                receive,
                Box::into_raw(Box::new(tx)) as *mut c_void,
            )
        };
        rx
    }
}
#[cfg(target_os = "macos")]
pub use native::{Browser, delete_profile};

#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
pub struct Browser(String);
#[cfg(not(target_os = "macos"))]
impl Browser {
    pub fn new(url: &str, _: &str) -> Self {
        Self(url.into())
    }
    pub fn hide(&self) {}
    pub fn command(&self, _: serde_json::Value, _: &gpui_kit::Window) -> BrowserReply {
        delete_profile("")
    }
    pub fn url(&self) -> String {
        self.0.clone()
    }
    pub fn element(&self) -> impl gpui_kit::IntoElement {
        use gpui_kit::ParentElement;
        gpui_kit::div().child("The embedded browser requires macOS.")
    }
}

#[cfg(not(target_os = "macos"))]
pub fn delete_profile(_: &str) -> BrowserReply {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(Err(
        "The embedded browser requires macOS 14 or later.".into()
    ));
    rx
}

pub type BrowserReply = tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedTab {
    pub id: String,
    pub url: String,
}
impl Default for SavedTab {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            url: "about:blank".into(),
        }
    }
}

pub fn script_for(request: &del_mcp::BrowserRequest) -> Result<String, String> {
    let quoted = |text: &str| serde_json::to_string(text).unwrap();
    let selector = quoted(request.selector.as_deref().unwrap_or(""));
    let find = format!(
        "const e=document.querySelector({selector}); if(!e) throw new Error('Element not found');"
    );
    Ok(match request.action.as_str() {
        "snapshot" => r#"return {url:location.href,title:document.title,ready:document.readyState,text:(document.body?.innerText||'').slice(0,30000),elements:Array.from(document.querySelectorAll('a,button,input,textarea,select,[role=button]')).slice(0,150).map((e)=>{let parts=[],n=e;while(n&&n.nodeType===1){if(n.id){parts.unshift('#'+CSS.escape(n.id));break;}let p=n.parentElement;parts.unshift(n.tagName.toLowerCase()+(p?':nth-child('+(Array.from(p.children).indexOf(n)+1)+')':''));n=p;}return {selector:parts.join(' > '),tag:e.tagName,text:(e.innerText||e.getAttribute('aria-label')||e.getAttribute('placeholder')||'').slice(0,160),type:e.getAttribute('type'),href:e.getAttribute('href')};})};"#.into(),
        "click" => format!("{find} e.scrollIntoView({{block:'center'}}); e.click(); return {{clicked:true}};"),
        "fill" => {
            let value = quoted(request.value.as_deref().unwrap_or(""));
            format!("{find} e.focus(); if(e.isContentEditable) e.textContent={value}; else {{const setter=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value')?.set;if(setter)setter.call(e,{value});else e.value={value};}} e.dispatchEvent(new Event('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));return {{filled:true}};")
        },
        "press_key" => {
            let key = request.key.as_deref().unwrap_or("");
            if !["Enter","Tab","Escape","ArrowUp","ArrowDown","ArrowLeft","ArrowRight"].contains(&key) { return Err("Unsupported key".into()); }
            let key = quoted(key);
            format!("const e=document.activeElement; e.dispatchEvent(new KeyboardEvent('keydown',{{key:{key},bubbles:true}})); if({key}==='Enter'&&e.form)e.form.requestSubmit(); e.dispatchEvent(new KeyboardEvent('keyup',{{key:{key},bubbles:true}})); return {{pressed:{key}}};")
        },
        "evaluate" => request.script.clone().ok_or("Missing script")?,
        _ => return Err("Not a page script action".into()),
    })
}

fn deletion_path() -> std::path::PathBuf {
    del_agent::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("browser-profile-deletions.json")
}
pub fn pending_deletions() -> Vec<String> {
    std::fs::read(deletion_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .collect()
}
pub fn record_deletion(profile: &str, pending: bool) -> std::io::Result<()> {
    let mut profiles = pending_deletions();
    profiles.retain(|id| id != profile);
    if pending {
        profiles.push(profile.to_owned());
    }
    let path = deletion_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&profiles)?)?;
    std::fs::rename(temporary, path)
}
