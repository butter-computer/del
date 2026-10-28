//! Tools opened inside one session. The right sidebar owns separate tool views.
use crate::appearance::{rgb, rgba};
use crate::new_tab::destination;
use crate::right_panel::{Pane, RightPanel};
use crate::zoom::px as zpx;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;
use std::{cell::Cell, rc::Rc, time::Duration};

pub const TAB_WIDTH: f32 = 160.0;
const TAB_GAP: f32 = 2.0;

#[derive(Clone)]
struct DraggedWorkspaceTab {
    owner: EntityId,
    id: u64,
    label: String,
    kind: Kind,
    origin_x: Rc<Cell<Pixels>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    pointer_y: Pixels,
    offset_x: Pixels,
}

impl Render for DraggedWorkspaceTab {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.bounds.get();
        let width = zpx(TAB_WIDTH).min(bounds.size.width.max(zpx(0.)));
        let natural_left = window.mouse_position().x - self.offset_x;
        let left = natural_left.clamp(bounds.left(), (bounds.right() - width).max(bounds.left()));
        div()
            .w(zpx(TAB_WIDTH))
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .child(
                h_flex()
                    .relative()
                    .top(self.pointer_y - window.mouse_position().y)
                    .left(left - natural_left)
                    .w(width)
                    .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(rgb(0x303033))
                    .text_color(rgb(0xf5f5f5))
                    .text_size(zpx(del_core::typography::TEXT_MD))
                    .shadow_md()
                    .child(crate::icons::icon(self.kind.icon()).with_size(zpx(15.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.label.clone()),
                    ),
            )
    }
}

#[derive(Clone, Copy)]
struct TabDrag {
    id: u64,
    source: usize,
    target: usize,
    previous: usize,
}

fn drag_target(source: usize, delta: f32, current: usize, count: usize) -> usize {
    let slot = TAB_WIDTH + TAB_GAP;
    let x = source as f32 * slot + delta;
    let target = (x / slot)
        .round()
        .clamp(0.0, count.saturating_sub(1) as f32) as usize;
    // Match the titlebar's dead zone around slot boundaries.
    if (target > current && x < (current as f32 + 0.5) * slot + 12.)
        || (target < current && x > (current as f32 - 0.5) * slot - 12.)
    {
        current
    } else {
        target
    }
}

fn drag_offset(index: usize, source: usize, target: usize) -> f32 {
    if source < target && index > source && index <= target {
        -(TAB_WIDTH + TAB_GAP)
    } else if target < source && index >= target && index < source {
        TAB_WIDTH + TAB_GAP
    } else {
        0.
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Blank,
    Files,
    Changes,
    Terminal,
    Browser,
    SideChat,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Blank => "New tab",
            Self::Files => "Files",
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
            Self::SideChat => "Side chat",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Blank => Pane::NewTab,
            Self::Files => Pane::Files,
            Self::Changes => Pane::Changes,
            Self::Terminal => Pane::Terminal,
            Self::Browser => Pane::Browser,
            Self::SideChat => Pane::SideChat,
        }
        .icon()
    }
}
struct ToolTab {
    id: u64,
    kind: Kind,
    panel: Option<Entity<RightPanel>>,
    browser: Option<crate::browser::Browser>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SavedSideTabs {
    ids: Vec<String>,
    active: Option<usize>,
}

pub struct WorkspaceTabs {
    side_context: Option<crate::side_chat::Context>,
    storage_key: Option<String>,
    drag: Option<TabDrag>,
    drag_origin: Rc<Cell<Pixels>>,
    strip_bounds: Rc<Cell<Bounds<Pixels>>>,
    root: PathBuf,
    tabs: Vec<ToolTab>,
    active: Option<u64>,
    next_id: u64,
    visible: bool,
    address: Entity<InputState>,
    profile: String,
    scroll: ScrollHandle,
}
impl EventEmitter<crate::side_chat::SendToMain> for WorkspaceTabs {}
impl EventEmitter<crate::chat::OpenLink> for WorkspaceTabs {}
impl WorkspaceTabs {
    pub fn set_side_context(&mut self, context: crate::side_chat::Context) {
        self.side_context = Some(context);
    }

    fn storage_path(key: &str) -> Option<PathBuf> {
        #[cfg(test)]
        {
            let _ = key;
            None
        }
        #[cfg(not(test))]
        {
            let name: String = key.bytes().map(|b| format!("{b:02x}")).collect();
            Some(
                del_agent::data_dir()?
                    .join("side-chat-tabs")
                    .join(format!("{name}.json")),
            )
        }
    }
    pub fn save_side_chats(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |panel, cx| panel.save_side_chat(cx));
            }
        }
        self.persist(cx);
    }

    pub fn persist(&self, cx: &App) {
        if let Some(path) = self.storage_key.as_deref().and_then(Self::storage_path) {
            let tabs: Vec<_> = self
                .tabs
                .iter()
                .filter_map(|tab| {
                    tab.panel
                        .as_ref()
                        .and_then(|panel| panel.read(cx).side_chat_id())
                        .map(|id| (tab.id, id))
                })
                .collect();
            let saved = SavedSideTabs {
                active: tabs.iter().position(|(id, _)| Some(*id) == self.active),
                ids: tabs.into_iter().map(|(_, id)| id).collect(),
            };
            if let Ok(bytes) = serde_json::to_vec(&saved)
                && let Err(error) =
                    crate::sessions::write_atomic(&path, &bytes, crate::sessions::PRIVATE)
            {
                eprintln!("Could not save side chat tabs: {error}");
            }
        }
    }
    pub fn set_storage_key(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.storage_key.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = self.storage_key.replace(key.clone()) {
            // Draft -> saved session: move the existing strip, don't reopen it.
            self.persist(cx);
            if let Some(path) = Self::storage_path(&old) {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
        if let Some(path) = Self::storage_path(&key) {
            let saved: SavedSideTabs = match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(saved) => saved,
                    Err(error) => {
                        eprintln!("Could not read side chat tabs: {error}");
                        return;
                    }
                },
                Err(_) => return,
            };
            let mut active = None;
            for (index, id) in saved.ids.into_iter().enumerate() {
                let Some(context) = self.side_context.clone() else {
                    break;
                };
                let panel = cx.new(|cx| {
                    let mut panel = RightPanel::new(self.root.clone(), window, cx);
                    panel.set_side_context(context);
                    panel.restore_side_chat(id, window, cx);
                    panel
                });
                cx.observe(&panel, |_, _, cx| cx.notify()).detach();
                cx.subscribe(&panel, |_, _, event: &crate::chat::OpenLink, cx| {
                    cx.emit(crate::chat::OpenLink(event.0.clone()))
                })
                .detach();
                let tab_id = self.next_id;
                self.next_id += 1;
                if saved.active == Some(index) {
                    active = Some(tab_id);
                }
                self.tabs.push(ToolTab {
                    id: tab_id,
                    kind: Kind::SideChat,
                    panel: Some(panel),
                    browser: None,
                });
            }
            self.select(active, window, cx);
        }
    }

    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter a URL"));
        cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.navigate(window, cx);
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.persist(cx);
            async {}
        })
        .detach();
        Self {
            side_context: None,
            storage_key: None,
            drag: None,
            drag_origin: Default::default(),
            strip_bounds: Default::default(),
            root,
            tabs: vec![],
            active: None,
            next_id: 1,
            visible: true,
            address,
            profile: uuid::Uuid::new_v4().to_string(),
            scroll: ScrollHandle::new(),
        }
    }
    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.persist(cx);
        self.drag = None;
        for tab in self.tabs.drain(..) {
            if let Some(panel) = tab.panel {
                panel.update(cx, |p, cx| p.release(cx));
            }
            if let Some(browser) = tab.browser {
                browser.hide();
            }
        }
        self.active = None;
    }

    pub fn is_chat(&self) -> bool {
        self.active.is_none()
    }
    pub fn set_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root.clone();
        for tab in &self.tabs {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |p, cx| p.set_root(root.clone(), window, cx));
            }
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if !visible {
            self.drag = None;
        }
        for tab in &self.tabs {
            let shown = visible && self.active == Some(tab.id);
            if let Some(panel) = &tab.panel {
                panel.update(cx, |p, cx| p.set_preview_visible(shown, cx));
            }
            if !shown && let Some(browser) = &tab.browser {
                browser.hide();
            }
        }
    }
    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(ToolTab {
            id,
            kind: Kind::Blank,
            panel: None,
            browser: None,
        });
        self.select(Some(id), window, cx);
    }
    pub fn select_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == 0 {
            self.select(None, window, cx);
        } else if let Some(tab) = self.tabs.get(index - 1) {
            self.select(Some(tab.id), window, cx);
        }
    }
    pub fn select_relative(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .map_or(0, |i| i + 1);
        let next =
            (current as isize + direction).rem_euclid((self.tabs.len() + 1) as isize) as usize;
        self.select_index(next, window, cx);
    }
    fn select(&mut self, id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = id;
        self.set_visible(self.visible, cx);
        self.address
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus(window, cx);
        cx.notify();
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|tab| Some(tab.id) == self.active) {
            if tab.kind == Kind::SideChat {
                if let Some(panel) = &tab.panel {
                    panel.update(cx, |p, cx| p.focus_side_chat(window, cx));
                }
            } else if tab.kind == Kind::Terminal {
                if let Some(panel) = &tab.panel {
                    panel.update(cx, |p, cx| p.show_terminal(window, cx));
                }
            } else if tab.kind != Kind::Browser {
                self.address.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.active else {
            return false;
        };
        self.close(id, window, cx);
        true
    }
    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        if self.tabs[index]
            .panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).has_unsaved_edits())
        {
            window.push_notification(
                gpui_kit::component::notification::Notification::info(
                    "Save or revert your file edits before closing this tab.",
                ),
                cx,
            );
            return;
        }
        self.drag = None;
        let tab = self.tabs.remove(index);
        if let Some(panel) = tab.panel {
            panel.update(cx, |p, cx| {
                p.forget_side_chat(cx);
                p.release(cx);
            });
        }
        if let Some(browser) = tab.browser {
            browser.hide();
        }
        if self.active == Some(id) {
            let next = index
                .checked_sub(1)
                .and_then(|i| self.tabs.get(i))
                .map(|t| t.id);
            self.select(next, window, cx);
        }
        cx.notify();
    }
    fn choose(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == self.active) else {
            return;
        };
        let pane = match kind {
            Kind::Files => Pane::Files,
            Kind::Changes => Pane::Changes,
            Kind::Terminal => Pane::Terminal,
            Kind::SideChat => Pane::SideChat,
            _ => return,
        };
        let panel = cx.new(|cx| {
            let mut panel = RightPanel::new(self.root.clone(), window, cx);
            if let Some(context) = self.side_context.clone() {
                panel.set_side_context(context);
            }
            panel.show_workspace(pane, window, cx);
            panel
        });
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&panel, |_, _, event: &crate::side_chat::SendToMain, cx| {
            cx.emit(event.clone())
        })
        .detach();
        cx.subscribe(&panel, |_, _, event: &crate::chat::OpenLink, cx| {
            cx.emit(crate::chat::OpenLink(event.0.clone()))
        })
        .detach();
        tab.kind = kind;
        tab.panel = Some(panel);
        cx.notify();
    }
    fn navigate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = destination(self.address.read(cx).value().as_ref()) else {
            return;
        };
        // Keep a file editor or terminal alive when its address bar opens a site.
        if self
            .tabs
            .iter()
            .any(|t| Some(t.id) == self.active && t.kind != Kind::Blank)
        {
            self.new_tab(window, cx);
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.id) == self.active) else {
            return;
        };
        tab.kind = Kind::Browser;
        tab.browser = Some(crate::browser::Browser::new(&url, &self.profile));
        cx.notify();
    }
    fn begin_drag(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(source) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.drag = Some(TabDrag {
            id,
            source,
            target: source,
            previous: source,
        });
        cx.notify();
    }

    fn move_drag(&mut self, owner: EntityId, id: u64, delta: f32, cx: &mut Context<Self>) {
        if owner != cx.entity_id() {
            return;
        }
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.id != id {
            return;
        }
        let target = drag_target(drag.source, delta, drag.target, self.tabs.len());
        if drag.target != target {
            drag.previous = drag.target;
            drag.target = target;
            crate::haptics::tick();
            cx.notify();
        }
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Some(source) = self.tabs.iter().position(|tab| tab.id == drag.id) {
            let tab = self.tabs.remove(source);
            self.tabs.insert(drag.target.min(self.tabs.len()), tab);
        }
        // The original chat is not in `tabs`, so it can never move or be displaced.
        // Selection and tool entities use stable ids and survive a reorder.
        cx.notify();
    }

    pub fn render_strip(&mut self, hints: bool, cx: &mut Context<Self>) -> AnyElement {
        if self.drag.is_some() && !cx.has_active_drag() {
            self.drag = None;
        }
        let bounds = self.strip_bounds.clone();
        let drag = self.drag;
        let row = h_flex()
            .id("workspace-tools-strip")
            .gap(zpx(TAB_GAP))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedWorkspaceTab>, _, cx| {
                    let drag = event.drag(cx);
                    let delta = del_core::zoom::unscaled(
                        (event.event.position.x - drag.origin_x.get()).as_f32(),
                    );
                    this.move_drag(drag.owner, drag.id, delta, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .on_drop(cx.listener(|this, dragged: &DraggedWorkspaceTab, _, cx| {
                if dragged.owner == cx.entity_id() {
                    this.finish_drag(cx);
                }
            }))
            .flex_none()
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let id = tab.id;
                let active = self.active == Some(id);
                let group: SharedString = format!("workspace-tab-{id}").into();
                let label = tab
                    .browser
                    .as_ref()
                    .and_then(|b| url::Url::parse(&b.url()).ok())
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_else(|| tab.kind.label().into());
                let payload = DraggedWorkspaceTab {
                    owner: cx.entity_id(),
                    id,
                    label: label.clone(),
                    kind: tab.kind,
                    origin_x: self.drag_origin.clone(),
                    bounds: self.strip_bounds.clone(),
                    pointer_y: Pixels::ZERO,
                    offset_x: Pixels::ZERO,
                };
                let origin = self.drag_origin.clone();
                let workspace = cx.entity();
                let element = h_flex()
                    .id(group.clone())
                    .debug_selector(move || format!("workspace-tool-{id}"))
                    .group(group.clone())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Open {label} tab"))
                    .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                    .w(zpx(TAB_WIDTH))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(zpx(del_core::typography::TEXT_MD))
                    .text_color(if active {
                        rgb(0xf5f5f5)
                    } else {
                        rgba(0xffffff8c)
                    })
                    .when(active, |t| t.bg(rgba(0xffffff14)))
                    .hover(|t| t.bg(rgba(0xffffff0a)))
                    .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                        origin.set(event.position.x);
                        cx.stop_propagation();
                    })
                    .on_drag(payload, move |drag, offset, window, cx| {
                        let movement = window.mouse_position().x - drag.origin_x.get();
                        workspace.update(cx, |this, cx| {
                            this.begin_drag(drag.id, cx);
                            // A fast drag can cross slots before GPUI starts its
                            // drag payload; include that first movement too.
                            this.move_drag(
                                drag.owner,
                                drag.id,
                                del_core::zoom::unscaled(movement.as_f32()),
                                cx,
                            );
                        });
                        let mut preview = drag.clone();
                        preview.offset_x = offset.x - movement;
                        preview.pointer_y = window.mouse_position().y;
                        cx.new(|_| preview)
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(Some(id), window, cx)),
                    )
                    .child(crate::icons::icon(tab.kind.icon()).with_size(zpx(15.)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(hints && index < 8, |t| {
                        t.child(div().text_size(zpx(10.)).child(format!("⌘{}", index + 2)))
                    })
                    .child(
                        div()
                            .id(("close-workspace-tab", id))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Close {} tab", tab.kind.label()))
                            .size(zpx(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .opacity(0.0)
                            .group_hover(group, |b| b.opacity(1.))
                            .hover(|b| b.bg(rgba(0xffffff14)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(id, window, cx);
                            }))
                            .child(crate::icons::icon(IconName::Close).with_size(zpx(13.))),
                    );
                let old_offset = drag.map_or(0., |d| drag_offset(index, d.source, d.previous));
                let new_offset = drag.map_or(0., |d| drag_offset(index, d.source, d.target));
                div()
                    .flex_none()
                    .when(drag.is_some_and(|d| d.id == id), |tab| tab.opacity(0.))
                    .child(element)
                    .with_animation(
                        SharedString::from(format!(
                            "workspace-tab-shift-{id}-{old_offset}-{new_offset}"
                        )),
                        Animation::new(Duration::from_millis(90)).with_easing(ease_out_quint()),
                        move |tab, t| {
                            tab.relative()
                                .left(zpx(old_offset + (new_offset - old_offset) * t))
                        },
                    )
            }));
        h_flex()
            .flex_none()
            .on_children_prepainted(move |children, _, _| {
                if let Some(bound) = children.first() {
                    bounds.set(*bound);
                }
            })
            .child(row)
            .into_any_element()
    }
    fn choose_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match pane {
            Pane::Files => Kind::Files,
            Pane::Changes => Kind::Changes,
            Pane::Terminal => Kind::Terminal,
            Pane::SideChat => Kind::SideChat,
            _ => return,
        };
        self.choose(kind, window, cx);
    }
}
impl Render for WorkspaceTabs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tabs.iter().find(|t| Some(t.id) == self.active);
        if let Some(browser) = tab.and_then(|t| t.browser.as_ref()) {
            return div()
                .size_full()
                .when(self.visible, |view| view.child(browser.element()))
                .into_any_element();
        }
        let kind = tab.map_or(Kind::Blank, |t| t.kind);
        let panel = tab.and_then(|t| t.panel.clone());
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .when(kind != Kind::SideChat, |v| {
                v.child(crate::new_tab::address_bar(&self.address))
            })
            .child(if kind == Kind::Blank {
                crate::new_tab::tools(false, &self.scroll, Self::choose_tool, cx)
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .when_some(panel, |v, p| v.child(p))
                    .into_any_element()
            })
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    #[::core::prelude::v1::test]
    fn address_accepts_urls_localhost_and_searches() {
        assert_eq!(
            destination(" example.com ").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            destination("localhost:8766").as_deref(),
            Some("http://localhost:8766/")
        );
        assert_eq!(
            destination("http://127.0.0.1:8766/a").as_deref(),
            Some("http://127.0.0.1:8766/a")
        );
        assert!(destination("two words").unwrap().contains("q=two+words"));
        assert!(destination("file:///tmp/private").is_none());
        assert!(destination("  ").is_none());
    }

    #[::gpui::test]
    fn session_tabs_keep_their_tools_and_close_back_to_chat(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            let other = cx.new(|cx| WorkspaceTabs::new(dir.path().into(), window, cx));
            workspace.update(cx, |w, cx| {
                assert!(w.is_chat());
                w.new_tab(window, cx);
                let first = w.active;
                w.choose(Kind::Files, window, cx);
                let files = w.tabs[0].panel.clone().unwrap();
                w.new_tab(window, cx);
                assert_eq!(w.tabs.len(), 2);
                w.choose(Kind::Changes, window, cx);
                w.select_index(1, window, cx);
                assert_eq!(w.active, first);
                assert_eq!(w.tabs[0].panel.as_ref(), Some(&files));
                w.set_visible(false, cx);
                w.set_visible(true, cx);
                assert_eq!(w.active, first);
                assert!(other.read(cx).is_chat());
                assert!(other.read(cx).tabs.is_empty());
                w.select_relative(-1, window, cx);
                assert!(w.is_chat());
                w.select_relative(-1, window, cx);
                assert_eq!(w.active, Some(w.tabs[1].id));
                assert!(w.close_active(window, cx));
                assert_eq!(w.active, first);
                assert!(w.close_active(window, cx));
                assert!(w.is_chat());
                assert!(!w.close_active(window, cx));
            });
        });
    }

    #[::gpui::test]
    fn reordering_preserves_tool_views_selection_and_session_boundaries(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            let other = cx.new(|cx| WorkspaceTabs::new(dir.path().into(), window, cx));
            other.update(cx, |other, cx| other.new_tab(window, cx));
            workspace.update(cx, |w, cx| {
                w.new_tab(window, cx);
                w.choose(Kind::Files, window, cx);
                let first = w.tabs[0].id;
                let files = w.tabs[0].panel.clone().unwrap();
                w.new_tab(window, cx);
                let second = w.tabs[1].id;
                w.new_tab(window, cx);
                let third = w.tabs[2].id;
                w.select(Some(first), window, cx);

                w.begin_drag(third, cx);
                w.move_drag(other.entity_id(), third, -1000., cx);
                assert_eq!(
                    w.drag.unwrap().target,
                    2,
                    "a different session cannot move these tabs"
                );
                w.move_drag(cx.entity_id(), third, -1000., cx);
                w.finish_drag(cx);
                assert_eq!(
                    w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                    vec![third, first, second]
                );
                assert_eq!(w.active, Some(first));
                assert_eq!(w.tabs[1].panel.as_ref(), Some(&files));
                assert_eq!(other.read(cx).tabs.len(), 1);

                w.begin_drag(third, cx);
                w.move_drag(cx.entity_id(), third, 1000., cx);
                w.finish_drag(cx);
                assert_eq!(
                    w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                    vec![first, second, third]
                );
                w.select_index(0, window, cx);
                assert!(w.is_chat());
                w.begin_drag(third, cx);
                w.move_drag(cx.entity_id(), third, -1000., cx);
                w.finish_drag(cx);
                assert!(
                    w.is_chat(),
                    "reordering tools never displaces the original chat"
                );
                w.select_relative(1, window, cx);
                assert_eq!(
                    w.active,
                    Some(third),
                    "keyboard traversal follows the new order"
                );

                w.begin_drag(first, cx);
                w.move_drag(cx.entity_id(), first, 1000., cx);
                w.set_visible(false, cx);
                w.finish_drag(cx);
                assert_eq!(
                    w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                    vec![third, first, second],
                    "switching sessions cancels an unfinished drag"
                );
                w.set_visible(true, cx);
                w.close(first, window, cx);
                assert_eq!(
                    w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                    vec![third, second]
                );
                assert_eq!(w.active, Some(third));
            });
        });
    }

    struct DragFixture(Entity<WorkspaceTabs>);
    impl Render for DragFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            h_flex()
                .child(
                    div()
                        .id("original-chat")
                        .debug_selector(|| "original-chat".into())
                        .w(zpx(TAB_WIDTH))
                        .flex_none()
                        .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                        .child("Original chat"),
                )
                .child(self.0.update(cx, |w, cx| w.render_strip(false, cx)))
        }
    }

    #[::gpui::test]
    fn mouse_drag_reorders_tools_without_crossing_the_original_chat(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let mut workspace = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut w = WorkspaceTabs::new(dir.path().into(), window, cx);
                for _ in 0..3 {
                    w.new_tab(window, cx);
                }
                w
            });
            workspace = Some(view.clone());
            cx.observe(&view, |_, _, cx| cx.notify()).detach();
            DragFixture(view)
        });
        let workspace = workspace.unwrap();
        cx.run_until_parked();
        let original = cx.debug_bounds("original-chat").unwrap();
        let last = cx.debug_bounds("workspace-tool-3").unwrap();
        cx.simulate_mouse_move(last.center(), None, Modifiers::default());
        cx.simulate_mouse_down(last.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(original.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_up(original.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        workspace.read_with(cx, |w, _| {
            assert_eq!(
                w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![3, 1, 2]
            );
            assert_eq!(w.active, Some(3));
        });
        assert_eq!(cx.debug_bounds("original-chat").unwrap(), original);

        // A normal multi-event drag can finish outside the row and still settles
        // into the last tool slot, without moving the fixed original tab.
        let moved = cx.debug_bounds("workspace-tool-3").unwrap().center();
        let outside = point(moved.x + zpx(900.), moved.y + zpx(80.));
        cx.simulate_mouse_move(moved, None, Modifiers::default());
        cx.simulate_mouse_down(moved, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            point(moved.x + zpx(12.), moved.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        workspace.read_with(cx, |w, _| {
            assert_eq!(
                w.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![1, 2, 3]
            );
            assert_eq!(w.active, Some(3));
        });
        assert_eq!(cx.debug_bounds("original-chat").unwrap(), original);
    }

    #[::gpui::test]
    fn url_from_a_tool_keeps_the_original_tool_open(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let (workspace, cx) =
            cx.add_window_view(|window, cx| WorkspaceTabs::new(dir.path().into(), window, cx));
        cx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.new_tab(window, cx);
                w.choose(Kind::Files, window, cx);
                w.address.update(cx, |input, cx| {
                    input.set_value("http://localhost:8766", window, cx)
                });
                w.navigate(window, cx);
                assert_eq!(w.tabs.len(), 2);
                assert_eq!(w.tabs[0].kind, Kind::Files);
                assert!(w.tabs[0].panel.is_some());
                assert_eq!(w.tabs[1].kind, Kind::Browser);
                assert_eq!(
                    w.tabs[1].browser.as_ref().unwrap().url(),
                    "http://localhost:8766/"
                );
                w.close_active(window, cx);
                assert_eq!(w.active, Some(w.tabs[0].id));
            })
        });
    }
}
