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
use std::{cell::Cell, rc::Rc};

pub const TAB_WIDTH: f32 = crate::icons::TOOL_TAB_WIDTH;
const TAB_GAP: f32 = 2.0;

#[derive(Clone)]
pub(crate) struct DraggedWorkspaceTab {
    pub source: WeakEntity<WorkspaceTabs>,
    pub placement: crate::tab_transfer::Placement,
    pub owner: EntityId,
    pub id: u64,
    label: String,
    kind: Kind,
    origin: Rc<Cell<Point<Pixels>>>,
    cursor_offset: Point<Pixels>,
    grab_offset: Point<Pixels>,
}

impl Render for DraggedWorkspaceTab {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let natural = window.mouse_position() - self.grab_offset;
        let source_y = self.origin.get().y - self.grab_offset.y;
        let rect = self.placement.preview(natural, source_y, zpx(TAB_WIDTH));
        let raw = window.mouse_position() - self.cursor_offset;
        div()
            .w(zpx(TAB_WIDTH))
            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
            .child(
                h_flex()
                    .relative()
                    .top(rect.top() - raw.y)
                    .left(rect.left() - raw.x)
                    .w(rect.size.width)
                    .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(crate::appearance::dragged_tab_surface())
                    .border_1()
                    .border_color(crate::appearance::separator())
                    .text_color(rgb(0xf5f5f5))
                    .text_size(zpx(del_core::typography::TEXT_MD))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Kind {
    Blank,
    Files,
    Changes,
    Terminal,
    Browser,
    SideChat,
}
impl Kind {
    fn from_pane(pane: Pane) -> Self {
        match pane {
            Pane::Files => Self::Files,
            Pane::Changes => Self::Changes,
            Pane::Terminal => Self::Terminal,
            Pane::Browser => Self::Browser,
            Pane::SideChat => Self::SideChat,
            Pane::NewTab => Self::Blank,
        }
    }

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
    draft: String,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedSideTabs {
    #[serde(default)]
    tabs: Option<Vec<SavedToolTab>>,
    #[serde(default)]
    ids: Vec<String>,
    active: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedToolTab {
    kind: Kind,
    panel: Option<crate::right_panel::SavedPanel>,
    url: Option<String>,
    profile: String,
    draft: String,
}

pub struct WorkspaceTabs {
    pub(crate) transfer_peer: Option<WeakEntity<RightPanel>>,
    incoming: crate::tab_transfer::InsertionGap,
    row_scroll: ScrollHandle,
    motion: crate::tab_motion::Strip<u64>,
    reveal_active: Rc<Cell<bool>>,
    watched_panels: std::collections::HashSet<EntityId>,
    transfer_bounds: Rc<Cell<Bounds<Pixels>>>,
    side_context: Option<crate::side_chat::Context>,
    storage_key: Option<String>,
    drag: Option<TabDrag>,
    drag_origin: Rc<Cell<Point<Pixels>>>,
    drag_outside: bool,
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
            let saved = self.snapshot(cx);
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
            self.restore_tabs(saved, window, cx);
        }
    }

    pub(crate) fn snapshot(&self, cx: &App) -> SavedSideTabs {
        SavedSideTabs {
            ids: Vec::new(),
            active: self.tabs.iter().position(|tab| Some(tab.id) == self.active),
            tabs: Some(
                self.tabs
                    .iter()
                    .map(|tab| SavedToolTab {
                        kind: tab.kind,
                        panel: tab
                            .panel
                            .as_ref()
                            .map(|panel| panel.read(cx).snapshot(false, cx)),
                        url: tab.browser.as_ref().map(|browser| browser.url()),
                        profile: self.profile.clone(),
                        draft: if Some(tab.id) == self.active {
                            self.address.read(cx).value().to_string()
                        } else {
                            tab.draft.clone()
                        },
                    })
                    .collect(),
            ),
        }
    }

    fn restore_tabs(&mut self, saved: SavedSideTabs, window: &mut Window, cx: &mut Context<Self>) {
        let mut tabs = Vec::new();
        if let Some(saved_tabs) = saved.tabs {
            for saved in saved_tabs {
                self.profile = saved.profile.clone();
                let panel = saved.panel.map(|saved| {
                    cx.new(|cx| {
                        let mut panel = RightPanel::new(self.root.clone(), window, cx);
                        if let Some(context) = self.side_context.clone() {
                            panel.set_side_context(context);
                        }
                        panel.restore_workspace(saved, window, cx);
                        panel.set_tab_zone(false);
                        panel
                    })
                });
                if let Some(panel) = &panel {
                    self.watch_panel(panel, cx);
                }
                let browser = saved
                    .url
                    .map(|url| crate::browser::Browser::new(&url, &saved.profile));
                let id = self.next_id;
                self.next_id += 1;
                tabs.push(ToolTab {
                    id,
                    kind: saved.kind,
                    panel,
                    browser,
                    draft: saved.draft,
                });
            }
        } else {
            // Migrate the earlier side-chat-only manifest without losing any chats.
            for id in saved.ids {
                let Some(context) = self.side_context.clone() else {
                    break;
                };
                let panel = cx.new(|cx| {
                    let mut panel = RightPanel::new(self.root.clone(), window, cx);
                    panel.set_side_context(context);
                    panel.restore_side_chat(id, window, cx);
                    panel
                });
                self.watch_panel(&panel, cx);
                let id = self.next_id;
                self.next_id += 1;
                tabs.push(ToolTab {
                    id,
                    kind: Kind::SideChat,
                    panel: Some(panel),
                    browser: None,
                    draft: String::new(),
                });
            }
        }
        let active = saved
            .active
            .and_then(|index| tabs.get(index))
            .map(|tab| tab.id);
        self.tabs.extend(tabs);
        self.select(active, window, cx);
    }

    fn watch_panel(&mut self, panel: &Entity<RightPanel>, cx: &mut Context<Self>) {
        if !self.watched_panels.insert(panel.entity_id()) {
            return;
        }
        cx.observe(panel, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            panel,
            |this, panel, event: &crate::side_chat::SendToMain, cx| {
                if this
                    .tabs
                    .iter()
                    .any(|tab| tab.panel.as_ref() == Some(&panel))
                {
                    cx.emit(event.clone());
                }
            },
        )
        .detach();
        cx.subscribe(panel, |this, panel, event: &crate::chat::OpenLink, cx| {
            if this
                .tabs
                .iter()
                .any(|tab| tab.panel.as_ref() == Some(&panel))
            {
                cx.emit(crate::chat::OpenLink(event.0.clone()));
            }
        })
        .detach();
    }

    pub(crate) fn take_tab(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<RightPanel>> {
        let index = self.tabs.iter().position(|tab| tab.id == id)?;
        let mut tab = self.tabs.remove(index);
        if self.active == Some(id) {
            tab.draft = self.address.read(cx).value().to_string();
        }
        let panel = tab.panel.unwrap_or_else(|| {
            cx.new(|cx| {
                let mut panel = RightPanel::new(self.root.clone(), window, cx);
                if let Some(context) = self.side_context.clone() {
                    panel.set_side_context(context);
                }
                if let Some(browser) = tab.browser {
                    panel.adopt_browser(browser, self.profile.clone());
                } else {
                    panel.blank_workspace(tab.draft, window, cx);
                }
                panel
            })
        });
        self.drag = None;
        if self.active == Some(id) {
            self.active = None;
            self.select(
                index
                    .checked_sub(1)
                    .and_then(|index| self.tabs.get(index))
                    .map(|tab| tab.id),
                window,
                cx,
            );
        }
        cx.notify();
        Some(panel)
    }

    pub(crate) fn accept_tab(
        &mut self,
        panel: Entity<RightPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.motion.settle();
        panel.update(cx, |panel, _| panel.set_tab_zone(false));
        self.watch_panel(&panel, cx);
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(ToolTab {
            id,
            kind: Kind::from_pane(panel.read(cx).current_pane()),
            panel: Some(panel),
            browser: None,
            draft: String::new(),
        });
        self.incoming = Default::default();
        self.select(Some(id), window, cx);
    }

    fn insertion_index(&self, pointer: Pixels) -> usize {
        let mut x = del_core::zoom::unscaled((pointer - self.strip_bounds.get().left()).as_f32());
        if let Some(index) = self.incoming.index {
            let left = index as f32 * (TAB_WIDTH + TAB_GAP);
            let width = self.incoming.width + TAB_GAP;
            if x > left + width {
                x -= width;
            } else if x >= left {
                x = left;
            }
        }
        ((x / (TAB_WIDTH + TAB_GAP)).round().max(0.) as usize).min(self.tabs.len())
    }

    pub(crate) fn render_row(&mut self, row: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        let row = self
            .transfer_target(row, cx)
            .track_scroll(&self.row_scroll)
            .w_full()
            .min_w_0();
        h_flex()
            .relative()
            .w_full()
            .min_w_0()
            .flex_none()
            .bg(crate::appearance::shell_surface())
            .overflow_hidden()
            .child(row)
            .children(crate::tab_transfer::edge_fades(
                &self.row_scroll,
                crate::appearance::shell_surface(),
            ))
            .into_any_element()
    }

    pub(crate) fn transfer_target(
        &mut self,
        row: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if !cx.has_active_drag() {
            self.incoming.clear();
        }
        let bounds = self.transfer_bounds.clone();
        row.relative()
            .child(
                canvas(
                    move |rect, _, _| {
                        bounds.set(rect);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<crate::right_panel::DraggedSidebarTab>, _, cx| {
                    let drag = event.drag(cx).clone();
                    if this
                        .transfer_peer
                        .as_ref()
                        .is_some_and(|peer| peer.entity_id() == drag.source.entity_id())
                    {
                        let inside = drag.placement.visit(
                            cx.entity_id(),
                            event.event.position,
                            event.bounds,
                        );
                        if inside {
                            let index = this.insertion_index(event.event.position.x);
                            this.incoming.update(Some(index), TAB_WIDTH);
                            let left = this.strip_bounds.get().left()
                                + zpx(index as f32 * (TAB_WIDTH + TAB_GAP));
                            drag.placement.slot(
                                Bounds::new(
                                    point(left, event.bounds.top()),
                                    size(zpx(TAB_WIDTH), event.bounds.size.height),
                                ),
                                event.bounds,
                            );
                            let mut offset = this.row_scroll.offset();
                            if event.event.position.x > event.bounds.right() - zpx(24.) {
                                offset.x =
                                    (offset.x - zpx(10.)).max(-this.row_scroll.max_offset().x);
                            } else if event.event.position.x
                                < event.bounds.left() + zpx(TAB_WIDTH + 24.)
                            {
                                offset.x = (offset.x + zpx(10.)).min(Pixels::ZERO);
                            }
                            this.row_scroll.set_offset(offset);
                        } else {
                            this.incoming.clear();
                        }
                        cx.notify();
                    }
                },
            ))
            .on_drop(cx.listener(
                |this, drag: &crate::right_panel::DraggedSidebarTab, window, cx| {
                    if this
                        .transfer_peer
                        .as_ref()
                        .is_some_and(|peer| peer.entity_id() == drag.source.entity_id())
                        && drag.placement.visit(
                            cx.entity_id(),
                            window.mouse_position(),
                            if this.row_scroll.bounds().size.width > Pixels::ZERO {
                                this.row_scroll.bounds()
                            } else {
                                this.transfer_bounds.get()
                            },
                        )
                        && let Ok(Some(panel)) = drag
                            .source
                            .update(cx, |source, cx| source.take_tab(&drag.key, window, cx))
                    {
                        let index = this
                            .incoming
                            .index
                            .unwrap_or_else(|| this.insertion_index(window.mouse_position().x));
                        this.accept_tab(panel, window, cx);
                        let tab = this.tabs.pop().unwrap();
                        this.tabs.insert(index.min(this.tabs.len()), tab);
                    }
                },
            ))
    }

    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter a URL"));
        cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.navigate(window, cx);
            } else if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| Some(tab.id) == this.active) {
                    tab.draft = this.address.read(cx).value().to_string();
                }
                cx.notify();
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.persist(cx);
            async {}
        })
        .detach();
        Self {
            transfer_peer: None,
            incoming: Default::default(),
            row_scroll: ScrollHandle::new(),
            motion: Default::default(),
            reveal_active: Default::default(),
            watched_panels: Default::default(),
            transfer_bounds: Default::default(),
            side_context: None,
            storage_key: None,
            drag: None,
            drag_origin: Default::default(),
            drag_outside: false,
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
            draft: String::new(),
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
        if let Some(tab) = self.tabs.iter_mut().find(|tab| Some(tab.id) == self.active) {
            tab.draft = self.address.read(cx).value().to_string();
        }
        self.active = id;
        self.reveal_active.set(true);
        self.set_visible(self.visible, cx);
        let draft = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == id)
            .map(|tab| tab.draft.clone())
            .unwrap_or_default();
        self.address
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        if let Some(panel) = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == id)
            .and_then(|tab| tab.panel.clone())
        {
            panel.update(cx, |panel, cx| panel.prepare_to_show(window, cx));
        }
        self.focus(window, cx);
        cx.notify();
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|tab| Some(tab.id) == self.active) {
            if let Some(panel) = &tab.panel {
                panel.update(cx, |panel, cx| panel.focus_workspace(window, cx));
                return;
            }
            if tab.kind != Kind::Browser {
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
        // Event forwarding checks current ownership after a cross-zone move.
        let tab_id = tab.id;
        self.watch_panel(&panel, cx);
        let tab = self.tabs.iter_mut().find(|tab| tab.id == tab_id).unwrap();
        tab.kind = kind;
        tab.panel = Some(panel);
        self.focus(window, cx);
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
        self.drag_outside = false;
        self.drag = Some(TabDrag {
            id,
            source,
            target: source,
        });
        cx.notify();
    }

    fn track_drag_position(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = if self.row_scroll.bounds().size.width > Pixels::ZERO {
            self.row_scroll.bounds()
        } else {
            self.transfer_bounds.get()
        };
        let outside = !bounds.contains(&pointer);
        if self.drag_outside != outside {
            self.drag_outside = outside;
            cx.notify();
        }
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
            drag.target = target;

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
        for tab in &mut self.tabs {
            if let Some(panel) = &tab.panel {
                tab.kind = Kind::from_pane(panel.read(cx).current_pane());
            }
        }
        if self.drag.is_some() && !cx.has_active_drag() {
            self.drag = None;
        }
        if !cx.has_active_drag() {
            self.incoming.clear();
        }
        let incoming = self.incoming;
        let bounds = self.strip_bounds.clone();
        let drag = self.drag;
        let mut indices: Vec<_> = (0..self.tabs.len()).collect();
        if let Some(drag) = drag {
            indices.retain(|index| self.tabs[*index].id != drag.id);
            if !self.drag_outside {
                indices.insert(drag.target.min(indices.len()), drag.source);
            }
        }
        let was_transitioning = self.motion.transitioning();
        let motion = self.motion.sync(
            indices
                .iter()
                .map(|index| {
                    let tab = &self.tabs[*index];
                    (
                        tab.id,
                        crate::tab_motion::Visual {
                            label: tab.kind.label().into(),
                            icon: tab.kind.icon(),
                            active: self.active == Some(tab.id),
                        },
                    )
                })
                .collect(),
            cx,
        );
        if was_transitioning || self.motion.transitioning() {
            self.reveal_active.set(true);
        }
        let reveal = self.reveal_active.clone();
        let scroll = self.row_scroll.clone();
        let active_index = self.tabs.iter().position(|tab| Some(tab.id) == self.active);
        let row = h_flex()
            .id("workspace-tools-strip")
            .gap(zpx(TAB_GAP))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedWorkspaceTab>, _, cx| {
                    let drag = event.drag(cx).clone();
                    this.track_drag_position(event.event.position, cx);
                    if !this.drag_outside {
                        let delta = del_core::zoom::unscaled(
                            (event.event.position.x - drag.origin.get().x).as_f32(),
                        );
                        this.move_drag(drag.owner, drag.id, delta, cx);
                    }
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
            .children(motion.into_iter().enumerate().flat_map(|(index, slot)| {
                if slot.closing {
                    return vec![slot.ghost(format!("workspace-close-{}", slot.key).into())];
                }
                let tab = self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == slot.key)
                    .expect("live tab");
                let id = tab.id;
                let active = self.active == Some(id);
                let unread = tab
                    .panel
                    .as_ref()
                    .is_some_and(|panel| panel.read(cx).is_unread(cx));
                let working = tab
                    .panel
                    .as_ref()
                    .is_some_and(|panel| panel.read(cx).is_working(cx));
                let group: SharedString = format!("workspace-tab-{id}").into();
                let label = tab
                    .browser
                    .as_ref()
                    .and_then(|b| url::Url::parse(&b.url()).ok())
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_else(|| tab.kind.label().into());
                let payload = DraggedWorkspaceTab {
                    source: cx.entity().downgrade(),
                    placement: Default::default(),
                    owner: cx.entity_id(),
                    id,
                    label: label.clone(),
                    kind: tab.kind,
                    origin: self.drag_origin.clone(),
                    cursor_offset: Point::default(),
                    grab_offset: Point::default(),
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
                        origin.set(event.position);

                        cx.stop_propagation();
                    })
                    .on_drag(payload, move |drag, offset, window, cx| {
                        let movement = window.mouse_position() - drag.origin.get();
                        workspace.update(cx, |this, cx| {
                            this.begin_drag(drag.id, cx);
                            this.track_drag_position(window.mouse_position(), cx);
                            if !this.drag_outside {
                                this.move_drag(
                                    drag.owner,
                                    drag.id,
                                    del_core::zoom::unscaled(movement.x.as_f32()),
                                    cx,
                                );
                            }
                        });
                        let mut preview = drag.clone();
                        preview.grab_offset = offset - movement;
                        preview.cursor_offset = offset;
                        cx.new(|_| preview)
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(Some(id), window, cx)),
                    )
                    .child(crate::icons::icon(tab.kind.icon()).with_size(zpx(15.)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(working, |tab| tab.child(crate::icons::working_spinner()))
                    .when(unread && !working, |tab| {
                        tab.child(crate::icons::unread_mark())
                    })
                    .when(hints && index < 8, |t| {
                        t.child(div().text_size(zpx(10.)).child(format!("⌘{}", index + 2)))
                    })
                    .child(
                        div()
                            .id(("close-workspace-tab", id))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Close {} tab", tab.kind.label()))
                            .size(zpx(crate::icons::TAB_CLOSE_SIZE))
                            .flex_none()
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
                            .child(
                                crate::icons::icon(IconName::Close)
                                    .with_size(zpx(crate::icons::TAB_CLOSE_GLYPH)),
                            ),
                    );
                let mut elements = Vec::new();
                if incoming.index == Some(index) {
                    elements.push(incoming.element("main-insertion-space"));
                }
                elements.push(
                    slot.wrap(
                        format!("workspace-open-{id}").into(),
                        div()
                            .flex_none()
                            .when(drag.is_some_and(|d| d.id == id), |tab| tab.opacity(0.))
                            .child(element),
                    ),
                );
                elements
            }))
            .when(incoming.index == Some(self.tabs.len()), |row| {
                row.child(incoming.element("main-insertion-space"))
            });
        h_flex()
            .flex_none()
            .on_children_prepainted(move |children, window, _| {
                if let Some(bound) = children.first() {
                    bounds.set(*bound);
                    let viewport = scroll.bounds();
                    if drag.is_none() && reveal.get() && viewport.size.width > Pixels::ZERO {
                        let (left, right) = if let Some(index) = active_index {
                            let left = bound.left() + zpx(index as f32 * (TAB_WIDTH + TAB_GAP));
                            (left, left + zpx(TAB_WIDTH))
                        } else {
                            (viewport.left() + scroll.offset().x, viewport.left())
                        };
                        let delta = if right > viewport.right() {
                            viewport.right() - right
                        } else if left < viewport.left() {
                            viewport.left() - left
                        } else {
                            Pixels::ZERO
                        };
                        if delta != Pixels::ZERO {
                            let offset = scroll.offset();
                            scroll
                                .set_offset(point((offset.x + delta).min(Pixels::ZERO), offset.y));
                            window.request_animation_frame();
                        } else {
                            reveal.set(false);
                        }
                    }
                }
            })
            .child(row)
            .into_any_element()
    }
    pub(crate) fn open_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .tabs
            .iter()
            .any(|tab| self.active == Some(tab.id) && tab.kind == Kind::Blank)
        {
            self.new_tab(window, cx);
        }
        self.choose_tool(pane, window, cx);
        self.focus(window, cx);
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
        if let Some(panel) = tab.and_then(|tab| tab.panel.clone()) {
            return div().size_full().child(panel).into_any_element();
        }
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(crate::new_tab::address_bar(&self.address))
            .child(crate::new_tab::tools(
                false,
                &self.scroll,
                Self::choose_tool,
                cx,
            ))
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
    fn rapid_new_tabs_reveal_the_active_tab(cx: &mut gpui::TestAppContext) {
        struct Fixture(Entity<WorkspaceTabs>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let row = self.0.update(cx, |workspace, cx| {
                    let strip = workspace.render_strip(false, cx);
                    let row = h_flex()
                        .id("reveal-row")
                        .overflow_x_scroll()
                        .h(zpx(crate::icons::TOOL_ROW_HEIGHT))
                        .child(div().w(zpx(TAB_WIDTH)).flex_none().child("Chat"))
                        .child(strip);
                    workspace.render_row(row, cx)
                });
                div().w(zpx(450.)).child(row)
            }
        }
        cx.update(gpui_kit::init);
        let mut handle = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| WorkspaceTabs::new(std::env::temp_dir(), window, cx));
            cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
            handle = Some(workspace.clone());
            Fixture(workspace)
        });
        let workspace = handle.unwrap();
        for id in 1..=12 {
            cx.update(|window, cx| workspace.update(cx, |w, cx| w.new_tab(window, cx)));
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
            let selector = Box::leak(format!("workspace-tool-{id}").into_boxed_str());
            let tab = cx.debug_bounds(selector).unwrap();
            let viewport = workspace.read_with(cx, |w, _| w.row_scroll.bounds());
            assert!(
                tab.right() <= viewport.right() + zpx(1.),
                "new tab {id} is offscreen"
            );
            assert!(tab.left() >= viewport.left() - zpx(1.));
        }
    }

    #[::gpui::test]
    fn drag_between_zones_moves_one_live_tab_and_preserves_drafts(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            workspace: Entity<WorkspaceTabs>,
            right: Entity<RightPanel>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let strip = self
                    .workspace
                    .update(cx, |workspace, cx| workspace.render_strip(false, cx));
                let row = h_flex()
                    .id("main-transfer-zone")
                    .debug_selector(|| "main-transfer-zone".into())
                    .w(zpx(600.))
                    .h(zpx(crate::icons::TOOL_ROW_HEIGHT))
                    .child(
                        div()
                            .id("fixed-chat")
                            .debug_selector(|| "fixed-chat".into())
                            .w(zpx(TAB_WIDTH))
                            .flex_none()
                            .child("Original chat"),
                    )
                    .child(strip)
                    .child(
                        div()
                            .id("main-plus")
                            .debug_selector(|| "main-plus".into())
                            .w(zpx(30.))
                            .h(zpx(40.))
                            .flex_none()
                            .child("+"),
                    );
                let row = self
                    .workspace
                    .update(cx, |workspace, cx| workspace.transfer_target(row, cx));
                h_flex()
                    .items_start()
                    .w(zpx(1250.))
                    .h(zpx(600.))
                    .child(
                        v_flex()
                            .w(zpx(600.))
                            .flex_none()
                            .child(row)
                            .child(self.workspace.clone()),
                    )
                    .child(
                        div()
                            .w(zpx(650.))
                            .h_full()
                            .flex_none()
                            .child(self.right.clone()),
                    )
            }
        }
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let mut handles = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let right = cx.new(|cx| RightPanel::new(dir.path().into(), window, cx));
            let workspace = cx.new(|cx| {
                let mut workspace = WorkspaceTabs::new(dir.path().into(), window, cx);
                workspace.transfer_peer = Some(right.downgrade());
                workspace.new_tab(window, cx);
                workspace.address.update(cx, |input, cx| {
                    input.set_value("transfer-draft.example", window, cx)
                });
                workspace
            });
            right.update(cx, |right, _| {
                right.transfer_peer = Some(workspace.downgrade())
            });
            cx.observe(&right, |_, _, cx| cx.notify()).detach();
            cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
            handles = Some((workspace.clone(), right.clone()));
            Fixture { workspace, right }
        });
        let (workspace, right) = handles.unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let fixed = cx.debug_bounds("fixed-chat").unwrap();
        let plus_before = cx.debug_bounds("main-plus").unwrap();
        let source_bounds = cx.debug_bounds("workspace-tool-1").unwrap();
        let source = source_bounds.center();
        let target = cx.debug_bounds("sidebar-transfer-zone").unwrap().center();
        // One fast move crosses the entire separator before GPUI's first drag event.
        cx.simulate_mouse_move(source, None, Modifiers::default());
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let plus_during_drag = cx.debug_bounds("main-plus").unwrap();
        assert!(
            (plus_before.left() - plus_during_drag.left() - zpx(TAB_WIDTH)).abs() < zpx(3.),
            "the source plus must follow the remaining tabs before drop"
        );
        cx.simulate_mouse_move(source, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("main-plus").unwrap().left(),
            plus_before.left()
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.tabs.is_empty());
            assert!(workspace.is_chat());
        });
        let saved = right.read_with(cx, |right, cx| {
            serde_json::to_value(right.snapshot(true, cx)).unwrap()
        });
        let hosted = saved["hosted"].as_array().unwrap();
        assert_eq!(hosted.len(), 1);
        assert_eq!(
            hosted[0]["panel"]["blank_tabs"][0]["address"],
            "transfer-draft.example"
        );
        let selector =
            Box::leak(format!("tab-hosted-{}", hosted[0]["id"].as_str().unwrap()).into_boxed_str());
        let moved_bounds = cx.debug_bounds(selector).unwrap();
        assert_eq!(moved_bounds.size.width, source_bounds.size.width);
        assert_eq!(moved_bounds.size.width, zpx(crate::icons::TOOL_TAB_WIDTH));
        let source = moved_bounds.center();
        let sidebar_plus_before = cx.debug_bounds("new-sidebar-tab").unwrap();
        let target = cx.debug_bounds("main-transfer-zone").unwrap().center();
        cx.simulate_mouse_move(source, None, Modifiers::default());
        cx.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            source - point(zpx(12.), zpx(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let sidebar_plus_during = cx.debug_bounds("new-sidebar-tab").unwrap();
        assert!(
            (sidebar_plus_before.left() - sidebar_plus_during.left() - zpx(TAB_WIDTH + TAB_GAP))
                .abs()
                < zpx(3.)
        );
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let fixed_after = cx.debug_bounds("fixed-chat").unwrap();
        assert_eq!(fixed_after.left(), fixed.left());
        assert_eq!(fixed_after.size.width, fixed.size.width);
        let imported = workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs.len(), 1);
            assert_eq!(workspace.active, Some(workspace.tabs[0].id));
            workspace.tabs[0].panel.clone().unwrap()
        });
        let saved = imported.read_with(cx, |panel, cx| {
            serde_json::to_value(panel.snapshot(false, cx)).unwrap()
        });
        assert_eq!(saved["blank_tabs"][0]["address"], "transfer-draft.example");
        right.read_with(cx, |right, cx| {
            assert!(
                serde_json::to_value(right.snapshot(true, cx)).unwrap()["hosted"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            )
        });
        // Save/restore records the moved tab in only its destination zone.
        cx.update(|window, cx| {
            let saved = workspace.read(cx).snapshot(cx);
            let restored = cx.new(|cx| {
                let mut restored = WorkspaceTabs::new(dir.path().into(), window, cx);
                restored.restore_tabs(
                    serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap(),
                    window,
                    cx,
                );
                restored
            });
            assert_eq!(restored.read(cx).tabs.len(), 1);
            assert!(restored.read(cx).tabs[0].panel.is_some());
        });
        // Before release, the destination opens a real slot and moves its
        // neighbors. Moving within the row changes that slot immediately.
        cx.update(|window, cx| {
            cx.set_reduce_motion(true);
            workspace.update(cx, |workspace, cx| {
                workspace.new_tab(window, cx);
                workspace.new_tab(window, cx);
            });
        });
        cx.run_until_parked();
        let before = cx.debug_bounds("workspace-tool-3").unwrap();
        let from = cx.debug_bounds("tab-files").unwrap().center();
        let to = point(fixed.right() + zpx((TAB_WIDTH + TAB_GAP) * 1.4), from.y);
        cx.simulate_mouse_move(from, None, Modifiers::default());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            from - point(zpx(12.), zpx(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("main-insertion-space").is_some());
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.incoming.index, Some(1))
        });
        let shifted = cx.debug_bounds("workspace-tool-3").unwrap();
        assert!((shifted.left() - before.left() - zpx(TAB_WIDTH + TAB_GAP)).abs() < zpx(1.));
        right.read_with(cx, |right, cx| {
            let saved = serde_json::to_value(right.snapshot(true, cx)).unwrap();
            assert!(
                saved["open_panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|pane| pane == "Files"),
                "the source remains owned until drop, even when its layout slot collapses"
            );
        });
        let to = point(zpx(190.), from.y);
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.incoming.index, Some(0))
        });
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.tabs[0].kind, Kind::Files)
        });
        assert_eq!(cx.debug_bounds("fixed-chat").unwrap().left(), fixed.left());
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
            self.0.update(cx, |w, cx| {
                let strip = w.render_strip(false, cx);
                let row = h_flex()
                    .id("drag-fixture-row")
                    .h(zpx(crate::icons::TOOL_ROW_HEIGHT))
                    .overflow_x_scroll()
                    .child(
                        div()
                            .id("original-chat")
                            .debug_selector(|| "original-chat".into())
                            .w(zpx(TAB_WIDTH))
                            .flex_none()
                            .h(zpx(crate::icons::TOOL_TAB_HEIGHT))
                            .child("Original chat"),
                    )
                    .child(strip);
                w.render_row(row, cx)
            })
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
        // Choose the last position while still inside the row, then leave it.
        cx.simulate_mouse_move(
            point(moved.x + zpx(2. * (TAB_WIDTH + TAB_GAP)), moved.y),
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
