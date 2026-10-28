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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Blank,
    Files,
    Changes,
    Terminal,
    Browser,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Blank => "New tab",
            Self::Files => "Files",
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Blank => IconName::Plus.into(),
            Self::Files => IconName::Folder.into(),
            Self::Changes => IconName::File.into(),
            Self::Terminal => Icon::default().path("icons/terminal.svg"),
            Self::Browser => IconName::Globe.into(),
        }
    }
}
struct ToolTab {
    id: u64,
    kind: Kind,
    panel: Option<Entity<RightPanel>>,
    browser: Option<crate::browser::Browser>,
}

pub struct WorkspaceTabs {
    root: PathBuf,
    tabs: Vec<ToolTab>,
    active: Option<u64>,
    next_id: u64,
    visible: bool,
    address: Entity<InputState>,
    profile: String,
    scroll: ScrollHandle,
}
impl WorkspaceTabs {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter a URL"));
        cx.subscribe_in(&address, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.navigate(window, cx);
            }
        })
        .detach();
        Self {
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
            if tab.kind == Kind::Terminal {
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
        let tab = self.tabs.remove(index);
        if let Some(panel) = tab.panel {
            panel.update(cx, |p, cx| p.release(cx));
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
            _ => return,
        };
        let panel = cx.new(|cx| {
            let mut panel = RightPanel::new(self.root.clone(), window, cx);
            panel.show_workspace(pane, window, cx);
            panel
        });
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
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
    pub fn render_strip(&self, hints: bool, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_0p5()
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
                h_flex()
                    .id(group.clone())
                    .group(group.clone())
                    .role(gpui::Role::Button)
                    .aria_label(format!("Open {label} tab"))
                    .h(zpx(34.))
                    .w(zpx(160.))
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
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(Some(id), window, cx)),
                    )
                    .when(tab.kind != Kind::Blank, |tab_element| {
                        tab_element.child(crate::icons::icon(tab.kind.icon()).with_size(zpx(15.)))
                    })
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(id, window, cx);
                            }))
                            .child(crate::icons::icon(IconName::Close).with_size(zpx(13.))),
                    )
            }))
            .into_any_element()
    }
    fn choose_tool(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match pane {
            Pane::Files => Kind::Files,
            Pane::Changes => Kind::Changes,
            Pane::Terminal => Kind::Terminal,
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
            .child(crate::new_tab::address_bar(&self.address))
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
