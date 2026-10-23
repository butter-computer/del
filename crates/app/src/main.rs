//! The `del` application shell: a custom top bar that hosts the macOS traffic
//! lights, over a rounded black page inset from the window edges. The window
//! itself is translucent and blurred, so the desktop shows through the shell.
//!
//! Layout metrics and the shell colours live in `del_core`; this crate renders
//! them.

use crate::zoom::px as zpx;
// The wallpaper background is switched off for now; the loader stays so it can
// be wired back in without redoing the preload and decode work.
#[allow(dead_code)]
mod wallpaper;

mod appearance;
mod assets;
mod attachments;
mod browser;
mod chat;
mod composer_ui;
mod conversation_search;
mod dictation;
mod document_preview;
mod followup_ui;
mod followups;
mod hover_motion;
#[allow(dead_code)]
mod mcp_bridge;
mod mode_ui;
mod model_picker;
mod provider_updates;
mod providers;
mod readiness;
mod right_panel;
mod search_ui;
mod sessions;
mod sidebar;
mod signin;
mod state;
mod terminal_conversation;
#[allow(dead_code)]
mod terminal_host;
mod terminal_store;
mod voice_ui;
mod zoom;

use crate::appearance::{page_surface, rgb, rgba, shell_surface};
use chat::ChatView;
use composer_ui::ComposerCard;
use del_agent::{AgentBackend, MockBackend, Provider, SessionId};
use del_core::ShellMetrics;
use del_core::typography::{TEXT_LG, TEXT_MD};
use del_mcp::SessionMode;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mcp_bridge::McpBridge;
use mode_ui::PendingQuestion;
use right_panel::RightPanel;
use state::{AppState, Panel, Selection, Tab};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use terminal_host::TerminalHost;
use terminal_store::TerminalStore;

/// Fallback when display geometry is unavailable.
const WINDOW_SIZE: gpui_kit::Size<Pixels> = size(px(1120.0), px(760.0));

/// The sidebar is reserved for something else for now: its code stays, but it is
/// neither shown nor toggleable (button and Cmd-B) until this is turned on.
const SIDEBAR_ENABLED: bool = true;

/// Every tab is this wide once settled; it animates to and from 0.
const TAB_WIDTH: f32 = 160.0;

/// Space after each tab, animated together with it.
const TAB_GAP: f32 = 2.0;

/// Stable tab identity also survives a draft becoming a live session mid-drag.
#[derive(Clone)]
struct DraggedTab {
    uid: u64,
    title: String,
    provider: Option<Provider>,
    start_pointer_y: Pixels,
    cursor_offset_x: Pixels,
    strip_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    start_pointer_x: std::rc::Rc<std::cell::Cell<Pixels>>,
}

impl Render for DraggedTab {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.strip_bounds.get();
        let width = zpx(TAB_WIDTH).min(bounds.size.width.max(zpx(0.)));
        let natural_left = window.mouse_position().x - self.cursor_offset_x;
        let left = natural_left.clamp(bounds.left(), (bounds.right() - width).max(bounds.left()));
        // GPUI places drag previews at the pointer. Cancel its vertical movement
        // inside a fixed-size wrapper, keeping the original grab offset intact.
        div().w(zpx(TAB_WIDTH)).h(zpx(24.)).child(
            h_flex()
                .relative()
                .top(self.start_pointer_y - window.mouse_position().y)
                .left(left - natural_left)
                .w(width)
                .overflow_hidden()
                .h(zpx(24.))
                .px_2()
                .gap_2()
                .items_center()
                .rounded_md()
                .bg(rgb(0x303033))
                .text_color(rgb(0xf5f5f5))
                .text_size(zpx(TEXT_MD))
                .shadow_md()
                .child(mode_ui::tab_mark(self.provider))
                .child(tab_title(self.title.clone())),
        )
    }
}

/// The title inside a tab.
fn tab_title(title: String) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(zpx(16.0))
        .line_height(zpx(16.0))
        .truncate()
        .child(title)
}

/// Shared timing for the left and right sidebar width transitions.
const PANEL_ANIM: Duration = Duration::from_millis(220);

/// The prompt above the composer.
const HEADING: &str = "What are we building?";

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = chat::COLUMN_MAX_WIDTH;

actions!(
    del,
    [
        NewSession,
        NextSession,
        PrevSession,
        FocusComposer,
        SearchConversations,
        StopSession,
        ToggleProviders,
        ToggleSidebar,
        Quit,
        ToggleFiles,
        ToggleTerminal,
        CloseTab,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9,
    ]
);

/// Backends for the session manager.
///
/// `DEL_BACKEND=mock` selects the scripted mock for both providers; otherwise
/// the real Codex and Claude backends drive the user's installed CLIs.
pub fn backends() -> Vec<Arc<dyn AgentBackend>> {
    if std::env::var("DEL_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("mock")) {
        return Provider::ALL
            .iter()
            .map(|&p| Arc::new(MockBackend::new(p)) as Arc<dyn AgentBackend>)
            .collect();
    }
    vec![
        Arc::new(del_codex::CodexBackend::new(Default::default())),
        Arc::new(del_acp::AcpBackend::default()),
        // Keep permission selection explicit in each session's launch arguments.
        Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
            setting_sources: Some("project".into()),
            ..Default::default()
        })),
    ]
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SidebarWidths {
    left: f32,
    right: f32,
}

impl Default for SidebarWidths {
    fn default() -> Self {
        Self {
            left: sidebar::SIDEBAR_WIDTH,
            right: right_panel::PANEL_WIDTH,
        }
    }
}

impl SidebarWidths {
    fn path() -> std::path::PathBuf {
        del_agent::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("sidebar-widths.json")
    }

    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|sizes| sizes.left.is_finite() && sizes.right.is_finite())
            .unwrap_or_default()
    }

    fn save(self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[derive(Clone)]
struct SidebarResize {
    right: bool,
    start_x: std::rc::Rc<std::cell::Cell<f32>>,
    width: f32,
    other: f32,
}

struct ResizePreview;
impl Render for ResizePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_0()
    }
}

fn sidebar_width(requested: f32, right: bool, viewport: f32, other: f32) -> f32 {
    let min = if right { 220.0 } else { 150.0 };
    let max = if right { 800.0_f32 } else { 360.0_f32 };
    requested.clamp(min, max.min(viewport - other - 260.0 - 20.0).max(min))
}

struct AppShell {
    shell_focus: FocusHandle,
    settings_focused: bool,
    zoom_percent: u16,
    state: Entity<AppState>,
    composer: Entity<TextareaState>,
    dictation: Option<dictation::Dictation>,
    dictation_generation: u64,
    model_picker: Entity<model_picker::ModelPicker>,
    rename: Entity<InputState>,
    /// The session whose title is loaded into the rename field.
    renaming_loaded: Option<SessionId>,
    sidebar_widths: SidebarWidths,
    resize_origin: std::rc::Rc<std::cell::Cell<f32>>,
    tab_drag_origin: std::rc::Rc<std::cell::Cell<Pixels>>,
    tab_strip_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    sidebar_open: bool,
    sidebar_visible: bool,
    sidebar_toggles: u64,
    tab_scroll: ScrollHandle,
    tab_reveal: Option<(u64, std::time::Instant)>,
    right: Entity<RightPanel>,
    right_owner: u64,
    right_key: Option<String>,
    right_cache: HashMap<u64, (Entity<RightPanel>, bool)>,
    right_store: right_panel::PanelStore,
    right_store_path: std::path::PathBuf,
    right_save_generation: u64,
    /// Where the right panel is heading (open or closed).
    right_open: bool,
    /// Whether it is on screen: stays true while the close animation plays.
    right_visible: bool,
    /// Bumped on every toggle so each one starts a fresh animation.
    right_toggles: u64,
    right_animate: bool,
    /// One chat view per session opened in this run.
    chats: HashMap<SessionId, Entity<ChatView>>,
    search_index: Arc<std::sync::Mutex<conversation_search::Index>>,
    search_open: bool,
    terminal_search_generation: u64,
    composer_owner: u64,
    composer_drafts: HashMap<u64, String>,
    composer_attachments: HashMap<u64, Vec<del_agent::Attachment>>,
    attachment_scroll: ScrollHandle,
    queue_hovered: Option<String>,
    metrics: ShellMetrics,
    terminals: HashMap<SessionId, Entity<TerminalHost>>,
    terminal_store: TerminalStore,
    terminal_attempts: HashSet<SessionId>,
    terminal_opening: HashSet<SessionId>,
    terminal_identity_pending: HashSet<SessionId>,
    terminal_update_restarts: HashSet<SessionId>,
    terminal_update_restarted: HashSet<(SessionId, u32)>,
    mcp: Option<McpBridge>,
    questions: VecDeque<PendingQuestion>,
    mcp_answer: Entity<InputState>,
    notices: VecDeque<(SessionId, String, String)>,
    reported: HashMap<SessionId, String>,
    mode_focus: Option<(Selection, SessionMode)>,
    mode_slide_from: f32,
    mode_slide_started: Option<std::time::Instant>,
    caffeinate: Option<std::process::Child>,
    dragging_tab: Option<u64>,
    drag_target: Option<usize>,
    previous_drag_target: Option<usize>,
}

impl AppShell {
    fn resize_handle(&self, right: bool, width: f32, other: f32) -> Stateful<Div> {
        let drag = SidebarResize {
            right,
            start_x: self.resize_origin.clone(),
            width,
            other,
        };
        let start_x = drag.start_x.clone();
        div()
            .id(if right {
                "resize-right-sidebar"
            } else {
                "resize-left-sidebar"
            })
            .absolute()
            .top_0()
            .bottom_0()
            .w(zpx(5.0))
            .cursor_col_resize()
            .on_mouse_down(MouseButton::Left, move |event, _, _| {
                start_x.set(event.position.x.as_f32())
            })
            .on_drag(drag, |_, _, _, cx| cx.new(|_| ResizePreview))
    }

    fn sync_caffeine(&mut self, cx: &mut Context<Self>) {
        let enabled = self.state.read(cx).caffeine_enabled;
        if enabled == self.caffeinate.is_some() {
            return;
        }
        if enabled {
            match std::process::Command::new("/usr/bin/caffeinate")
                .args(["-di", "-w", &std::process::id().to_string()])
                .spawn()
            {
                Ok(child) => self.caffeinate = Some(child),
                Err(error) => self.state.update(cx, |state, cx| {
                    state.caffeine_enabled = false;
                    state.error = Some(format!("Could not keep the Mac awake: {error}"));
                    cx.notify();
                }),
            }
        } else if let Some(mut child) = self.caffeinate.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn begin_tab_drag(&mut self, uid: u64, cx: &mut Context<Self>) {
        let index = {
            let state = self.state.read(cx);
            state
                .tabs
                .iter()
                .position(|tab| state.uid_of(tab) == uid)
                .unwrap_or(0)
        };
        self.dragging_tab = Some(uid);
        self.drag_target = Some(index);
        self.previous_drag_target = Some(index);
        cx.notify();
    }

    fn move_tab_drag(&mut self, target: usize, cx: &mut Context<Self>) {
        if self.drag_target == Some(target) {
            return;
        }
        self.previous_drag_target = self.drag_target;
        self.drag_target = Some(target);
        cx.notify();
    }

    fn finish_tab_drag(&mut self, dragged: u64, cx: &mut Context<Self>) {
        if let Some(target) = self.drag_target {
            let target_uid = {
                let state = self.state.read(cx);
                state.tabs.get(target).map(|tab| state.uid_of(tab))
            };
            if let Some(target_uid) = target_uid {
                self.state
                    .update(cx, |state, cx| state.reorder_tab(dragged, target_uid, cx));
            }
        }
        self.dragging_tab = None;
        self.drag_target = None;
        self.previous_drag_target = None;
        cx.notify();
    }

    fn sync_terminal_sidebar_actions(&mut self, cx: &mut Context<Self>) {
        let (removed, pins) = self.state.update(cx, |state, _| {
            (
                std::mem::take(&mut state.removed_terminals),
                std::mem::take(&mut state.terminal_pin_changes),
            )
        });
        if removed.is_empty() && pins.is_empty() {
            return;
        }
        for (id, pinned) in pins {
            self.terminal_store.set_pinned(&id, pinned);
        }
        for id in removed {
            if let Some(terminal) = self.terminals.remove(&id) {
                terminal.update(cx, |terminal, cx| terminal.close(cx));
            }
            if let Some(bridge) = &self.mcp {
                bridge.handle().revoke(&id);
                bridge.handle().remove_terminal(&id);
            }
            self.terminal_store.remove(&id);
        }
        if let Err(error) = self.terminal_store.save(&TerminalStore::path()) {
            self.state.update(cx, |state, cx| {
                state.error = Some(format!("Could not save terminal history: {error}"));
                cx.notify();
            });
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask anything...")
                .auto_grow(3, 8)
                .submit_on_enter(true);
            state.set_submit_on_enter(true, cx);
            state
        });
        // Enter (without Shift) submits; Shift+Enter inserts a newline.
        cx.subscribe_in(
            &composer,
            window,
            |this, _, event, window, cx| match event {
                InputEvent::PressEnter { secondary, shift } if !shift => {
                    this.submit_with_mode(*secondary || window.modifiers().platform, window, cx)
                }
                InputEvent::Change => {
                    if this.dictation.as_ref().is_some_and(|voice| {
                        voice.draft.displayed != this.composer.read(cx).value().as_ref()
                    }) {
                        this.cancel_dictation(cx);
                    }
                    cx.notify();
                }
                InputEvent::Focus if this.dictation.is_some() => {
                    this.settle_dictation(window, cx);
                    this.cancel_dictation(cx);
                }
                _ => {}
            },
        )
        .detach();
        composer.update(cx, |state, cx| state.focus(window, cx));

        let state = cx.new(AppState::new);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();

        // Kill every child process when the app quits.
        let quit_state = state.clone();
        cx.on_app_quit(move |_, cx| {
            quit_state.read(cx).shutdown();
            async {}
        })
        .detach();

        let rename = cx.new(|cx| InputState::new(window, cx));
        cx.subscribe_in(&rename, window, |this, rename, event, _, cx| match event {
            InputEvent::PressEnter { .. } => {
                let title = rename.read(cx).value().to_string();
                this.state
                    .update(cx, |s, cx| s.finish_rename(Some(title), cx));
            }
            InputEvent::Blur => this.state.update(cx, |s, cx| s.finish_rename(None, cx)),
            _ => {}
        })
        .detach();

        let right_root = state.read(cx).effective().2;
        let right = cx.new(|cx| RightPanel::new(right_root, window, cx));
        let right_store_path = del_agent::index::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("right-panels.json");
        let right_store = right_panel::PanelStore::load(&right_store_path);
        let terminal_store = TerminalStore::load(&TerminalStore::path());
        state.update(cx, |s, _| {
            s.terminal_rows = terminal_store
                .records
                .iter()
                .map(|r| sessions::TerminalRow {
                    id: r.id.clone(),
                    title: if r.title.trim().is_empty() {
                        r.launch.label()
                    } else {
                        r.title.clone()
                    },
                    cwd: r.launch.cwd.clone(),
                    provider: r.launch.provider(),
                    active: false,
                    summary: String::new(),
                    pinned: r.pinned,
                })
                .collect();
            s.terminal_rows.sort_by_key(|row| !row.pinned);
            if let Some(error) = &terminal_store.error {
                s.error = Some(error.clone());
            }
        });
        let mcp_answer = cx.new(|cx| InputState::new(window, cx).placeholder("Your answer…"));
        let model_picker = cx.new(|cx| model_picker::ModelPicker::new(state.clone(), window, cx));
        let mut shell = Self {
            shell_focus: cx.focus_handle(),
            settings_focused: false,
            zoom_percent: del_core::zoom::percent(),
            state,
            composer,
            model_picker,
            rename,
            renaming_loaded: None,
            sidebar_widths: SidebarWidths::load(),
            resize_origin: Default::default(),
            tab_drag_origin: Default::default(),
            tab_strip_bounds: Default::default(),
            sidebar_open: true,
            sidebar_visible: true,
            sidebar_toggles: 0,
            tab_scroll: ScrollHandle::new(),
            tab_reveal: None,
            right,
            right_owner: 0,
            right_key: None,
            right_cache: HashMap::new(),
            right_store,
            right_store_path,
            right_save_generation: 0,
            right_open: false,
            right_visible: false,
            right_toggles: 0,
            right_animate: false,
            chats: HashMap::new(),
            search_index: search_ui::new_index(),
            search_open: false,
            terminal_search_generation: 0,
            dictation: None,
            dictation_generation: 0,
            composer_owner: 0,
            composer_drafts: HashMap::new(),
            composer_attachments: HashMap::new(),
            attachment_scroll: ScrollHandle::new(),
            queue_hovered: None,
            metrics: ShellMetrics::default(),
            terminals: HashMap::new(),
            terminal_store,
            terminal_attempts: HashSet::new(),
            terminal_opening: HashSet::new(),
            terminal_identity_pending: HashSet::new(),
            terminal_update_restarts: HashSet::new(),
            terminal_update_restarted: HashSet::new(),
            mcp: None,
            questions: VecDeque::new(),
            mcp_answer,
            notices: VecDeque::new(),
            reported: HashMap::new(),
            mode_focus: None,
            mode_slide_from: 0.0,
            mode_slide_started: None,
            caffeinate: None,
            dragging_tab: None,
            drag_target: None,
            previous_drag_target: None,
        };
        shell.start_modes(window, cx);
        shell.watch_terminal_titles(cx);
        shell.watch_right(cx);
        cx.on_app_quit(|this, cx| {
            this.cancel_dictation(cx);
            this.remember_right(cx);
            this.persist_right(cx);
            if let Err(error) = this.sidebar_widths.save() {
                eprintln!("Could not save sidebar widths: {error}");
            }
            async {}
        })
        .detach();
        shell
    }

    /// Make sure the open session has a live chat view.
    fn ensure_chat(&mut self, cx: &mut Context<Self>) {
        let unloaded = self
            .state
            .update(cx, |s, _| std::mem::take(&mut s.unloaded_chats));
        for id in unloaded {
            self.chats.remove(&id);
        }
        let Selection::Session(id) = self.state.read(cx).selection.clone() else {
            return;
        };
        if let Some(chat) = self.chats.get(&id) {
            // A completed transcript remains visible after its stream closes.
            // Rebuild only after that session is explicitly running again.
            if !chat.read(cx).is_closed()
                || self.state.read(cx).active_status() != del_agent::SessionStatus::Running
            {
                return;
            }
            self.chats.remove(&id);
        }
        let (backend, first) = self
            .state
            .update(cx, |s, _| (s.backend(), s.take_first_prompt(&id)));
        let session = id.clone();
        let chat = cx.new(|cx| ChatView::new(backend, Some(session), first, cx));
        // The shared composer reflects hydration/running state owned by ChatView.
        cx.observe(&chat, |_, _, cx| cx.notify()).detach();
        let tab = Tab::Session(id.clone());
        cx.subscribe(&chat, move |this, _, _: &chat::PromptSubmitted, cx| {
            this.state.update(cx, |state, cx| state.keep_tab(&tab, cx));
        })
        .detach();
        self.chats.insert(id, chat);
    }

    fn set_sidebar(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open == open {
            return;
        }
        self.sidebar_open = open;
        self.sidebar_toggles += 1;
        if open {
            self.sidebar_visible = true;
        } else {
            let toggle = self.sidebar_toggles;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PANEL_ANIM * 12 / 10).await;
                let _ = this.update(cx, |this, cx| {
                    if !this.sidebar_open && this.sidebar_toggles == toggle {
                        this.sidebar_visible = false;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Open or close the right panel, animating its width.
    fn set_right(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.right_open == open {
            return;
        }
        self.right_open = open;
        self.right_animate = true;
        self.right_toggles += 1;
        if open {
            self.right_visible = true;
        } else {
            // Keep it on screen until the close animation has finished.
            let toggle = self.right_toggles;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PANEL_ANIM * 12 / 10).await;
                let _ = this.update(cx, |this, cx| {
                    if !this.right_open && this.right_toggles == toggle {
                        this.right_visible = false;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        self.remember_right(cx);
        cx.notify();
    }

    fn watch_right(&mut self, cx: &mut Context<Self>) {
        cx.observe(&self.right, |this, panel, cx| {
            if this.right == panel {
                this.remember_right(cx);
            }
        })
        .detach();
    }

    fn remember_right(&mut self, cx: &mut Context<Self>) {
        self.right_cache
            .insert(self.right_owner, (self.right.clone(), self.right_open));
        let Some(key) = self.right_key.clone() else {
            return;
        };
        if self
            .right_store
            .set(key, self.right.read(cx).snapshot(self.right_open))
        {
            // Divider drags update in memory each frame; coalesce disk writes.
            self.right_save_generation += 1;
            let generation = self.right_save_generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.right_save_generation == generation {
                        this.persist_right(cx);
                    }
                });
            })
            .detach();
        }
    }

    fn persist_right(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.right_store.save(&self.right_store_path) {
            self.state.update(cx, |state, cx| {
                state.error = Some(format!("Could not save sidebar state: {error}"));
                cx.notify();
            });
        }
    }

    fn delete_browser_profile(&mut self, profile: String, cx: &mut Context<Self>) {
        if let Err(error) = crate::browser::record_deletion(&profile, true) {
            self.state.update(cx, |state, cx| {
                state.error = Some(format!("Could not schedule browser data deletion: {error}"));
                cx.notify();
            });
            return;
        }
        cx.spawn(async move |this, cx| {
            for _ in 0..5 {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let Ok(reply) = this.update(cx, |_, _| crate::browser::delete_profile(&profile))
                else {
                    return;
                };
                if matches!(reply.await, Ok(Ok(_))) {
                    let _ = crate::browser::record_deletion(&profile, false);
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |state, cx| {
                    state.error = Some(
                        "Browser data cleanup is pending; Del will retry at next launch.".into(),
                    );
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn sync_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let deleted = self
            .state
            .update(cx, |state, _| std::mem::take(&mut state.deleted_panels));
        for (key, uid) in deleted {
            let cached = self
                .right_cache
                .remove(&uid)
                .map(|(panel, _)| panel.read(cx).snapshot(false));
            let saved = self.right_store.remove(&key).or(cached);
            if self.right_key.as_ref() == Some(&key) {
                self.right_key = None;
                self.right =
                    cx.new(|cx| RightPanel::new(self.state.read(cx).effective().2, window, cx));
                self.right_owner = u64::MAX;
            }
            if let Some(saved) = saved {
                self.delete_browser_profile(saved.browser_profile().to_owned(), cx);
            }
            self.persist_right(cx);
        }
        let state = self.state.read(cx);
        let (tab, key) = match &state.selection {
            Selection::Draft(id) => (Tab::Draft(*id), None),
            Selection::Session(id) => (
                Tab::Session(id.clone()),
                Some(format!("chat:{}", id.as_str())),
            ),
            Selection::Terminal(id) => (
                Tab::Terminal(id.clone()),
                Some(format!("terminal:{}", id.as_str())),
            ),
        };
        let owner = state.uid_of(&tab);
        let root = state.effective().2;
        if self.right_owner != owner {
            self.remember_right(cx);
            let newly_created = !self.right_cache.contains_key(&owner);
            let (panel, open) = if let Some(cached) = self.right_cache.get(&owner) {
                cached.clone()
            } else {
                let saved = key
                    .as_deref()
                    .map(|key| self.right_store.get(key))
                    .unwrap_or_default();
                let open = saved.open;
                let panel = cx.new(|cx| {
                    let mut panel = RightPanel::new(root.clone(), window, cx);
                    panel.restore(saved, window, cx);
                    panel
                });
                (panel, open)
            };
            self.right = panel;
            self.right_owner = owner;
            self.right_open = open;
            self.right_visible = open;
            self.right_animate = false;
            self.right_toggles += 1;
            if newly_created {
                self.watch_right(cx);
            }
        }
        // A draft keeps its panel when it becomes a saved conversation.
        if self.right_key != key {
            self.right_key = key;
            self.remember_right(cx);
        }
        self.right.update(cx, |panel, cx| {
            panel.set_root(root, window, cx);
            if self.right_open {
                panel.prepare_to_show(window, cx);
            }
        });
    }

    fn go_tab(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.select_nth(n, cx));
        self.focus_composer(window, cx);
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            return;
        }
        if let Selection::Terminal(id) = &self.state.read(cx).selection
            && let Some(terminal) = self.terminals.get(id)
        {
            terminal.update(cx, |t, cx| t.focus(window, cx));
            return;
        }
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// The open session's chat view, if it has one.
    fn open_chat(&self, cx: &App) -> Option<Entity<ChatView>> {
        match &self.state.read(cx).selection {
            Selection::Session(id) => self.chats.get(id).cloned(),
            Selection::Draft(_) | Selection::Terminal(_) => None,
        }
    }

    /// Send the composer's text: starts the session from a draft, or sends to
    /// (steers) the open one. Clears the field.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_with_mode(false, window, cx);
    }

    fn submit_with_mode(&mut self, steer: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.is_some() {
            return;
        }
        let owner = self.composer_owner(cx);
        let text = self.composer.read(cx).value().trim().to_string();
        let attachments = self
            .composer_attachments
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        self.ensure_chat(cx);
        let chat = self.open_chat(cx);
        if text.is_empty() && attachments.is_empty() {
            if steer && let Some(chat) = chat {
                let first = chat
                    .read(cx)
                    .queue
                    .items
                    .first()
                    .map(|item| item.id.clone());
                if let Some(id) = first {
                    chat.update(cx, |chat, cx| {
                        chat.deliver_queued(id, chat.is_running(), cx)
                    });
                }
            }
            return;
        }
        if self.state.read(cx).terminal_draft().is_some() {
            self.submit_terminal_prompt(&text, &attachments, window, cx);
            return;
        }
        if chat.as_ref().is_some_and(|chat| chat.read(cx).is_loading())
            || self.state.read(cx).is_starting()
        {
            return;
        }
        let input = del_agent::UserInput { text, attachments };
        self.composer_attachments.remove(&owner);
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        match chat {
            Some(chat) => chat.update(cx, |chat, cx| chat.submit_input(input, steer, cx)),
            None => self
                .state
                .update(cx, |state, cx| state.send_input(input, cx)),
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.open_chat(cx) {
            chat.update(cx, |c, cx| c.stop(cx));
        }
    }

    /// The shared composer card, identical on the new-chat and chat pages.
    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = self.open_chat(cx);
        let running = chat.as_ref().is_some_and(|c| c.read(cx).is_running());
        let (provider, cwd, locked, ready) = {
            let st = self.state.read(cx);
            let (provider, _, cwd) = st.effective();
            (
                provider,
                cwd,
                !st.is_draft(),
                st.status_of(provider).is_some_and(providers::ready),
            )
        };
        let (shell, state_p, state_f) = (cx.entity(), self.state.clone(), self.state.clone());
        let shell_stop = shell.clone();
        let shell_attach = shell.clone();
        let shell_paste = shell.clone();
        let shell_voice = shell.clone();
        let retry_state = self.state.clone();
        let owner = self.composer_owner(cx);
        let has_attachments = self
            .composer_attachments
            .get(&owner)
            .is_some_and(|images| !images.is_empty());
        let composer = ComposerCard::new(
            &self.composer,
            COMPOSER_MAX_WIDTH,
            provider,
            ready,
            self.model_picker.clone(),
            sessions::folder_name(&cwd),
            locked,
            running,
        )
        .dictation(
            self.dictation
                .as_ref()
                .map_or(dictation::Phase::Idle, |voice| voice.phase),
        )
        .on_voice(move |window, cx| {
            shell_voice.update(cx, |this, cx| this.toggle_dictation(window, cx))
        })
        .show_folder(locked || self.state.read(cx).pending_prompt().is_some())
        .starting(
            self.state.read(cx).is_starting()
                || chat.as_ref().is_some_and(|chat| chat.read(cx).is_loading()),
        )
        .on_send(move |window, cx| shell.update(cx, |this, cx| this.submit(window, cx)))
        .on_stop(move |_, cx| shell_stop.update(cx, |this, cx| this.stop(cx)))
        .on_provider(move |p, _, cx| state_p.update(cx, |s, cx| s.set_provider(p, cx)))
        .on_folder(move |_, cx| state_f.update(cx, |s, cx| s.pick_folder(cx)))
        .attachments(has_attachments)
        .attachment_content(has_attachments.then(|| self.render_attachments(cx).into_any_element()))
        .on_attach(move |_, cx| shell_attach.update(cx, |this, cx| this.pick_attachments(cx)))
        .on_paste(move |item, _, cx| {
            shell_paste.update(cx, |this, cx| this.paste_attachments(item, cx))
        });
        v_flex()
            .w_full()
            .max_w(zpx(COMPOSER_MAX_WIDTH))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(owner, paths.0.to_vec(), cx)
            }))
            .when(
                self.state.read(cx).pending_error().is_some()
                    && self.state.read(cx).mode() == SessionMode::Chat,
                |column| {
                    column.child(
                        Button::new("retry-first-prompt")
                            .ghost()
                            .small()
                            .label("Retry message")
                            .on_click(move |_, _, cx| {
                                retry_state.update(cx, |state, cx| state.retry_pending(cx))
                            }),
                    )
                },
            )
            .child(self.render_followups(cx))
            .child(composer)
    }

    /// Session tabs in the top bar, with a plus button for a new session. The
    /// bar is also the window's drag area, so tabs stop mouse-down from
    /// starting a drag.
    fn render_topbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgba(0xffffff8c);
        let (tabs, selection, panel) = {
            let st = self.state.read(cx);
            // Live tabs, with just-closed ones (ghosts) kept at their old place
            // while they shrink away.
            let mut tabs: Vec<(Option<Tab>, u64, String, Option<Provider>)> = st
                .tabs
                .iter()
                .map(|tab| {
                    let (title, provider) = st.tab_info(tab);
                    (Some(tab.clone()), st.uid_of(tab), title, provider)
                })
                .collect();
            for g in &st.ghosts {
                let at = g.at.min(tabs.len());
                tabs.insert(at, (None, g.uid, g.title.clone(), g.provider));
            }
            (tabs, st.selection.clone(), st.panel)
        };
        if let Some((uid, started)) = self.tab_reveal
            && started.elapsed() <= state::TAB_ANIM + Duration::from_millis(50)
            && let Some(index) = tabs.iter().position(|(_, tab_uid, _, _)| *tab_uid == uid)
        {
            self.tab_scroll.scroll_to_item(index);
        }
        let dragging_tab = self.dragging_tab;
        let drag_target = self.drag_target;
        let previous_drag_target = self.previous_drag_target;
        let drag_source = dragging_tab.and_then(|uid| {
            tabs.iter()
                .position(|(tab, tab_uid, _, _)| tab.is_some() && *tab_uid == uid)
        });
        let tab = |id: SharedString, active: bool| {
            h_flex()
                .id(id.clone())
                .group(id)
                .h(zpx(24.0))
                .min_w(zpx(120.0))
                .max_w(zpx(200.0))
                .pl_2()
                .pr_1()
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_size(zpx(TEXT_MD))
                .text_color(if active { rgb(0xf5f5f5) } else { muted })
                .when(active, |t| t.bg(rgba(0xffffff14)))
                .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };

        TitleBar::new()
            .h(zpx(self.metrics.topbar_height))
            .min_h(zpx(self.metrics.topbar_height))
            .flex_none()
            .border_b_0()
            // Painted once, here and on the gutter below, never underneath the
            // page: stacked translucent layers would make the page more
            // opaque than the bar.
            .bg(shell_surface())
            .child(
                h_flex()
                    // TitleBar reserves 80px for macOS window controls. Its inner
                    // flex row cannot shrink, so bound our content explicitly.
                    .w((window.viewport_size().width
                        - gpui::px(if cfg!(target_os = "macos") {
                            80.0
                        } else {
                            12.0
                        }))
                    .max(zpx(0.0)))
                    .flex_none()
                    .h_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .w_0()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .items_center()
                            .gap_0()
                            .pl_2()
                            .on_children_prepainted({
                                let strip_bounds = self.tab_strip_bounds.clone();
                                move |bounds, _, _| {
                                    if let Some(bounds) = bounds.get(usize::from(SIDEBAR_ENABLED)) {
                                        strip_bounds.set(*bounds);
                                    }
                                }
                            })
                            .on_drag_move(cx.listener(
                                |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                                    let count = this.state.read(cx).tabs.len();
                                    if count == 0 || this.dragging_tab != Some(event.drag(cx).uid) {
                                        return;
                                    }
                                    let slot = TAB_WIDTH + TAB_GAP;
                                    let drag = event.drag(cx);
                                    let source = this
                                        .state
                                        .read(cx)
                                        .tabs
                                        .iter()
                                        .position(|tab| this.state.read(cx).uid_of(tab) == drag.uid)
                                        .unwrap_or(0);
                                    // Measure movement from where this tab was grabbed, so
                                    // grabbing its right edge cannot immediately shift the row.
                                    let x = source as f32 * slot
                                        + del_core::zoom::unscaled((event.event.position.x - drag.start_pointer_x.get())
                                            .as_f32());
                                    let mut target =
                                        (x / slot).round().clamp(0.0, (count - 1) as f32) as usize;
                                    // Keep a small dead zone around slot boundaries so a
                                    // fast pointer does not make adjacent tabs chatter.
                                    if let Some(current) = this.drag_target {
                                        let hysteresis = 12.0;
                                        let hold_forward = target > current
                                            && x < (current as f32 + 0.5) * slot + hysteresis;
                                        let hold_backward = target < current
                                            && x > (current as f32 - 0.5) * slot - hysteresis;
                                        if hold_forward || hold_backward {
                                            target = current;
                                        }
                                    }
                                    this.move_tab_drag(target, cx);
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(dragged) = this.dragging_tab {
                                        this.finish_tab_drag(dragged, cx);
                                    }
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(dragged) = this.dragging_tab {
                                        this.finish_tab_drag(dragged, cx);
                                    }
                                }),
                            )
                            .when(SIDEBAR_ENABLED, |row| {
                                row.child(
                                    div()
                                        .id("toggle-sidebar")
                                        .role(gpui::Role::Button)
                                        .aria_label("Toggle left sidebar")
                                        .flex_none()
                                        .size(zpx(24.0))
                                        .mr_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|b| b.bg(rgba(0xffffff0a)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_sidebar(!this.sidebar_open, cx);
                                        }))
                                        .child(
                                            Icon::new(if self.sidebar_open {
                                                IconName::PanelLeftClose
                                            } else {
                                                IconName::PanelLeftOpen
                                            })
                                            .with_size(zpx(sidebar::CONTROL_ICON_SIZE))
                                            .text_color(muted),
                                        ),
                                )
                            })
                            .child(
                                h_flex()
                                    .id("tab-scroll")
                                    .w_0()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .items_center()
                                    .overflow_x_scroll()
                                    .track_scroll(&self.tab_scroll)
                                    .on_scroll_wheel(cx.listener(
                                        |this, event: &ScrollWheelEvent, window, cx| {
                                            let delta =
                                                event.delta.pixel_delta(window.line_height());
                                            let offset = this.tab_scroll.offset();
                                            let dx = if delta.x.abs() > delta.y.abs() {
                                                delta.x
                                            } else {
                                                delta.y
                                            };
                                            this.tab_scroll
                                                .set_offset(point(offset.x + dx, zpx(0.0)));
                                            cx.stop_propagation();
                                            cx.notify();
                                        },
                                    ))
                                    .children(tabs.into_iter().enumerate().map(
                                        |(index, (tab_key, uid, title, provider))| {
                                            let live = tab_key.is_some();
                                            let preview = tab_key.as_ref().is_some_and(|tab| self.state.read(cx).preview_tab.as_ref() == Some(tab));
                                            let animate_open = !matches!(tab_key, Some(Tab::Session(_)));

                                            let shortcut_number = tab_key.as_ref().and_then(|tab| self.state.read(cx).tabs.iter().position(|candidate| candidate == tab)).filter(|index| *index < 9).map(|index| index + 1);
                                            let shortcut_hint = !self.search_open && self.state.read(cx).shortcut_hints && shortcut_number.is_some();
                                            let active = tab_key
                                                .as_ref()
                                                .is_some_and(|t| selection == t.selection())
                                                && panel == Panel::Chat;
                                            let name = match &tab_key {
                                                Some(Tab::Session(id))
                                                | Some(Tab::Terminal(id)) => id.0.clone(),
                                                Some(Tab::Draft(k)) => format!("draft-{k}"),
                                                None => format!("closing-{uid}"),
                                            };
                                            let title_loading = match &tab_key {
                                                Some(Tab::Terminal(id)) => self.state.read(cx).title_loading(id),
                                                _ => false,
                                            };

                                            let group = SharedString::from(format!("tab-{name}"));
                                            let mut el = tab(group.clone(), active);
                                            if let Some(tab_key) = tab_key.clone() {
                                                let (select, close) =
                                                    (self.state.clone(), self.state.clone());
                                                let (select_tab, close_tab) =
                                                    (tab_key.clone(), tab_key);
                                                let indicator_id = match &select_tab { Tab::Session(id) | Tab::Terminal(id) => Some(id.clone()), _ => None };
                                                let drag_shell = cx.entity();
                                                let drop_shell = drag_shell.clone();
                                                let drag = DraggedTab {
                                                    uid,
                                                    title: title.clone(),
                                                    provider,
                                                    start_pointer_y: Pixels::ZERO,
                                                    cursor_offset_x: Pixels::ZERO,
                                                    strip_bounds: self.tab_strip_bounds.clone(),
                                                    start_pointer_x: self.tab_drag_origin.clone(),
                                                };
                                                let start_x = drag.start_pointer_x.clone();
                                                el = el
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |event, _, _| {
                                                            start_x.set(event.position.x)
                                                        },
                                                    )
                                                    .on_drag(drag, move |drag, offset, window, cx| {
                                                        drag_shell.update(cx, |this, cx| {
                                                            this.begin_tab_drag(drag.uid, cx)
                                                        });
                                                        let mut preview = drag.clone();
                                                        preview.cursor_offset_x = offset.x;
                                                        preview.start_pointer_y =
                                                            window.mouse_position().y;
                                                        cx.new(|_| preview)
                                                    })
                                                    .on_drop(move |drag: &DraggedTab, _, cx| {
                                                        drop_shell.update(cx, |this, cx| {
                                                            this.finish_tab_drag(drag.uid, cx)
                                                        })
                                                    })
                                                    .on_click(move |event, _, cx| {
                                                        let tab = select_tab.clone();
                                                        select.update(cx, |s, cx| {
                                                            s.select_tab(&tab, cx);
                                                            if event.click_count() == 2 { s.keep_tab(&tab, cx); }
                                                        });
                                                    })
                                                    .child(mode_ui::tab_mark(provider))
                                                    .child(if title_loading {
                                                        div().flex_1().min_w_0().truncate().child(
                                                            gpui_kit::component::shimmer::ShimmerText::new(title)
                                                                .id(SharedString::from(format!("title-shimmer-{name}"))))
                                                    } else { tab_title(title).when(preview, |title| title.italic().opacity(0.65)) })
                                                    .when(shortcut_hint, |tab| tab.child(div().absolute().right(zpx(6.)).top(zpx(5.)).px_1().rounded_sm().bg(rgb(0x303033)).text_size(zpx(10.)).child(format!("⌘{}", shortcut_number.unwrap_or(1)))))
                                                    .when_some(indicator_id.as_ref(), |tab, id| tab.child(sidebar::unread_dot(id, self.state.read(cx), cx)))
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "close-{name}"
                                                            )))
                                                            .size(zpx(16.0))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .flex_none()
                                                            .rounded_sm()
                                                            // Shown on hover only; faded rather than hidden
                                                            // so layout and painting stay stable.
                                                            .opacity(0.0)
                                                            .group_hover(group.clone(), |c| {
                                                                c.opacity(1.0)
                                                            })
                                                            .hover(|c| c.bg(rgba(0xffffff1a)))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                |_, _, cx| cx.stop_propagation(),
                                                            )
                                                            .on_click(move |_, _, cx| {
                                                                cx.stop_propagation();
                                                                let tab = close_tab.clone();
                                                                close.update(cx, |s, cx| {
                                                                    s.close_tab(&tab, cx)
                                                                });
                                                            })
                                                            .child(
                                                                Icon::new(IconName::Close)
                                                                    .with_size(zpx(sidebar::CONTROL_ICON_SIZE))
                                                                    .text_color(muted),
                                                            ),
                                                    );
                                            } else {
                                                // Closing: same look, no handlers.
                                                el = el
                                                    .child(mode_ui::tab_mark(provider))
                                                    .child(tab_title(title))
                                                    .child(div().size(zpx(16.0)).flex_none());
                                            }
                                            // The tab keeps its full size inside a clip whose width
                                            // (tab plus the gap after it) animates from 0 on open and
                                            // back to 0 on close. Padding and gap live inside the clip,
                                            // so nothing is left behind at width 0 and nothing jumps
                                            // when a closed tab is finally removed.
                                            let clip = div()
                                                .flex_none()
                                                .w(zpx(TAB_WIDTH + TAB_GAP))
                                                .overflow_hidden()
                                                .child(
                                                    el.w(zpx(TAB_WIDTH))
                                                        .min_w(zpx(0.0))
                                                        .max_w(zpx(TAB_WIDTH))
                                                        .flex_none(),
                                                );

                                            let offset_for = |target: Option<usize>| -> f32 {
                                                let (Some(source), Some(target)) =
                                                    (drag_source, target)
                                                else {
                                                    return 0.0;
                                                };
                                                if source < target
                                                    && index > source
                                                    && index <= target
                                                {
                                                    -(TAB_WIDTH + TAB_GAP)
                                                } else if target < source
                                                    && index >= target
                                                    && index < source
                                                {
                                                    TAB_WIDTH + TAB_GAP
                                                } else {
                                                    0.0
                                                }
                                            };
                                            let old_offset = offset_for(previous_drag_target);
                                            let new_offset = offset_for(drag_target);
                                            let shifted = div()
                                                .flex_none()
                                                .when(dragging_tab == Some(uid), |tab| {
                                                    tab.opacity(0.0)
                                                })
                                                .child(clip)
                                                .with_animation(
                                                    SharedString::from(format!(
                                                        "tab-shift-{uid}-{old_offset}-{new_offset}"
                                                    )),
                                                    Animation::new(Duration::from_millis(90))
                                                        .with_easing(ease_out_quint()),
                                                    move |tab, t| {
                                                        tab.relative().left(zpx(old_offset
                                                            + (new_offset - old_offset) * t))
                                                    },
                                                );
                                            // This stable slot owns width animation. A new drag
                                            // target may restart the child's position animation,
                                            // but must never restart the tab's opening animation.
                                            div()
                                                .flex_none()
                                                .when(dragging_tab.is_none(), |slot| {
                                                    slot.overflow_hidden()
                                                })
                                                .child(shifted)
                                                .with_animation(
                                                    SharedString::from(format!(
                                                        "tab-anim-{uid}-{live}"
                                                    )),
                                                    Animation::new(state::TAB_ANIM)
                                                        .with_easing(ease_out_quint()),
                                                    move |slot, t| {
                                                        let t = if live { if animate_open { t } else { 1.0 } } else { 1.0 - t };
                                                        slot.w(zpx((TAB_WIDTH + TAB_GAP) * t))
                                                    },
                                                )
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("new-tab")
                                    .role(gpui::Role::Button)
                                    .aria_label("New tab")
                                    .flex_none()
                                    .when(dragging_tab.is_some(), |plus| plus.opacity(0.0))
                                    .size(zpx(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|plus| plus.bg(rgba(0xffffff0a)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.dragging_tab.is_some() {
                                            return;
                                        }
                                        this.state.update(cx, |s, cx| s.new_session(cx));
                                        this.focus_composer(window, cx);
                                    }))
                                    .child(Icon::new(IconName::Plus).with_size(zpx(sidebar::CONTROL_ICON_SIZE)).text_color(muted)),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-right")
                            .role(gpui::Role::Button)
                            .aria_label("Toggle right sidebar")
                            .flex_none()
                            .size(zpx(24.0))
                            .mr_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|b| b.bg(rgba(0xffffff0a)))
                            .when(self.right_open, |b| b.bg(rgba(0xffffff14)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(self.state.read(cx).panel == Panel::Providers, |button| button.invisible())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.state.read(cx).panel == Panel::Chat { this.set_right(!this.right_open, cx); }
                            }))
                            .child(
                                Icon::new(if self.right_open {
                                    IconName::PanelRightClose
                                } else {
                                    IconName::PanelRightOpen
                                })
                                .with_size(zpx(sidebar::CONTROL_ICON_SIZE))
                                .text_color(muted),
                            ),
                    ),
            )
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = zpx(self.metrics.page_radius);
        let (panel, selection) = {
            let st = self.state.read(cx);
            (st.panel, st.selection.clone())
        };
        let chat = self.open_chat(cx);
        if panel == Panel::Chat && matches!(selection, Selection::Terminal(_)) {
            return div()
                .size_full()
                .rounded(radius)
                .overflow_hidden()
                .bg(page_surface())
                .child(self.render_terminal_page(cx))
                .into_any_element();
        }

        div()
            .id("page")
            .relative()
            .size_full()
            .rounded(radius)
            .overflow_hidden()
            .bg(page_surface())
            .when(panel == Panel::Providers, |page| {
                page.child(signin::render(&self.state, cx))
            })
            .when(panel == Panel::Chat, |page| match chat {
                // Chat page: transcript above, the shared composer below.
                Some(chat) => page.child(
                    v_flex()
                        .size_full()
                        .child(div().flex_1().min_h_0().child(chat))
                        .child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .px_3()
                                .pb_3()
                                .child(self.render_composer(cx)),
                        ),
                ),
                // Show the first message immediately, while the agent starts.
                None if self.state.read(cx).pending_prompt().is_some() => {
                    let state = self.state.read(cx);
                    let transition_key = match selection {
                        Selection::Draft(key) => key,
                        Selection::Session(_) | Selection::Terminal(_) => 0,
                    };
                    page.child(
                        v_flex()
                            .size_full()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .child(chat::pending_prompt(
                                        state.pending_input().unwrap().clone(),
                                        state.pending_error().map(str::to_owned),
                                    ))
                                    .with_animation(
                                        ("first-prompt", transition_key),
                                        Animation::new(Duration::from_millis(120))
                                            .with_easing(ease_out_quint()),
                                        |transcript, t| transcript.opacity(t),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .w_full()
                                    .items_center()
                                    .px_3()
                                    .pb_3()
                                    .child(self.render_composer(cx)),
                            ),
                    )
                }
                // New-chat page: heading and the same composer, centred.
                None => page.child(
                    v_flex()
                        .absolute()
                        .inset_0()
                        .items_center()
                        .justify_center()
                        .px_3()
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(zpx(COMPOSER_MAX_WIDTH))
                                .items_start()
                                .gap_2()
                                .child(
                                    div()
                                        // The composer's padding plus the input's own
                                        // inset, so the heading and the input text
                                        // share a left edge.
                                        .pl(zpx(17.5))
                                        .text_size(zpx(TEXT_LG))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(0xf5f5f5))
                                        .child(HEADING),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .pb(zpx(40.0))
                                        .child(
                                            div()
                                                .absolute()
                                                .bottom_0()
                                                .left(zpx(16.0))
                                                .right(zpx(16.0))
                                                .h(zpx(40.0))
                                                .rounded_b(zpx(14.0))
                                                .bg(rgb(0x1b1b1b))
                                                .child(self.render_new_session_options(cx)),
                                        )
                                        .child(div().relative().child(self.render_composer(cx))),
                                ),
                        ),
                ),
            })
            .into_any_element()
    }
}

impl Drop for AppShell {
    fn drop(&mut self) {
        if let Some(mut child) = self.caffeinate.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zoom = del_core::zoom::percent();
        if self.zoom_percent != zoom {
            self.zoom_percent = zoom;
            for chat in self.chats.values() {
                chat.update(cx, |chat, cx| chat.remeasure_for_zoom(cx));
            }
        }
        if self.dragging_tab.is_some() && !cx.has_active_drag() {
            self.dragging_tab = None;
            self.drag_target = None;
            self.previous_drag_target = None;
        }
        if !window.is_window_active() && self.state.read(cx).shortcut_hints {
            self.state
                .update(cx, |state, _| state.shortcut_hints = false);
        }
        if let Some(error) = self.state.update(cx, |state, _| state.error.take()) {
            window.defer(cx, move |window, cx| {
                window.push_notification(
                    gpui_kit::component::notification::Notification::error(error)
                        .title("Something went wrong")
                        .autohide(false),
                    cx,
                );
            });
        }
        for chat in self.chats.values() {
            let errors = chat.update(cx, |chat, _| std::mem::take(&mut chat.pending_errors));
            for error in errors {
                window.defer(cx, move |window, cx| {
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(error)
                            .title("Session error")
                            .autohide(false),
                        cx,
                    );
                });
            }
        }
        if self.dictation.as_ref().is_some_and(|voice| {
            voice.owner != self.composer_owner(cx) || self.state.read(cx).panel != Panel::Chat
        }) {
            self.cancel_dictation(cx);
        }
        let settings_open = self.state.read(cx).panel == Panel::Providers;
        if settings_open && !self.settings_focused {
            self.shell_focus.focus(window, cx);
        }
        self.settings_focused = settings_open;
        self.sync_caffeine(cx);
        self.sync_terminal_sidebar_actions(cx);
        let right_visible = self.right_visible && self.state.read(cx).panel == Panel::Chat;
        let inset = zpx(self.metrics.page_inset);
        let viewport = del_core::zoom::unscaled(window.viewport_size().width.as_f32());
        let left_width = sidebar_width(
            self.sidebar_widths.left,
            false,
            viewport,
            if right_visible {
                self.sidebar_widths.right.min(400.0)
            } else {
                0.0
            },
        );
        let right_width = sidebar_width(
            self.sidebar_widths.right,
            true,
            viewport,
            if self.sidebar_visible {
                left_width
            } else {
                0.0
            },
        );
        if self
            .mode_slide_started
            .is_some_and(|started| started.elapsed() < Duration::from_millis(180))
        {
            window.request_animation_frame();
        }
        self.ensure_modes(window, cx);
        let mode_key = (
            self.state.read(cx).selection.clone(),
            self.state.read(cx).mode(),
        );
        if self.state.read(cx).panel == Panel::Chat && self.mode_focus.as_ref() != Some(&mode_key) {
            self.mode_focus = Some(mode_key);
            self.focus_composer(window, cx);
        }
        self.ensure_chat(cx);
        self.sync_composer_draft(window, cx);
        let selected_uid = self.composer_owner(cx);
        if self.tab_reveal.is_none_or(|(uid, _)| uid != selected_uid) {
            self.tab_reveal = Some((selected_uid, std::time::Instant::now()));
        }
        self.sync_right(window, cx);
        self.right.update(cx, |panel, cx| {
            panel.set_preview_visible(right_visible && !self.search_open, cx)
        });
        for (panel, _) in self.right_cache.values() {
            if panel != &self.right {
                panel.update(cx, |panel, cx| panel.set_preview_visible(false, cx));
            }
        }

        // Load the title into the rename field when a rename starts.
        let renaming = self.state.read(cx).renaming.clone();
        if renaming != self.renaming_loaded {
            if let Some(id) = &renaming {
                let title = self.state.read(cx).current_title(id);
                self.rename.update(cx, |input, cx| {
                    input.set_value(title, window, cx);
                    input.focus(window, cx);
                });
            }
            self.renaming_loaded = renaming;
        }

        v_flex()
            .size_full()
            .track_focus(&self.shell_focus)
            .key_context("Shell")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.state.read(cx).panel == Panel::Providers
                {
                    this.state
                        .update(cx, |state, cx| state.toggle_providers(cx));
                    this.focus_composer(window, cx);
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.shortcut_hints = event.modifiers.platform && !this.search_open;
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &SearchConversations, window, cx| {
                this.open_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if !SIDEBAR_ENABLED {
                    return;
                }
                this.set_sidebar(!this.sidebar_open, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| {
                if this.state.read(cx).panel == Panel::Chat {
                    this.set_right(!this.right_open, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                if !this.right_open {
                    this.set_right(true, cx);
                }
                this.right.update(cx, |r, cx| r.show_terminal(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.state.update(cx, |s, cx| s.close_active_tab(cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Tab1, w, cx| this.go_tab(0, w, cx)))
            .on_action(cx.listener(|this, _: &Tab2, w, cx| this.go_tab(1, w, cx)))
            .on_action(cx.listener(|this, _: &Tab3, w, cx| this.go_tab(2, w, cx)))
            .on_action(cx.listener(|this, _: &Tab4, w, cx| this.go_tab(3, w, cx)))
            .on_action(cx.listener(|this, _: &Tab5, w, cx| this.go_tab(4, w, cx)))
            .on_action(cx.listener(|this, _: &Tab6, w, cx| this.go_tab(5, w, cx)))
            .on_action(cx.listener(|this, _: &Tab7, w, cx| this.go_tab(6, w, cx)))
            .on_action(cx.listener(|this, _: &Tab8, w, cx| this.go_tab(7, w, cx)))
            .on_action(cx.listener(|this, _: &Tab9, w, cx| this.go_tab(8, w, cx)))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.state.update(cx, |s, cx| s.new_session(cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(-1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusComposer, window, cx| {
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &StopSession, _, cx| {
                this.state.update(cx, |s, cx| s.stop(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleProviders, _, cx| {
                this.state.update(cx, |s, cx| s.toggle_providers(cx));
            }))
            .child(self.render_topbar(window, cx))
            .children(self.render_agent_messages(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .bg(shell_surface())
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<SidebarResize>, window, cx| {
                            let drag = event.drag(cx);
                            let delta = del_core::zoom::unscaled(
                                window.mouse_position().x.as_f32() - drag.start_x.get(),
                            );
                            let width = sidebar_width(
                                drag.width + if drag.right { -delta } else { delta },
                                drag.right,
                                del_core::zoom::unscaled(window.viewport_size().width.as_f32()),
                                drag.other,
                            );
                            if drag.right {
                                this.sidebar_widths.right = width;
                            } else {
                                this.sidebar_widths.left = width;
                            }
                            cx.notify();
                        },
                    ))
                    .when(SIDEBAR_ENABLED && self.sidebar_visible, |row| {
                        let sidebar = div()
                            .relative()
                            .h_full()
                            .flex_none()
                            .overflow_hidden()
                            .child(sidebar::render(&self.state, &self.rename, cx));
                        if self.sidebar_toggles == 0 {
                            row.child(sidebar.w(zpx(left_width)))
                        } else {
                            let opening = self.sidebar_open;
                            row.child(sidebar.with_animation(
                                ("left-sidebar", self.sidebar_toggles),
                                Animation::new(PANEL_ANIM).with_easing(ease_out_quint()),
                                move |element, t| {
                                    let t = if opening { t } else { 1.0 - t };
                                    element.w(zpx(left_width * t))
                                },
                            ))
                        }
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(inset)
                            .pb(inset)
                            .child(self.render_page(cx))
                            .when(self.sidebar_visible, |page| {
                                page.child(
                                    self.resize_handle(
                                        false,
                                        left_width,
                                        if right_visible { right_width } else { 0.0 },
                                    )
                                    .left(inset)
                                    .bottom(inset),
                                )
                            }),
                    )
                    .when(right_visible, |row| {
                        let opening = self.right_open;
                        let animate = self.right_animate;
                        let full = right_width + self.metrics.page_inset;
                        row.child(
                            div()
                                .relative()
                                .h_full()
                                .flex_none()
                                .overflow_hidden()
                                .child(
                                    // Fills whatever width the animation gives it: the panel
                                    // grows out of the page's own space and lays out at the
                                    // current width instead of sliding in from offscreen.
                                    div().size_full().pr(inset).pb(inset).child(
                                        div()
                                            .size_full()
                                            .rounded(zpx(self.metrics.page_radius))
                                            .overflow_hidden()
                                            .bg(page_surface())
                                            .child(self.right.clone()),
                                    ),
                                )
                                .child(
                                    self.resize_handle(
                                        true,
                                        right_width,
                                        if self.sidebar_visible {
                                            left_width
                                        } else {
                                            0.0
                                        },
                                    )
                                    .left_0()
                                    .bottom(inset),
                                )
                                .with_animation(
                                    ("right-panel", self.right_toggles),
                                    Animation::new(PANEL_ANIM).with_easing(ease_out_quint()),
                                    move |el, t| {
                                        let t = if !animate {
                                            1.0
                                        } else if opening {
                                            t
                                        } else {
                                            1.0 - t
                                        };
                                        el.w(zpx(full * t))
                                    },
                                ),
                        )
                    }),
            )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            // Must run before any gpui-component feature is used.
            gpui_kit::init(cx);
            assets::set_app_icon();
            del_terminal::init(cx);
            zoom::load();
            zoom::init(cx);
            appearance::load(cx);

            cx.bind_keys([
                KeyBinding::new("cmd-k", SearchConversations, None),
                KeyBinding::new("cmd-\\", ToggleFiles, None),
                KeyBinding::new("cmd-shift-b", ToggleFiles, None),
                KeyBinding::new("ctrl-`", ToggleTerminal, None),
                KeyBinding::new("cmd-j", ToggleTerminal, None),
                KeyBinding::new("cmd-t", NewSession, None),
                KeyBinding::new("cmd-n", NewSession, None),
                KeyBinding::new("cmd-w", CloseTab, None),
                KeyBinding::new("cmd-1", Tab1, None),
                KeyBinding::new("cmd-2", Tab2, None),
                KeyBinding::new("cmd-3", Tab3, None),
                KeyBinding::new("cmd-4", Tab4, None),
                KeyBinding::new("cmd-5", Tab5, None),
                KeyBinding::new("cmd-6", Tab6, None),
                KeyBinding::new("cmd-7", Tab7, None),
                KeyBinding::new("cmd-8", Tab8, None),
                KeyBinding::new("cmd-9", Tab9, None),
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-tab", NextSession, None),
                KeyBinding::new("cmd-shift-]", NextSession, None),
                KeyBinding::new("ctrl-shift-tab", PrevSession, None),
                KeyBinding::new("cmd-shift-[", PrevSession, None),
                KeyBinding::new("cmd-l", FocusComposer, None),
                KeyBinding::new("cmd-.", StopSession, None),
                KeyBinding::new("cmd-,", ToggleProviders, None),
            ]);
            // Quit runs the shutdown hook, which ends every backend process.
            cx.on_action(|_: &Quit, cx| cx.quit());
            // Closing the window quits, which also runs the shutdown hook.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let metrics = ShellMetrics::default();
            let (light_x, light_y) = metrics.traffic_light_origin();

            let window_size = cx.primary_display().map_or(WINDOW_SIZE, |display| {
                let available = display.visible_bounds().size;
                size(
                    (available.width * 0.70).max(px(960.)),
                    (available.height * 0.76).max(px(640.)),
                )
            });
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("del".into()),
                    traffic_light_position: Some(point(px(light_x), px(light_y))),
                    // Hides the system title bar so the top bar below is drawn
                    // entirely by us.
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    window_size,
                    cx,
                ))),
                window_min_size: Some(size(px(960.0), px(640.0))),
                window_background: WindowBackgroundAppearance::Blurred,
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| AppShell::new(window, cx))
            })
            .expect("failed to open window");

            cx.activate(true);
        });
}
