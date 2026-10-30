//! A dismissible view of agent browser activity. It never owns a browser or
//! changes its viewport: snapshots come from the existing tab, asynchronously.
use crate::{right_panel::RightPanel, state::Tab, zoom::px as zpx};
use base64::Engine as _;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use image::GenericImageView;
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct OpenBrowser {
    pub session: Tab,
    pub panel: WeakEntity<RightPanel>,
    pub tab_id: String,
}

#[derive(Default)]
pub(crate) struct BrowserPreview {
    selected: Option<Tab>,
    parked: HashMap<Tab, SessionPreview>,
    target: Option<OpenBrowser>,
    image: Option<PreviewImage>,
    dismissed: bool,
    suppressed: bool,
    topbar: f32,
    position: Option<Point<f32>>,
    grab: Option<Grab>,
    generation: u64,
    sample: Option<Task<()>>,
    observation: Option<Subscription>,
}

#[derive(Default)]
struct SessionPreview {
    target: Option<OpenBrowser>,
    image: Option<PreviewImage>,
    dismissed: bool,
    position: Option<Point<f32>>,
}

struct Grab {
    pointer: Point<Pixels>,
    origin: Point<Pixels>,
    moved: bool,
}

impl EventEmitter<OpenBrowser> for BrowserPreview {}

impl BrowserPreview {
    pub(crate) fn set_session(
        &mut self,
        selected: Option<Tab>,
        open: &[Tab],
        cx: &mut Context<Self>,
    ) {
        self.parked.retain(|session, _| open.contains(session));
        let selected = selected.filter(|session| open.contains(session));
        if self.selected == selected {
            return;
        }
        self.sample = None;
        self.observation = None;
        self.grab = None;
        self.generation += 1;
        if let Some(target) = self.target.take() {
            let session = target.session.clone();
            let saved = SessionPreview {
                target: Some(target),
                image: self.image.take(),
                dismissed: self.dismissed,
                position: self.position.take(),
            };
            if open.contains(&session) {
                self.parked.insert(session, saved);
            }
        }
        let restored = selected
            .as_ref()
            .and_then(|session| self.parked.remove(session))
            .unwrap_or_default();
        self.target = restored.target;
        self.image = restored.image;
        self.dismissed = restored.dismissed;
        self.position = restored.position;
        self.selected = selected;
        if let Some(panel) = self
            .target
            .as_ref()
            .and_then(|target| target.panel.upgrade())
        {
            self.observation = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
        }
        cx.notify();
    }

    pub(crate) fn target(&self) -> Option<OpenBrowser> {
        self.target.clone()
    }

    pub(crate) fn rebind(
        &mut self,
        session: Tab,
        panel: Entity<RightPanel>,
        tab_id: String,
        cx: &mut Context<Self>,
    ) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.panel != panel.downgrade())
        {
            self.sample = None;
            self.generation += 1;
            self.observation = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
            self.target = Some(OpenBrowser {
                session,
                panel: panel.downgrade(),
                tab_id,
            });
            cx.notify();
        }
    }

    pub(crate) fn record(
        &mut self,
        session: Tab,
        panel: Entity<RightPanel>,
        tab_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_ref() != Some(&session) {
            let saved = self.parked.entry(session.clone()).or_default();
            let same = saved
                .target
                .as_ref()
                .is_some_and(|target| target.tab_id == tab_id);
            if !same {
                saved.image = None;
                saved.dismissed = false;
            }
            saved.target = Some(OpenBrowser {
                session,
                panel: panel.downgrade(),
                tab_id,
            });
            return;
        }
        let same = self
            .target
            .as_ref()
            .is_some_and(|target| target.session == session && target.tab_id == tab_id);
        if same {
            self.rebind(session.clone(), panel.clone(), tab_id.clone(), cx);
        }
        if !same {
            self.generation += 1;
            self.sample = None;
            self.image = None;
            self.dismissed = false;
            self.grab = None;
            self.observation = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
            self.target = Some(OpenBrowser {
                session,
                panel: panel.downgrade(),
                tab_id,
            });
        }
        cx.notify();
    }

    pub(crate) fn set_suppressed(&mut self, suppressed: bool, topbar: f32, cx: &mut Context<Self>) {
        if self.suppressed != suppressed || self.topbar != topbar {
            self.suppressed = suppressed;
            self.topbar = topbar;
            cx.notify();
        }
    }

    fn source(&self, cx: &App) -> Option<crate::browser::Browser> {
        if self.dismissed || self.suppressed {
            return None;
        }
        let target = self.target.as_ref()?;
        let panel = target.panel.upgrade()?;
        let panel = panel.read(cx);
        if panel.browser_surface_visible(cx) {
            return None;
        }
        panel.browser_preview_source(&target.tab_id)
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.dismissed = true;
        self.sample = None;
        self.image = None;
        self.grab = None;
        self.generation += 1;
        cx.notify();
    }

    fn sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sample.is_some() {
            return;
        }
        let generation = self.generation;
        self.sample = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(Some(reply)) = this.update_in(cx, |this, window, cx| {
                    this.source(cx).map(|browser| {
                        browser.command(
                            serde_json::json!({"action":"screenshot", "preview":true}),
                            window,
                        )
                    })
                }) else {
                    break;
                };
                if let Ok(Ok(value)) = reply.await {
                    let image = cx
                        .background_executor()
                        .spawn(async move { decode_snapshot(value) })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.generation == generation && this.source(cx).is_some() {
                            if let Some(image) = image {
                                this.image = Some(image);
                            }
                            cx.notify();
                        }
                    });
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
            }
        }));
    }
}

#[derive(Clone)]
struct PreviewImage {
    page: Arc<RenderImage>,
    glass: Arc<RenderImage>,
    light: bool,
}

fn render_image(mut image: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in image.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(image)]))
}

fn decode_snapshot(value: serde_json::Value) -> Option<PreviewImage> {
    let encoded = value.get("image")?.as_str()?;
    if encoded.len() > 6 * 1024 * 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
    // Small real page pixels keep the localized glass blur inexpensive. Both
    // images use identical geometry so the close control samples its backdrop.
    let image = image.thumbnail(560, 560).into_rgba8();
    let glass = image::imageops::blur(&image, (image.width() as f32 / 260. * 5.).max(1.));
    let region = image::imageops::crop_imm(
        &image,
        image.width() * 3 / 4,
        0,
        (image.width() / 4).max(1),
        (image.height() / 4).max(1),
    );
    let luminance = region
        .pixels()
        .map(|(_, _, p)| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
        .sum::<f32>()
        / (region.width() * region.height()).max(1) as f32;
    Some(PreviewImage {
        page: render_image(image),
        glass: render_image(glass),
        light: luminance > 150.,
    })
}

fn fitted_image_bounds(bounds: Bounds<Pixels>, image: &RenderImage) -> Bounds<Pixels> {
    let dimensions = image.size(0);
    let scale = (bounds.size.width.as_f32() / dimensions.width.0 as f32)
        .min(bounds.size.height.as_f32() / dimensions.height.0 as f32);
    let size = size(
        px(dimensions.width.0 as f32 * scale),
        px(dimensions.height.0 as f32 * scale),
    );
    Bounds::new(
        bounds.origin
            + point(
                (bounds.size.width - size.width) / 2.,
                (bounds.size.height - size.height) / 2.,
            ),
        size,
    )
}

fn layout(
    viewport: gpui_kit::Size<Pixels>,
    topbar: Pixels,
    position: Option<Point<Pixels>>,
    aspect: f32,
) -> Bounds<Pixels> {
    let inset = zpx(12.).min(viewport.width / 4.).min(viewport.height / 4.);
    let width = zpx(260.).min((viewport.width - inset * 2.).max(px(0.)));
    let top = (topbar + inset).min((viewport.height - inset).max(inset));
    let max_height = (width * 0.625).min((viewport.height - top - inset).max(px(0.)));
    let width = width.min(max_height * aspect);
    let height = width / aspect;
    let max_x = (viewport.width - width - inset).max(inset);
    let max_y = (viewport.height - height - inset).max(top);
    let position = position.unwrap_or(point(max_x, top + zpx(40.)));
    Bounds::new(
        point(position.x.clamp(inset, max_x), position.y.clamp(top, max_y)),
        size(width, height),
    )
}

impl Render for BrowserPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(_) = self.source(cx) else {
            self.sample = None;
            self.grab = None;
            return div().into_any_element();
        };
        self.sample(window, cx);
        let aspect = self
            .image
            .as_ref()
            .map(|image| {
                let size = image.page.size(0);
                size.width.0 as f32 / size.height.0 as f32
            })
            .unwrap_or(1.6);
        let bounds = layout(
            window.viewport_size(),
            zpx(self.topbar),
            self.position.map(|p| point(zpx(p.x), zpx(p.y))),
            aspect,
        );
        let page_bounds = std::rc::Rc::new(std::cell::Cell::new(bounds));
        let glass_bounds = page_bounds.clone();
        let visible_page = self
            .image
            .as_ref()
            .map(|image| fitted_image_bounds(bounds, &image.page))
            .unwrap_or(bounds);
        let close_left = (visible_page.right() - bounds.left() - zpx(32.)).max(zpx(4.));
        let close_top = visible_page.top() - bounds.top() + zpx(8.);
        let light_close = self.image.as_ref().is_some_and(|image| image.light);
        let view = cx.entity().downgrade();
        let listeners = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let view = view.clone();
                window.on_mouse_event({
                    let view = view.clone();
                    move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() {
                            return;
                        }
                        let _ = view.update(cx, |this, cx| {
                            let Some(grab) = &mut this.grab else {
                                return;
                            };
                            if event.pressed_button != Some(MouseButton::Left) {
                                this.grab = None;
                                return;
                            }
                            let delta = event.position - grab.pointer;
                            if delta.x.abs().max(delta.y.abs()) > zpx(4.) {
                                grab.moved = true;
                            }
                            if grab.moved {
                                let bounds = layout(
                                    window.viewport_size(),
                                    zpx(this.topbar),
                                    Some(grab.origin + delta),
                                    aspect,
                                );
                                this.position = Some(point(
                                    del_core::zoom::unscaled(bounds.origin.x.as_f32()),
                                    del_core::zoom::unscaled(bounds.origin.y.as_f32()),
                                ));
                                cx.notify();
                            }
                            cx.stop_propagation();
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if !phase.capture() || event.button != MouseButton::Left {
                        return;
                    }
                    let _ = view.update(cx, |this, cx| {
                        let Some(grab) = this.grab.take() else {
                            return;
                        };
                        if !grab.moved
                            && let Some(target) = this.target.clone()
                        {
                            cx.emit(target);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .inset_0();
        div()
            .id("agent-browser-preview")
            .debug_selector(|| "agent-browser-preview".into())
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .occlude()
            .rounded(zpx(10.))
            .overflow_hidden()
            .shadow_lg()
            .cursor_pointer()
            .role(Role::Button)
            .aria_label("Open agent browser tab")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.grab = Some(Grab {
                        pointer: event.position,
                        origin: bounds.origin,
                        moved: false,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .size_full()
                    .overflow_hidden()
                    .rounded(zpx(10.))
                    .when_some(self.image.clone(), |body, image| {
                        body.child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    page_bounds.set(bounds);
                                    let _ = window.paint_image(
                                        bounds,
                                        fitted_image_bounds(bounds, &image.page),
                                        Corners::all(zpx(10.)),
                                        image.page.clone(),
                                        0,
                                        false,
                                    );
                                },
                            )
                            .size_full(),
                        )
                    })
                    .when(self.image.is_none(), |body| {
                        body.bg(gpui_kit::rgb(0x080808))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::icons::icon(IconName::Globe)
                                    .with_size(zpx(28.))
                                    .text_color(gpui_kit::rgb(0x888888)),
                            )
                    }),
            )
            .child(
                div()
                    .id("dismiss-browser-preview")
                    .debug_selector(|| "dismiss-browser-preview".into())
                    .role(Role::Button)
                    .aria_label("Dismiss browser preview")
                    .absolute()
                    .top(close_top)
                    .left(close_left)
                    .size(zpx(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .overflow_hidden()
                    .border_1()
                    .border_color(gpui_kit::rgba(0xffffff24))
                    .shadow_sm()
                    .when_some(self.image.clone(), |button, image| {
                        button.child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    let _ = window.paint_image(
                                        bounds,
                                        fitted_image_bounds(glass_bounds.get(), &image.glass),
                                        Corners::all(bounds.size.width / 2.),
                                        image.glass.clone(),
                                        0,
                                        false,
                                    );
                                },
                            )
                            .absolute()
                            .inset_0()
                            .size_full(),
                        )
                    })
                    .child(div().absolute().inset_0().rounded_full().bg(gpui_kit::rgba(
                        if light_close { 0xffffff30 } else { 0xffffff14 },
                    )))
                    .child(
                        crate::icons::icon(IconName::Close)
                            .with_size(zpx(crate::icons::TAB_CLOSE_GLYPH))
                            .text_color(gpui_kit::rgba(if light_close {
                                0x151515e0
                            } else {
                                0xffffffe0
                            })),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.dismiss(cx);
                    })),
            )
            .child(listeners)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    #[::gpui::test]
    fn preview_visibility_dismissal_and_tab_identity(cx: &mut ::gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        let preview = cx.new(|_| BrowserPreview::default());
        cx.update(|window, cx| {
            let request = |action: &str| {
                serde_json::from_value::<del_mcp::BrowserRequest>(
                    serde_json::json!({"action":action}),
                )
                .unwrap()
            };
            let original = panel.read(cx).current_pane();
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(request("new_tab"), window, cx)
                    .unwrap();
            });
            assert_eq!(
                panel.read(cx).current_pane(),
                original,
                "agent actions must not select Browser"
            );
            let id = panel.read(cx).active_browser_id();
            preview.update(cx, |preview, cx| {
                preview.set_session(Some(Tab::Draft(0)), &[Tab::Draft(0)], cx);
                preview.record(Tab::Draft(0), panel.clone(), id.clone(), cx);
                assert!(preview.source(cx).is_some());
            });
            panel.update(cx, |panel, cx| {
                let count = panel.agent_browser_tabs(cx).len();
                assert!(panel.show_browser_id(&id, window, cx));
                panel.set_preview_visible(true, cx);
                assert_eq!(
                    panel.agent_browser_tabs(cx).len(),
                    count,
                    "reveal must reuse the tab"
                );
            });
            preview.update(cx, |preview, cx| {
                assert!(
                    preview.source(cx).is_none(),
                    "no preview while browser is visible"
                )
            });
            panel.update(cx, |panel, cx| panel.set_preview_visible(false, cx));
            preview.update(cx, |preview, cx| {
                assert!(preview.source(cx).is_some());
                preview.dismiss(cx);
                preview.record(Tab::Draft(0), panel.clone(), id.clone(), cx);
                assert!(
                    preview.source(cx).is_none(),
                    "repeated tool activity must respect dismissal"
                );
                assert!(
                    panel.read(cx).browser_preview_source(&id).is_some(),
                    "dismissal must leave browser running"
                );
            });
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(request("new_tab"), window, cx)
                    .unwrap();
            });
            let next = panel.read(cx).active_browser_id();
            preview.update(cx, |preview, cx| {
                preview.record(Tab::Draft(0), panel.clone(), next.clone(), cx);
                assert!(preview.source(cx).is_some());
                preview.set_suppressed(true, 40., cx);
                assert!(preview.source(cx).is_none());
                preview.set_suppressed(false, 40., cx);
                assert!(preview.source(cx).is_some());
            });
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(request("close_tab"), window, cx)
                    .unwrap();
            });
            preview.update(cx, |preview, cx| {
                assert!(
                    preview.source(cx).is_none(),
                    "closed tab must not leave a stale preview"
                )
            });
        });
    }

    #[::gpui::test]
    fn open_browser_suppresses_preview_of_its_other_tab(cx: &mut ::gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        let preview = cx.new(|_| BrowserPreview::default());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(
                        serde_json::from_value(serde_json::json!({"action":"new_tab"})).unwrap(),
                        window,
                        cx,
                    )
                    .unwrap();
            });
            let blank = panel.read(cx).active_browser_id();
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(
                        serde_json::from_value(serde_json::json!({"action":"new_tab"})).unwrap(),
                        window,
                        cx,
                    )
                    .unwrap();
            });
            let target = panel.read(cx).active_browser_id();
            preview.update(cx, |preview, cx| {
                preview.set_session(Some(Tab::Draft(0)), &[Tab::Draft(0)], cx);
                preview.record(Tab::Draft(0), panel.clone(), target, cx);
                assert!(preview.source(cx).is_some());
            });
            panel.update(cx, |panel, cx| {
                assert!(panel.show_browser_id(&blank, window, cx));
                panel.set_preview_visible(true, cx);
            });
            preview.read_with(cx, |preview, cx| {
                assert!(
                    preview.source(cx).is_none(),
                    "a native browser tab must not clip a floating preview of another tab"
                );
            });
            panel.update(cx, |panel, cx| panel.set_preview_visible(false, cx));
            preview.read_with(cx, |preview, cx| assert!(preview.source(cx).is_some()));
        });
    }

    #[::gpui::test]
    fn switching_sessions_restores_each_preview_without_background_leaks(
        cx: &mut ::gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        let preview = cx.new(|_| BrowserPreview::default());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .browser_command(
                        serde_json::from_value(serde_json::json!({"action":"new_tab"})).unwrap(),
                        window,
                        cx,
                    )
                    .unwrap();
            });
            let id = panel.read(cx).active_browser_id();
            let a = Tab::Draft(10);
            let b = Tab::Draft(20);
            let open = [a.clone(), b.clone()];
            preview.update(cx, |preview, cx| {
                preview.set_session(Some(a.clone()), &open, cx);
                preview.record(a.clone(), panel.clone(), id.clone(), cx);
                preview.position = Some(point(70., 120.));
                assert!(preview.source(cx).is_some());
                preview.set_session(Some(b.clone()), &open, cx);
                assert!(preview.source(cx).is_none());
                preview.record(a.clone(), panel.clone(), id.clone(), cx);
                assert!(
                    preview.source(cx).is_none(),
                    "background activity cannot leak into the current session"
                );
                preview.record(b.clone(), panel.clone(), id.clone(), cx);
                preview.position = Some(point(250., 220.));
                preview.set_session(Some(a.clone()), &open, cx);
                assert_eq!(preview.target().unwrap().session, a);
                assert_eq!(preview.position, Some(point(70., 120.)));
                assert!(preview.source(cx).is_some());
                preview.dismiss(cx);
                preview.set_session(Some(b.clone()), &open, cx);
                assert_eq!(preview.target().unwrap().session, b);
                assert_eq!(preview.position, Some(point(250., 220.)));
                assert!(preview.source(cx).is_some());
                preview.record(a.clone(), panel.clone(), id.clone(), cx);
                preview.set_session(Some(a.clone()), &open, cx);
                assert!(
                    preview.source(cx).is_none(),
                    "dismissal must survive session switches"
                );
                preview.set_session(Some(b.clone()), std::slice::from_ref(&b), cx);
                assert!(
                    !preview.parked.contains_key(&a),
                    "closed sessions release their preview"
                );
                preview.set_session(Some(a), &open, cx);
                assert!(preview.source(cx).is_none());
            });
        });
    }

    #[::gpui::test]
    fn moved_browser_is_found_and_revealed_without_recreating_tab(cx: &mut ::gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (panel, cx) =
            cx.add_window_view(|window, cx| RightPanel::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            let workspace = cx
                .new(|cx| crate::workspace_tabs::WorkspaceTabs::new(dir.path().into(), window, cx));
            let request = serde_json::from_value(
                serde_json::json!({"action":"new_tab","url":"https://example.test/preview"}),
            )
            .unwrap();
            panel.update(cx, |panel, cx| {
                panel.browser_command(request, window, cx).unwrap();
            });
            let id = panel.read(cx).active_browser_id();
            let moved = panel
                .update(cx, |panel, cx| {
                    panel.take_tab(
                        &crate::right_panel::SidebarTab::Browser(id.clone()),
                        window,
                        cx,
                    )
                })
                .unwrap();
            workspace.update(cx, |workspace, cx| {
                workspace.accept_tab(moved.clone(), window, cx)
            });
            assert!(workspace.read(cx).browser_surface_visible(cx));
            workspace.update(cx, |workspace, cx| workspace.set_visible(false, cx));
            assert!(!workspace.read(cx).browser_surface_visible(cx));
            workspace.update(cx, |workspace, cx| workspace.set_visible(true, cx));
            assert!(RightPanel::find_browser_panel(&panel, &id, cx).is_none());
            assert_eq!(
                workspace.read(cx).find_browser_panel(&id, cx),
                Some(moved.clone())
            );
            let returned = workspace
                .update(cx, |workspace, cx| {
                    workspace.take_browser_tab(&id, window, cx)
                })
                .unwrap();
            assert_eq!(returned, moved);
            panel.update(cx, |panel, cx| {
                panel.accept_tab(returned, window, cx);
                assert!(panel.show_browser_id(&id, window, cx));
            });
            assert!(workspace.read(cx).find_browser_panel(&id, cx).is_none());
            assert_eq!(RightPanel::find_browser_panel(&panel, &id, cx), Some(moved));
            assert!(panel.read(cx).browser_surface_visible(cx));
            panel.update(cx, |panel, cx| panel.set_preview_visible(false, cx));
            assert!(!panel.read(cx).browser_surface_visible(cx));
            assert_eq!(
                panel
                    .read(cx)
                    .agent_browser_tabs(cx)
                    .iter()
                    .filter(|tab| tab.id == id)
                    .count(),
                1
            );
        });
    }

    #[::gpui::test]
    fn dragging_does_not_open_and_close_only_dismisses(cx: &mut ::gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        struct Fixture(Entity<BrowserPreview>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().relative().size_full().child(self.0.clone())
            }
        }
        let mut handles = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let panel = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
            panel.update(cx, |host, cx| {
                host.browser_command(
                    serde_json::from_value(serde_json::json!({"action":"new_tab"})).unwrap(),
                    window,
                    cx,
                )
                .unwrap();
            });
            let id = panel.read(cx).active_browser_id();
            let preview = cx.new(|cx| {
                let mut preview = BrowserPreview::default();
                preview.set_session(Some(Tab::Draft(0)), &[Tab::Draft(0)], cx);
                preview.record(Tab::Draft(0), panel.clone(), id, cx);
                preview
            });
            handles = Some((panel, preview.clone()));
            Fixture(preview)
        });
        let (panel, preview) = handles.unwrap();
        let original_tabs = panel.read_with(cx, |panel, cx| panel.agent_browser_tabs(cx).len());
        let opened = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|_, cx| {
            let opened = opened.clone();
            cx.subscribe(&preview, move |_, _: &OpenBrowser, _| {
                opened.set(opened.get() + 1)
            })
            .detach();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let initial = cx.debug_bounds("agent-browser-preview").unwrap();
        let from = initial.center();
        let to = from + point(zpx(-80.), zpx(80.));
        cx.simulate_mouse_move(from, None, Modifiers::default());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let moved = cx.debug_bounds("agent-browser-preview").unwrap();
        assert_ne!(moved.origin, initial.origin);
        assert_eq!(opened.get(), 0, "dragging must not reveal the sidebar");
        let click = |cx: &mut ::gpui::VisualTestContext, point| {
            cx.simulate_mouse_move(point, None, Modifiers::default());
            cx.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        click(cx, moved.center());
        assert_eq!(opened.get(), 1);
        let close = cx.debug_bounds("dismiss-browser-preview").unwrap().center();
        click(cx, close);
        assert_eq!(opened.get(), 1, "close must not also reveal the browser");
        preview.read_with(cx, |preview, cx| assert!(preview.source(cx).is_none()));
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.agent_browser_tabs(cx).len(), original_tabs)
        });
    }

    #[test]
    fn snapshot_decoding_uses_real_png_and_rejects_invalid_data() {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
        assert!(decode_snapshot(serde_json::json!({"image":encoded})).is_some());
        assert!(decode_snapshot(serde_json::json!({"image":"invalid"})).is_none());
    }

    #[test]
    fn glass_blurs_page_detail_without_changing_image_geometry() {
        let image = image::RgbaImage::from_fn(64, 40, |x, _| {
            let value = if x % 2 == 0 { 0 } else { 255 };
            image::Rgba([value, value, value, 255])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
        let snapshot = decode_snapshot(serde_json::json!({"image":encoded})).unwrap();
        assert_eq!(snapshot.page.size(0), snapshot.glass.size(0));
        let page = snapshot.page.as_bytes(0).unwrap();
        let glass = snapshot.glass.as_bytes(0).unwrap();
        let center = (20 * 64 + 32) * 4;
        assert_eq!(page[center], 0);
        assert!(glass[center] > 80 && glass[center] < 180);
        let bounds = Bounds::new(point(px(30.), px(40.)), size(px(260.), px(162.5)));
        assert_eq!(
            fitted_image_bounds(bounds, &snapshot.page),
            fitted_image_bounds(bounds, &snapshot.glass)
        );
    }

    #[test]
    fn card_stays_inside_resized_window() {
        for (w, h) in [(1200., 800.), (360., 300.), (120., 90.)] {
            let viewport = size(px(w), px(h));
            for position in [
                None,
                Some(point(px(-500.), px(-500.))),
                Some(point(px(2000.), px(2000.))),
            ] {
                for aspect in [0.7, 1.4, 1.6, 3.] {
                    let bounds = layout(viewport, px(40.), position, aspect);
                    assert!(bounds.left() >= px(0.) && bounds.top() >= px(0.));
                    assert!(bounds.right() <= viewport.width && bounds.bottom() <= viewport.height);
                    assert!(
                        (bounds.size.width.as_f32() / bounds.size.height.as_f32() - aspect).abs()
                            < 0.001,
                        "preview frame must match page aspect ratio without letterboxing"
                    );
                }
            }
        }
    }
}
