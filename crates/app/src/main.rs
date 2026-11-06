//! The `del` application shell: a custom top bar that hosts the macOS traffic
//! lights, over a rounded black page inset from the window edges. The window
//! itself is translucent and blurred, so the desktop shows through the shell.
//!
//! Layout metrics and the shell colours live in `del_core`; this crate renders
//! them.

use crate::animated_icons::Glyph;
use crate::keyboard_control::KeyboardControl as _;
use crate::micro_motion::MotionIconExt as _;
use crate::zoom::px as zpx;
// The wallpaper background is switched off for now; the loader stays so it can
// be wired back in without redoing the preload and decode work.
#[allow(dead_code)]
mod wallpaper;

mod accounts;
mod ambient;
mod animated_icons;
mod app_updates;
mod appearance;
mod assets;
mod attachments;
mod backdrop;
mod branch_picker;
mod browser;
mod browser_preview;
mod chat;
mod command_menu;
mod composer_ui;
mod conversation_search;
mod dictation;
mod document_preview;
mod followup_ui;
mod followups;
#[cfg(test)]
mod goal_integration;
mod goal_ui;
mod haptics;
mod hover_motion;
mod icons;
mod keybinds;
mod keyboard_control;
#[allow(dead_code)]
mod mcp_bridge;
mod micro_motion;
mod mode_ui;
mod model_picker;
mod nav_history;
mod new_tab;
mod panel_motion;
mod projects;
mod provider_updates;
mod providers;
mod readiness;
mod right_panel;
mod search_ui;
mod session_lifecycle;
mod session_modes;
mod session_usage;
mod sessions;
mod side_chat;
mod sidebar;
mod signin;
mod state;
mod status_processes;
mod tab_motion;
mod tab_transfer;
mod tab_zone;
mod terminal_conversation;
#[allow(dead_code)]
mod terminal_host;
mod terminal_store;
mod ui;
mod usage;
mod voice_ui;
mod workbench;
mod workspace_files;
mod workspace_tabs;
mod zoom;

use crate::appearance::{page_surface, rgb, rgba, shell_surface};
use chat::ChatView;
use composer_ui::ComposerCard;
use del_agent::{AgentBackend, MockBackend, Provider, SessionId};
use del_core::ShellMetrics;
use del_core::typography::TEXT_MD;
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
const TAB_WIDTH: f32 = 144.0;

/// Session tab height inside the 40 px title bar.
const TAB_HEIGHT: f32 = 26.0;

/// Space after each tab, animated together with it.
const TAB_GAP: f32 = 4.0;

/// Stable tab identity also survives a draft becoming a live session mid-drag.
#[derive(Clone)]
struct DraggedTab {
    uid: u64,
    title: String,
    provider: Option<Provider>,
    terminal: bool,
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
        div().w(zpx(TAB_WIDTH)).h(zpx(TAB_HEIGHT)).child(
            h_flex()
                .relative()
                .top(self.start_pointer_y - window.mouse_position().y)
                .left(left - natural_left)
                .w(width)
                .overflow_hidden()
                .h(zpx(TAB_HEIGHT))
                .px_2()
                .gap_2()
                .items_center()
                .rounded(zpx(8.0))
                .bg(appearance::dragged_tab_surface())
                .text_color(rgb(0xf5f5f5))
                .text_size(zpx(TEXT_MD))
                .shadow_md()
                .child(if self.terminal {
                    mode_ui::terminal_mark(self.provider)
                } else {
                    mode_ui::tab_mark(self.provider)
                })
                .child(tab_title(self.title.clone())),
        )
    }
}

/// The title inside a tab.
fn tab_title(title: String) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(zpx(22.0))
        .line_height(zpx(22.0))
        .overflow_hidden()
        .child(sidebar::fading_title(title))
}

/// Top-tab drag shifts use the sidebar's 90 ms quint.
const TAB_SHIFT_MOTION: Duration = Duration::from_millis(90);
/// The close slot opens and closes at the fill speed.
const CLOSE_SLOT_MOTION: Duration = Duration::from_millis(ui::FILL_MS);

/// A value easing toward a target. Retargeting starts from the current
/// presentation, so rapid input never jumps back.
#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    started: Option<std::time::Instant>,
    duration: Duration,
}

impl Tween {
    fn settled(value: f32, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            started: None,
            duration,
        }
    }

    /// Reduce Motion settles at once.
    fn retarget(&mut self, to: f32, now: std::time::Instant, reduce_motion: bool) {
        if reduce_motion {
            *self = Self::settled(to, self.duration);
        } else if to != self.to {
            self.from = self.sample(now).0;
            self.to = to;
            self.started = Some(now);
        }
    }

    fn sample(&self, now: std::time::Instant) -> (f32, bool) {
        let Some(started) = self.started else {
            return (self.to, false);
        };
        let t = (now.saturating_duration_since(started).as_secs_f32()
            / self.duration.as_secs_f32().max(f32::EPSILON))
        .min(1.);
        let eased = 1. - (1. - t).powi(5);
        (self.from + (self.to - self.from) * eased, t < 1.)
    }
}

/// The keyboard hint and hover close glyph share the reserved trailing slot.
/// Keeping both inside it prevents the hint from covering the close button.
/// `hint_fade` restarts the hint's fade-in for each fresh appearance; `None`
/// (Reduce Motion) shows it at once.
fn tab_action_content(
    group: SharedString,
    shortcut: Option<usize>,
    color: Hsla,
    working: bool,
    focused: bool,
    hint_fade: Option<u64>,
) -> impl IntoElement {
    let glyph_group = group.clone();
    div()
        .id((ElementId::from(group.clone()), "close-motion"))
        .relative()
        .w(if shortcut.is_some() {
            crate::icons::button_size()
        } else {
            zpx(crate::icons::TAB_CLOSE_SIZE)
        })
        .h(zpx(crate::icons::TAB_CLOSE_SIZE))
        .group_hover(group.clone(), |slot| {
            slot.w(zpx(crate::icons::TAB_CLOSE_SIZE))
        })
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when(working, |slot| {
            slot.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(focused, |indicator| indicator.opacity(0.))
                    .group_hover(group.clone(), |indicator| indicator.opacity(0.))
                    .child(crate::icons::working_spinner().color(color)),
            )
        })
        .when_some(shortcut.filter(|_| !working), |slot, number| {
            slot.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(focused, |hint| hint.opacity(0.0))
                    .group_hover(group, |hint| hint.opacity(0.0))
                    .child({
                        let badge = ui::kbd(format!("⌘{number}"))
                            .h(zpx(crate::icons::TAB_CLOSE_SIZE))
                            .min_w(zpx(crate::icons::TAB_CLOSE_SIZE));
                        match hint_fade {
                            Some(epoch) => badge
                                .with_animation(
                                    SharedString::from(format!("hint-fade-{epoch}")),
                                    Animation::new(Duration::from_millis(ui::FILL_MS))
                                        .with_easing(ui::ease_out),
                                    |badge, t| badge.opacity(t),
                                )
                                .into_any_element(),
                            None => badge.into_any_element(),
                        }
                    }),
            )
        })
        .motion_child(true, move |feedback| {
            div()
                .flex()
                .opacity(if focused { 1.0 } else { 0.0 })
                .group_hover(glyph_group, |glyph| glyph.opacity(1.0))
                .child(
                    feedback
                        .glyph(Glyph::Close)
                        .with_size(zpx(crate::icons::TAB_CLOSE_GLYPH))
                        .text_color(color),
                )
        })
}

/// Identifies Del's toasts, so a repeat of one still on screen replaces it.
struct NoticeToast;

/// One toast for a queued notice: errors stay until dismissed; warnings and
/// information hide on their own.
fn push_notice(
    notice: state::notices::Notice,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::notification::Notification;
    use state::notices::{Severity, Undo};
    let mut toast = match notice.severity {
        Severity::Error => Notification::error(notice.message.clone()),
        Severity::Warning => Notification::warning(notice.message.clone()),
        Severity::Info => Notification::info(notice.message.clone()),
    }
    .id1::<NoticeToast>(SharedString::from(notice.key()))
    .autohide(!notice.severity.sticky());
    if let Some(title) = notice.title.clone() {
        toast = toast.title(title);
    }
    if let Some(Undo::Unarchive(id)) = notice.undo.clone() {
        let state = state.clone();
        toast = toast.action(move |_, _, _| {
            let state = state.clone();
            let id = id.clone();
            Button::new("notice-undo")
                .small()
                .label("Undo")
                .on_click(move |_, _, cx| {
                    state.update(cx, |state, cx| state.restore(id.clone(), cx));
                })
        });
    }
    window.push_notification(toast, cx);
}

/// Shared timing for the left and right sidebar width transitions.
const PANEL_ANIM: Duration = Duration::from_millis(220);

/// Widest the composer grows on a large window, in logical pixels.
const COMPOSER_MAX_WIDTH: f32 = chat::COLUMN_MAX_WIDTH;

/// One container under the new-tab composer: the composer surface at the
/// composer's 8 px radius, sized to its controls.
fn new_tab_group() -> Div {
    h_flex()
        .flex_none()
        .h(zpx(32.0))
        .px_1()
        .items_center()
        .rounded(zpx(8.0))
        .bg(crate::appearance::composer_surface())
}

#[derive(::gpui::Action, Clone, PartialEq, serde::Deserialize)]
#[action(namespace = del, no_json)]
struct ReopenSideChat {
    id: String,
    parent: del_agent::SessionId,
}

actions!(
    del,
    [
        NewSession,
        NewWorkspaceTab,
        NextWorkspaceTab,
        PrevWorkspaceTab,
        NextSession,
        PrevSession,
        NextSidebarSession,
        PrevSidebarSession,
        FocusComposer,
        SearchConversations,
        StopSession,
        ToggleProviders,
        ToggleSidebar,
        Quit,
        QuitCompletely,
        ToggleFiles,
        ToggleTerminal,
        CloseTab,
        CloseWorkspaceTab,
        OpenChanges,
        OpenFiles,
        OpenSideChat,
        OpenSideHistory,
        SelectProvider,
        SelectModel,
        SelectEffort,
        SelectBranch,
        Tab1,
        Tab2,
        Tab3,
        Tab4,
        Tab5,
        Tab6,
        Tab7,
        Tab8,
        Tab9,
        NavigateBack,
        NavigateForward,
        ToggleTerminalPanel,
        About,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
    ]
);

/// Backends for the session manager.
///
/// `DEL_BACKEND=mock` selects the scripted mock for both providers; otherwise
/// the real Codex and Claude backends drive the user's installed CLIs.
pub fn backends() -> Vec<Arc<dyn AgentBackend>> {
    if std::env::var("DEL_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("mock")) {
        let delay = std::env::var("DEL_MOCK_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or_default();
        let models = if std::env::var("DEL_MOCK_MODELS").is_ok_and(|value| value == "1") {
            (0..12)
                .map(|index| del_agent::ModelInfo {
                    id: format!("mock-model-{index}"),
                    name: match index {
                        0 => "Mock Balanced".into(),
                        1 => "Mock Quick".into(),
                        2 => "Mock Extended — a deliberately long model name".into(),
                        _ => format!("Mock model {:02}", index + 1),
                    },
                    description: "Synthetic model for UI development".into(),
                    efforts: ["low", "medium", "high", "xhigh"]
                        .into_iter()
                        .map(del_agent::EffortOption::named)
                        .collect(),
                    default_effort: Some("medium".into()),
                    fast_tier: Some("priority".into()),
                    is_default: index == 0,
                })
                .collect()
        } else {
            Vec::new()
        };
        return Provider::ALL
            .iter()
            .map(|&p| {
                Arc::new(
                    MockBackend::new(p)
                        .with_delay(delay)
                        .with_models(models.clone()),
                ) as Arc<dyn AgentBackend>
            })
            .collect();
    }
    vec![
        Arc::new(del_codex::CodexBackend::new(Default::default())),
        Arc::new(del_acp::AcpBackend::default()),
        Arc::new(del_opencode::OpenCodeBackend::default()),
        // Keep permission selection explicit in each session's launch arguments.
        Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
            setting_sources: None,
            ..Default::default()
        })),
    ]
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SidebarWidths {
    left: f32,
    right: f32,
    /// Height of the bottom terminal panel.
    bottom: f32,
}

const LEFT_SIDEBAR_MIN: f32 = 180.0;
const LEFT_SIDEBAR_MAX: f32 = 320.0;
const LEFT_SIDEBAR_COLLAPSE_THRESHOLD: f32 = LEFT_SIDEBAR_MIN - 80.0;
const LEFT_SIDEBAR_REOPEN_THRESHOLD: f32 = LEFT_SIDEBAR_COLLAPSE_THRESHOLD + 32.0;
const LEFT_SIDEBAR_RESIZE_TARGET: f32 = 16.0;

fn left_sidebar_open_for_drag(requested: f32, open: bool) -> bool {
    if open {
        requested > LEFT_SIDEBAR_COLLAPSE_THRESHOLD
    } else {
        requested >= LEFT_SIDEBAR_REOPEN_THRESHOLD
    }
}

impl Default for SidebarWidths {
    fn default() -> Self {
        Self {
            left: sidebar::SIDEBAR_WIDTH.clamp(LEFT_SIDEBAR_MIN, LEFT_SIDEBAR_MAX),
            right: right_panel::PANEL_WIDTH,
            bottom: workbench::PANEL_DEFAULT_HEIGHT,
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
        Self::read(&Self::path())
    }

    fn read(path: &std::path::Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|sizes| sizes.left.is_finite() && sizes.right.is_finite())
            .map(|sizes| Self {
                left: sizes.left.clamp(LEFT_SIDEBAR_MIN, LEFT_SIDEBAR_MAX),
                ..sizes
            })
            .unwrap_or_default()
    }

    fn save(mut self) -> std::io::Result<()> {
        self.left = self.left.clamp(LEFT_SIDEBAR_MIN, LEFT_SIDEBAR_MAX);
        sessions::write_atomic(&Self::path(), &serde_json::to_vec(&self)?, 0o644)
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
    let min = if right {
        right_panel::PANEL_MIN_WIDTH
    } else {
        LEFT_SIDEBAR_MIN
    };
    let max = if right { 800.0_f32 } else { LEFT_SIDEBAR_MAX };
    requested.clamp(min, max.min(viewport - other - 260.0 - 20.0).max(min))
}

/// A right panel kept alive for an open tab that is not currently selected.
struct CachedRight {
    panel: Entity<RightPanel>,
    open: bool,
    /// Store key (`chat:…`/`terminal:…`); drafts have none.
    key: Option<String>,
}

struct AppShell {
    updater: Entity<app_updates::AppUpdater>,
    shell_focus: FocusHandle,
    focus_watch: keyboard_control::FocusWatch,
    settings_focused: bool,
    /// Settings has been dismissed and is fading out, like Cmd-K.
    settings_closing: Option<std::time::Instant>,
    zoom_percent: u16,
    state: Entity<AppState>,
    composer: Entity<TextareaState>,
    command_menu: Entity<command_menu::CommandMenu>,
    dictation: Option<dictation::Dictation>,
    dictation_generation: u64,
    model_picker: Entity<model_picker::ModelPicker>,
    branch_picker: Entity<branch_picker::BranchPicker>,
    provider_menu_open: bool,
    rename: Entity<InputState>,
    /// The session whose title is loaded into the rename field.
    renaming_loaded: Option<SessionId>,
    project_renaming_loaded: Option<std::path::PathBuf>,
    sidebar_widths: SidebarWidths,
    sidebar_widths_generation: u64,
    window_bounds_generation: u64,
    /// The Command hold behind the tab-number hints.
    hint_hold: keybinds::HintHold,
    /// A one-second repaint runs only while the open chat has an active goal.
    goal_ticking: bool,
    resize_origin: std::rc::Rc<std::cell::Cell<f32>>,
    tab_drag_origin: std::rc::Rc<std::cell::Cell<Pixels>>,
    tab_strip_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    page_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    sidebar_open: bool,
    sidebar_visible: bool,
    sidebar_toggles: u64,
    sidebar_motion: panel_motion::Reveal,
    tab_scroll: ScrollHandle,
    tab_reveal: Option<(u64, std::time::Instant)>,
    workspaces: HashMap<u64, Entity<workspace_tabs::WorkspaceTabs>>,
    workspace_previous: Option<u64>,
    pending_peer: Option<SessionId>,
    tab_zone: std::rc::Rc<std::cell::Cell<tab_zone::Zone>>,
    right: Entity<RightPanel>,
    browser_preview: Entity<browser_preview::BrowserPreview>,
    right_owner: u64,
    right_key: Option<String>,
    right_cache: HashMap<u64, CachedRight>,
    /// The panel last shown; only it needs hiding when the selection changes.
    right_previous: Option<Entity<RightPanel>>,
    right_store: right_panel::PanelStore,
    right_store_path: std::path::PathBuf,
    right_save_generation: u64,
    /// Where the right panel is heading (open or closed).
    right_open: bool,
    /// Whether it is on screen: stays true while the close animation plays.
    right_visible: bool,
    /// Bumped on every toggle so each one starts a fresh animation.
    right_toggles: u64,
    right_motion: panel_motion::Reveal,
    /// One chat view per session opened in this run.
    chats: HashMap<SessionId, Entity<ChatView>>,
    search_index: Arc<std::sync::Mutex<conversation_search::Index>>,
    usage: Entity<usage::Usage>,
    search_open: bool,
    search_closing: Option<std::time::Instant>,
    terminal_search_generation: u64,
    composer_owner: u64,
    composer_drafts: HashMap<u64, gpui_kit::component::input::InputContent>,
    goal_modes: HashMap<u64, del_agent::ComposerCommand>,
    goal_expanded: HashSet<u64>,
    goal_controls: HashSet<SessionId>,
    composer_attachments: HashMap<u64, Vec<del_agent::Attachment>>,
    attachment_scroll: ScrollHandle,
    metrics: ShellMetrics,
    ambient: Entity<ambient::LifeBackground>,
    terminals: HashMap<SessionId, Entity<TerminalHost>>,
    mode_terminals: HashMap<SessionId, session_modes::ModeTerminal>,
    mode_switching: HashSet<SessionId>,
    terminal_store: TerminalStore,
    terminal_attempts: HashSet<SessionId>,
    terminal_opening: HashSet<SessionId>,
    terminal_identity_pending: HashSet<SessionId>,
    terminal_update_restarts: HashSet<SessionId>,
    terminal_update_restarted: HashSet<(SessionId, u32)>,
    mcp: Option<McpBridge>,
    questions: VecDeque<PendingQuestion>,
    notices: VecDeque<(SessionId, String, String)>,
    reported: HashMap<SessionId, String>,
    mode_focus: Option<(Selection, SessionMode)>,
    mode_slide_from: f32,
    mode_slide_started: Option<std::time::Instant>,
    caffeinate: Option<std::process::Child>,
    dragging_tab: Option<u64>,
    drag_target: Option<usize>,
    /// Drag shifts of the top tabs, by uid; each retargets from where it is.
    tab_shifts: std::cell::RefCell<HashMap<u64, Tween>>,
    /// Width of each top tab's close slot, by uid.
    close_slots: std::cell::RefCell<HashMap<u64, Tween>>,
    hovered_tab: Option<u64>,
    /// Visited sessions and tabs, for back/forward and the recent menu.
    nav: nav_history::NavHistory,
    /// The directory whose Git state was last refreshed for the status bar.
    git_refreshed: Option<std::path::PathBuf>,
    processes: status_processes::Snapshot,
    status_refreshing: bool,
    bottom: workbench::BottomPanel,
}

impl AppShell {
    fn resize_handle(
        &self,
        right: bool,
        width: f32,
        other: f32,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let drag = SidebarResize {
            right,
            start_x: self.resize_origin.clone(),
            width,
            other,
        };
        let start_x = drag.start_x.clone();
        let shell = cx.entity();
        div()
            .id(if right {
                "resize-right-sidebar"
            } else {
                "resize-left-sidebar"
            })
            .absolute()
            .top_0()
            .bottom_0()
            .w(zpx(3.0))
            .cursor_col_resize()
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                start_x.set(event.position.x.as_f32());
                cx.stop_propagation();
            })
            .on_drag(drag, move |drag, _, window, cx| {
                // Include the motion that crossed GPUI's drag threshold, even
                // when a quick drag releases before the next move event.
                shell.update(cx, |this, cx| this.resize_sidebar(drag, window, cx));
                cx.new(|_| ResizePreview)
            })
    }

    fn resize_sidebar(&mut self, drag: &SidebarResize, window: &Window, cx: &mut Context<Self>) {
        let delta =
            del_core::zoom::unscaled(window.mouse_position().x.as_f32() - drag.start_x.get());
        let requested = drag.width + if drag.right { -delta } else { delta };
        if !drag.right {
            // Keep using the original drag anchor after collapse. Separate
            // thresholds let the same held drag reopen without edge flicker.
            let open = left_sidebar_open_for_drag(requested, self.sidebar_open);
            self.set_sidebar(open, cx);
            if !open {
                return;
            }
        }
        let width = sidebar_width(
            requested,
            drag.right,
            del_core::zoom::unscaled(window.viewport_size().width.as_f32()),
            drag.other,
        );
        if drag.right {
            self.sidebar_widths.right = width;
        } else {
            self.sidebar_widths.left = width;
        }
        self.save_sidebar_widths_soon(cx);
        cx.notify();
    }

    fn open_peer(&mut self, requested: &SessionId, cx: &mut Context<Self>) {
        let manager = self.state.read(cx).backend().manager;
        let id = manager.resolve_peer(requested);
        let owner = self
            .workspaces
            .iter()
            .find(|(_, w)| w.read(cx).contains_peer(&id, cx))
            .map(|(uid, _)| *uid)
            .or_else(|| {
                self.right_cache
                    .iter()
                    .find(|(_, r)| r.panel.read(cx).contains_peer(&id, cx))
                    .map(|(uid, _)| *uid)
            })
            .or_else(|| {
                self.right
                    .read(cx)
                    .contains_peer(&id, cx)
                    .then_some(self.right_owner)
            });
        if let Some(uid) = owner {
            let tab = self
                .state
                .read(cx)
                .tabs
                .iter()
                .find(|tab| self.state.read(cx).uid_of(tab) == uid)
                .cloned();
            if let Some(tab) = tab {
                self.state
                    .update(cx, |state, cx| state.select_tab(&tab, cx));
            }
            self.pending_peer = Some(id);
        } else {
            self.state.update(cx, |state, cx| {
                let target = if manager.is_temporary(&id) {
                    manager.peer_parent(&id).unwrap_or(id.clone())
                } else {
                    id.clone()
                };
                if manager.session_record(&target).is_ok() {
                    state.select(target, cx);
                } else {
                    state.select_terminal(target, cx);
                }
            });
            self.pending_peer = Some(id);
        }
        cx.notify();
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
                    state.report_error(format!("Could not keep the Mac awake: {error}"));
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
        cx.notify();
    }

    fn move_tab_drag(&mut self, target: usize, cx: &mut Context<Self>) {
        if self.drag_target == Some(target) {
            return;
        }
        self.drag_target = Some(target);
        crate::haptics::tick();
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
        // The reorder already moved the tabs; shifts must not ease back.
        self.tab_shifts.borrow_mut().clear();
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
                state.report(state::notices::Severity::Warning, format!("Could not save terminal history: {error}"));
                cx.notify();
            });
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Ask anything…")
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
        let updater = cx.new(app_updates::AppUpdater::new);
        cx.observe(&updater, |_, _, cx| cx.notify()).detach();
        updater.update(cx, |updater, cx| updater.watch_sessions(&state, cx));
        state.update(cx, |state, cx| state.restore_draft_owners(cx));
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        updater.update(cx, |updater, cx| updater.check(false, cx));

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
        let browser_preview = cx.new(|_| browser_preview::BrowserPreview::default());
        cx.subscribe_in(
            &browser_preview,
            window,
            |this, _, event: &browser_preview::OpenBrowser, window, cx| {
                let Some(_) = this.find_agent_browser(&event.session, &event.tab_id, cx) else {
                    return;
                };
                this.state
                    .update(cx, |state, cx| state.select_tab(&event.session, cx));
                this.sync_right(window, cx);
                let found = this.right.update(cx, |panel, cx| {
                    panel.show_browser_id(&event.tab_id, window, cx)
                });
                if !found && let Some(workspace) = this.workspace(cx) {
                    let panel = workspace.update(cx, |workspace, cx| {
                        workspace.take_browser_tab(&event.tab_id, window, cx)
                    });
                    if let Some(panel) = panel {
                        panel.update(cx, |panel, cx| {
                            panel.show_browser_id(&event.tab_id, window, cx);
                        });
                        this.right
                            .update(cx, |right, cx| right.accept_tab(panel, window, cx));
                    }
                }
                this.set_right(true, cx);
                this.tab_zone.set(tab_zone::Zone::Right);
                cx.notify();
            },
        )
        .detach();
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
                    account_id: r.launch.account_id.clone(),
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
            s.sort_terminal_rows();
            if let Some(error) = &terminal_store.error {
                s.report_error(error);
            }
        });
        state.update(cx, |s, cx| s.restore_spaces(cx));
        let save_generation = std::rc::Rc::new(std::cell::Cell::new(0_u64));
        cx.observe(&state, move |_, state, cx| {
            let generation = save_generation.get().wrapping_add(1);
            save_generation.set(generation);
            let counter = save_generation.clone();
            let state = state.clone();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                if counter.get() == generation {
                    cx.update(|cx| {
                        if let Err(error) = state.read(cx).save_spaces() {
                            eprintln!("Could not save spaces: {error}");
                        }
                        let _ = state.read(cx).save_draft_owners();
                    });
                }
            })
            .detach();
        })
        .detach();
        let branch_picker =
            cx.new(|cx| branch_picker::BranchPicker::new(state.clone(), window, cx));
        let model_picker = cx.new(|cx| model_picker::ModelPicker::new(state.clone(), window, cx));
        let command_menu =
            cx.new(|cx| command_menu::CommandMenu::new(composer.clone(), state.clone(), cx));
        cx.observe(&command_menu, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &command_menu,
            window,
            |this, _, event: &command_menu::GoalSelected, window, cx| {
                this.begin_goal(event.0.clone(), window, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &command_menu,
            window,
            |this, _, _: &command_menu::ModeSelected, window, cx| {
                this.submit_mode_command(window, cx);
            },
        )
        .detach();
        let usage = cx.new(|_| usage::Usage::new(state.clone()));
        cx.observe(&usage, |_, _, cx| cx.notify()).detach();
        let mut shell = Self {
            updater,
            shell_focus: cx.focus_handle(),
            focus_watch: Default::default(),
            settings_focused: false,
            settings_closing: None,
            zoom_percent: del_core::zoom::percent(),
            state,
            composer,
            command_menu,
            model_picker,
            branch_picker,
            provider_menu_open: false,
            rename,
            renaming_loaded: None,
            project_renaming_loaded: None,
            sidebar_widths: SidebarWidths::load(),
            sidebar_widths_generation: 0,
            window_bounds_generation: 0,
            hint_hold: Default::default(),
            goal_ticking: false,
            resize_origin: Default::default(),
            tab_drag_origin: Default::default(),
            tab_strip_bounds: Default::default(),
            page_bounds: Default::default(),
            sidebar_open: true,
            sidebar_visible: true,
            sidebar_toggles: 0,
            sidebar_motion: Default::default(),
            tab_scroll: ScrollHandle::new(),
            tab_reveal: None,
            workspaces: HashMap::new(),
            workspace_previous: None,
            pending_peer: None,
            tab_zone: Default::default(),
            right,
            browser_preview,
            right_owner: 0,
            right_key: None,
            right_cache: HashMap::new(),
            right_previous: None,
            right_store,
            right_store_path,
            right_save_generation: 0,
            right_open: false,
            right_visible: false,
            right_toggles: 0,
            right_motion: Default::default(),
            chats: HashMap::new(),
            search_index: search_ui::new_index(),
            usage,
            search_open: false,
            search_closing: None,
            terminal_search_generation: 0,
            dictation: None,
            dictation_generation: 0,
            composer_owner: 0,
            composer_drafts: HashMap::new(),
            goal_modes: HashMap::new(),
            goal_expanded: HashSet::new(),
            goal_controls: HashSet::new(),
            composer_attachments: HashMap::new(),
            attachment_scroll: ScrollHandle::new(),
            metrics: ShellMetrics::default(),
            ambient: cx.new(|cx| ambient::LifeBackground::new(window, cx)),
            terminals: HashMap::new(),
            mode_terminals: HashMap::new(),
            mode_switching: HashSet::new(),
            terminal_store,
            terminal_attempts: HashSet::new(),
            terminal_opening: HashSet::new(),
            terminal_identity_pending: HashSet::new(),
            terminal_update_restarts: HashSet::new(),
            terminal_update_restarted: HashSet::new(),
            mcp: None,
            questions: VecDeque::new(),
            notices: VecDeque::new(),
            reported: HashMap::new(),
            mode_focus: None,
            mode_slide_from: 0.0,
            mode_slide_started: None,
            caffeinate: None,
            dragging_tab: None,
            drag_target: None,
            tab_shifts: Default::default(),
            close_slots: Default::default(),
            hovered_tab: None,
            nav: Default::default(),
            git_refreshed: None,
            processes: Default::default(),
            status_refreshing: false,
            bottom: Default::default(),
        };
        shell.sidebar_motion.settle(true);
        shell.start_modes(window, cx);
        shell.watch_terminal_titles(cx);
        shell.watch_right(cx);
        shell.start_status_ticker(cx);
        let mut background_requests = session_lifecycle::install();
        cx.spawn(async move |this, cx| {
            while background_requests.recv().await.is_some() {
                if this
                    .update(cx, |this, cx| {
                        this.cancel_dictation(cx);
                        this.save_workspace(cx);
                        this.state
                            .update(cx, |state, cx| state.window_activation_changed(false, cx));
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        window.on_window_should_close(cx, |_, cx| {
            if session_lifecycle::can_background() {
                session_lifecycle::request_quit(cx);
                false
            } else {
                true
            }
        });
        shell.state.update(cx, |state, cx| {
            state.window_activation_changed(window.is_window_active(), cx)
        });
        cx.observe_window_bounds(window, |this, window, cx| {
            this.save_window_bounds_soon(window, cx);
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.hide_shortcut_hints(cx);
            }
            if window.is_window_active() {
                this.updater
                    .update(cx, |updater, cx| updater.check(false, cx));
            }
            this.state.update(cx, |state, cx| {
                state.window_activation_changed(window.is_window_active(), cx);
            });
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.cancel_dictation(cx);
            this.save_workspace(cx);
            this.state.read(cx).shutdown();
            async {}
        })
        .detach();
        shell
    }

    fn save_workspace(&mut self, cx: &mut Context<Self>) {
        self.right.update(cx, |panel, cx| panel.save_side_chat(cx));
        for cached in self.right_cache.values() {
            cached
                .panel
                .update(cx, |panel, cx| panel.save_side_chat(cx));
        }
        for workspace in self.workspaces.values() {
            workspace.update(cx, |workspace, cx| workspace.save_side_chats(cx));
        }
        self.remember_right(cx);
        self.persist_right(cx);
        if let Err(error) = self.sidebar_widths.save() {
            eprintln!("Could not save sidebar widths: {error}");
        }
        let _ = self.state.read(cx).save_spaces();
        if let Err(error) = self.state.read(cx).save_draft_owners() {
            eprintln!("Could not save draft owners: {error}");
        }
    }

    fn goal_active(&self, cx: &App) -> bool {
        self.open_chat(cx).is_some_and(|chat| {
            chat.read(cx)
                .goal()
                .is_some_and(|goal| goal.status == del_agent::GoalStatus::Active)
        })
    }

    /// Refresh the goal's elapsed time each second, only while one is active.
    fn ensure_goal_ticker(&mut self, cx: &mut Context<Self>) {
        if self.goal_ticking || !self.goal_active(cx) {
            return;
        }
        self.goal_ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let active = this.update(cx, |this, cx| {
                    let active = this.goal_active(cx);
                    if active {
                        cx.notify();
                    } else {
                        this.goal_ticking = false;
                    }
                    active
                });
                if !matches!(active, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Moving or resizing reports bounds every frame; write the settled frame
    /// once, off the UI thread.
    fn save_window_bounds_soon(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(path) = state::window_bounds::path() else {
            return;
        };
        let display = window
            .display(cx)
            .and_then(|display| display.uuid().ok())
            .map(|uuid| uuid.to_string());
        let saved = state::window_bounds::Saved::from_window(window.window_bounds(), display);
        self.window_bounds_generation += 1;
        let generation = self.window_bounds_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            if this
                .update(cx, |this, _| this.window_bounds_generation == generation)
                .unwrap_or(false)
            {
                let _ = cx
                    .background_executor()
                    .spawn(async move { state::window_bounds::write(&path, &saved) })
                    .await;
            }
        })
        .detach();
    }

    /// Hides the tab-number hints at once and cancels a pending hold.
    fn hide_shortcut_hints(&mut self, cx: &mut Context<Self>) {
        self.hint_hold.cancel();
        if self.state.read(cx).shortcut_hints {
            self.state.update(cx, |state, cx| {
                state.shortcut_hints = false;
                cx.notify();
            });
        }
    }

    /// Divider drags update widths every frame; coalesce disk writes like the right panel.
    fn save_sidebar_widths_soon(&mut self, cx: &mut Context<Self>) {
        self.sidebar_widths_generation += 1;
        let generation = self.sidebar_widths_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.sidebar_widths_generation == generation
                    && let Err(error) = this.sidebar_widths.save()
                {
                    this.state.update(cx, |state, cx| {
                        state.report(state::notices::Severity::Warning, format!("Could not save sidebar widths: {error}"));
                        cx.notify();
                    });
                }
            });
        })
        .detach();
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
        if self.mode_terminals.contains_key(&id) || self.mode_switching.contains(&id) {
            return;
        }
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
            this.state.update(cx, |state, cx| {
                state.keep_tab(&tab, cx);
                state.mark_visible_chat_read(cx);
            });
        })
        .detach();
        cx.subscribe(&chat, |this, _, event: &chat::OpenPeer, cx| {
            this.open_peer(&event.0, cx);
        })
        .detach();
        cx.subscribe(&chat, |this, _, event: &chat::OpenLink, cx| {
            let cwd = this.state.read(cx).effective().2;
            match chat::links::classify(&event.0, &cwd) {
                chat::links::LinkTarget::Local(path) => {
                    this.set_right(true, cx);
                    this.right.update(cx, |right, cx| right.preview_file(path, cx));
                }
                chat::links::LinkTarget::External(url) => cx.open_url(&url),
                chat::links::LinkTarget::Blocked => this.state.update(cx, |state, cx| {
                    state.report_error(format!(
                        "Del did not open {}. Only web (http, https) and mail links open outside Del.",
                        event.0.chars().take(200).collect::<String>()
                    ));
                    cx.notify();
                }),
            }
        }).detach();
        self.chats.insert(id, chat);
    }

    fn set_sidebar(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open == open {
            return;
        }
        self.sidebar_open = open;
        self.sidebar_motion
            .set(open, cx.reduce_motion(), std::time::Instant::now());
        self.sidebar_toggles += 1;
        if cx.reduce_motion() {
            self.sidebar_visible = open;
        } else if open {
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
        self.right_motion
            .set(open, cx.reduce_motion(), std::time::Instant::now());
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

    fn send_side_to_main(&mut self, event: &side_chat::SendToMain, cx: &mut Context<Self>) {
        if !self
            .state
            .read(cx)
            .backend()
            .manager
            .list_sessions()
            .iter()
            .any(|r| r.id == event.parent)
        {
            self.state.update(cx, |state, cx| { state.report_error("The main conversation is no longer available. Copy the side reply before closing it."); cx.notify(); });
            return;
        }
        self.state
            .update(cx, |state, cx| state.select(event.parent.clone(), cx));
        self.ensure_chat(cx);
        if let Some(chat) = self.chats.get(&event.parent) {
            let input = del_agent::UserInput::text(format!(
                "Use this conclusion from our side chat as context for the main task:\n\n{}",
                event.text
            ));
            chat.update(cx, |chat, cx| chat.submit_input(input, false, cx));
        }
    }

    fn watch_right(&mut self, cx: &mut Context<Self>) {
        cx.subscribe(&self.right, |this, _, event: &side_chat::SendToMain, cx| {
            this.send_side_to_main(event, cx)
        })
        .detach();
        cx.subscribe(&self.right, |this, _, event: &chat::OpenPeer, cx| {
            this.open_peer(&event.0, cx);
        })
        .detach();
        cx.subscribe(&self.right, |_, _, event: &chat::OpenLink, cx| {
            if matches!(
                chat::links::classify(&event.0, std::path::Path::new("/")),
                chat::links::LinkTarget::External(_)
            ) {
                cx.open_url(&event.0);
            }
        })
        .detach();
        cx.observe(&self.right, |this, panel, cx| {
            if this.right == panel {
                this.remember_right(cx);
                // Recompute floating-preview visibility when a Browser pane changes.
                cx.notify();
            }
        })
        .detach();
    }

    fn remember_right(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.state.read(cx).save_draft_owners() {
            eprintln!("Could not save draft owners: {error}");
        }
        self.right_cache.insert(
            self.right_owner,
            CachedRight {
                panel: self.right.clone(),
                open: self.right_open,
                key: self.right_key.clone(),
            },
        );
        let Some(key) = self.right_key.clone() else {
            return;
        };
        if self
            .right_store
            .set(key, self.right.read(cx).snapshot(self.right_open, cx))
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

    /// Drop the panel cached for a tab that is no longer open: persist its state,
    /// then free its shells, web views and watch loops.
    fn evict_right(&mut self, uid: u64, cx: &mut Context<Self>) {
        let Some(cached) = self.right_cache.remove(&uid) else {
            return;
        };
        if let Some(key) = cached.key
            && self
                .right_store
                .set(key, cached.panel.read(cx).snapshot(cached.open, cx))
        {
            self.persist_right(cx);
        }
        cached.panel.update(cx, |panel, cx| panel.release(cx));
        if self.right_previous.as_ref() == Some(&cached.panel) {
            self.right_previous = None;
        }
    }

    fn persist_right(&self, cx: &mut Context<Self>) {
        if let Err(error) = self.right_store.save(&self.right_store_path) {
            self.state.update(cx, |state, cx| {
                state.report(state::notices::Severity::Warning, format!("Could not save sidebar state: {error}"));
                cx.notify();
            });
        }
    }

    fn delete_browser_profile(&mut self, profile: String, cx: &mut Context<Self>) {
        if let Err(error) = crate::browser::record_deletion(&profile, true) {
            self.state.update(cx, |state, cx| {
                state.report(state::notices::Severity::Warning, format!("Could not schedule browser data deletion: {error}"));
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
                    state.report(
                        state::notices::Severity::Info,
                        "Browser data cleanup is pending; Del will retry at next launch.",
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
            let cached = self.right_cache.remove(&uid).map(|cached| {
                let snapshot = cached.panel.read(cx).snapshot(false, cx);
                cached.panel.update(cx, |panel, cx| panel.release(cx));
                snapshot
            });
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
            Selection::Draft(id) => (
                Tab::Draft(*id),
                Some(format!("draft:{id}:{}", state.effective().2.display())),
            ),
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
        let open_uids: std::collections::HashSet<u64> = state
            .all_open_tabs()
            .iter()
            .map(|tab| state.uid_of(tab))
            .collect();
        if self.right_owner != owner {
            self.remember_right(cx);
            let newly_created = !self.right_cache.contains_key(&owner);
            let (panel, open) = if let Some(cached) = self.right_cache.get(&owner) {
                (cached.panel.clone(), cached.open)
            } else {
                let saved = key
                    .as_deref()
                    .map(|key| self.right_store.get(key))
                    .unwrap_or_default();
                let open = saved.restore_open();
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
            self.right_motion.settle(open);
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
        // Panels for closed tabs would otherwise keep PTYs, WKWebViews and their
        // 2 s Changes watch alive for the rest of the run. Uid 0 belongs to
        // the startup composer.
        let stale: Vec<u64> = self
            .right_cache
            .keys()
            .copied()
            .filter(|uid| *uid != 0 && *uid != self.right_owner && !open_uids.contains(uid))
            .collect();
        for uid in stale {
            self.evict_right(uid, cx);
        }
        let context = self
            .state
            .read(cx)
            .side_chat_context(self.state.downgrade());
        self.right.update(cx, |panel, cx| {
            panel.set_side_context(context);
            panel.set_root(root, window, cx);
            if self.right_open {
                panel.prepare_to_show(window, cx);
            }
        });
    }

    fn find_agent_browser(&self, session: &Tab, id: &str, cx: &App) -> Option<Entity<RightPanel>> {
        let owner = self.state.read(cx).uid_of(session);
        if !self.state.read(cx).all_open_tabs().contains(session) {
            return None;
        }
        let root = if owner == self.right_owner {
            Some(&self.right)
        } else {
            self.right_cache.get(&owner).map(|cached| &cached.panel)
        };
        root.and_then(|panel| RightPanel::find_browser_panel(panel, id, cx))
            .or_else(|| {
                self.workspaces
                    .get(&owner)
                    .and_then(|workspace| workspace.read(cx).find_browser_panel(id, cx))
            })
    }

    fn workspace(&self, cx: &App) -> Option<Entity<workspace_tabs::WorkspaceTabs>> {
        self.workspaces.get(&self.composer_owner(cx)).cloned()
    }

    fn sync_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.composer_owner(cx);
        let root = self.state.read(cx).effective().2;
        let state = self.state.read(cx);
        let open: std::collections::HashSet<_> = state
            .all_open_tabs()
            .iter()
            .map(|t| state.uid_of(t))
            .collect();
        let stale: Vec<_> = self
            .workspaces
            .keys()
            .copied()
            .filter(|uid| *uid != owner && !open.contains(uid))
            .collect();
        for uid in stale {
            if let Some(workspace) = self.workspaces.remove(&uid) {
                workspace.update(cx, |w, cx| w.release(cx));
            }
        }
        self.workspaces.entry(owner).or_insert_with(|| {
            let workspace =
                cx.new(|cx| workspace_tabs::WorkspaceTabs::new(root.clone(), window, cx));
            let save_generation = std::rc::Rc::new(std::cell::Cell::new(0_u64));
            cx.observe(&workspace, move |_, workspace, cx| {
                let generation = save_generation.get().wrapping_add(1);
                save_generation.set(generation);
                let pending = save_generation.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    if pending.get() != generation {
                        return;
                    }
                    let _ = this.update(cx, |this, cx| {
                        if let Err(error) = this.state.read(cx).save_draft_owners() {
                            eprintln!("Could not save draft owners: {error}");
                        }
                        workspace.read(cx).persist(cx);
                    });
                })
                .detach();
                cx.notify();
            })
            .detach();
            cx.subscribe(&workspace, |this, _, event: &side_chat::SendToMain, cx| {
                this.send_side_to_main(event, cx)
            })
            .detach();
            cx.subscribe(&workspace, |this, _, event: &chat::OpenPeer, cx| {
                this.open_peer(&event.0, cx);
            })
            .detach();
            cx.subscribe(&workspace, |_, _, event: &chat::OpenLink, cx| {
                if matches!(
                    chat::links::classify(&event.0, std::path::Path::new("/")),
                    chat::links::LinkTarget::External(_)
                ) {
                    cx.open_url(&event.0);
                }
            })
            .detach();
            workspace
        });
        if self.workspace_previous != Some(owner) {
            if let Some(previous) = self
                .workspace_previous
                .and_then(|id| self.workspaces.get(&id))
            {
                previous.update(cx, |w, cx| w.set_visible(false, cx));
            }
            self.workspace_previous = Some(owner);
        }
        if let Some(workspace) = self.workspace(cx) {
            let visible = !self.search_open && self.state.read(cx).panel == Panel::Chat;
            let context = self
                .state
                .read(cx)
                .side_chat_context(self.state.downgrade());
            let storage_key = match &self.state.read(cx).selection {
                Selection::Session(id) => format!("chat:{}", id.as_str()),
                Selection::Terminal(id) => format!("terminal:{}", id.as_str()),
                Selection::Draft(id) => format!("draft:{id}:{}", root.display()),
            };
            workspace.update(cx, |w, cx| {
                w.set_side_context(context);
                w.set_storage_key(storage_key, window, cx);
                w.set_root(root, window, cx);
                w.set_visible(visible, cx);
            });
        }
    }

    fn active_tab_zone(&self) -> tab_zone::Zone {
        self.tab_zone.get().active(self.right_open)
    }

    fn open_composer_picker(
        &mut self,
        picker: keybinds::Picker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_open || self.state.read(cx).panel != Panel::Chat {
            return;
        }
        if self.active_tab_zone() == tab_zone::Zone::Right {
            self.right.update(cx, |panel, cx| {
                panel.open_composer_picker(picker, window, cx)
            });
            return;
        }
        if let Some(workspace) = self.workspace(cx)
            && !workspace.read(cx).is_chat()
        {
            workspace.update(cx, |workspace, cx| {
                workspace.open_composer_picker(picker, window, cx)
            });
            return;
        }
        if matches!(self.state.read(cx).selection, Selection::Terminal(_))
            || matches!(&self.state.read(cx).selection, Selection::Session(id)
                if self.mode_terminals.contains_key(id) || self.mode_switching.contains(id))
        {
            return;
        }
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        self.provider_menu_open = false;
        if !matches!(picker, keybinds::Picker::Model | keybinds::Picker::Effort) {
            self.model_picker
                .update(cx, |picker, cx| picker.close_picker(cx));
        }
        if picker != keybinds::Picker::Branch {
            self.branch_picker
                .update(cx, |picker, cx| picker.close_picker(cx));
        }
        cx.notify();
        match picker {
            keybinds::Picker::Provider if !self.state.read(cx).is_starting() => {
                self.provider_menu_open = true;
                cx.notify();
            }
            keybinds::Picker::Model | keybinds::Picker::Effort => {
                self.model_picker.update(cx, |picker_view, cx| {
                    picker_view.open_picker(picker == keybinds::Picker::Effort, window, cx)
                })
            }
            keybinds::Picker::Branch => self
                .branch_picker
                .update(cx, |picker, cx| picker.open_picker(window, cx)),
            _ => {}
        }
    }

    fn new_inner_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace(cx).is_none() {
            self.sync_workspaces(window, cx);
        }
        if self.workspace(cx).is_some() {
            self.set_right(true, cx);
            self.tab_zone.set(tab_zone::Zone::Right);
            tab_zone::Zone::Right.new_tab(&self.state, &self.right, window, cx);
        }
    }

    fn open_inner_tool(
        &mut self,
        pane: right_panel::Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).panel != Panel::Chat {
            return;
        }
        if let Some(workspace) = self.workspace(cx) {
            self.set_right(true, cx);
            self.tab_zone.set(tab_zone::Zone::Right);
            tab_zone::Zone::Right.open_tool(pane, &workspace, &self.right, window, cx);
        }
    }

    fn go_tab(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_zone.set(tab_zone::Zone::Main);
        self.state.update(cx, |s, cx| s.select_nth(n, cx));
        self.focus_composer(window, cx);
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            return;
        }
        self.tab_zone.set(tab_zone::Zone::Main);
        if let Selection::Session(id) = &self.state.read(cx).selection
            && let Some(terminal) = self.mode_terminals.get(id)
        {
            terminal.host.update(cx, |host, cx| host.focus(window, cx));
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
            Selection::Session(id)
                if !self.mode_terminals.contains_key(id) && !self.mode_switching.contains(id) =>
            {
                self.chats.get(id).cloned()
            }
            Selection::Session(_) => None,
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
        if self.submit_mode_command(window, cx) {
            return;
        }
        if self
            .command_menu
            .update(cx, |menu, cx| menu.submit_control(window, cx))
        {
            return;
        }
        let owner = self.composer_owner(cx);
        let text = self.composer.read(cx).value().trim().to_string();
        let attachments = self
            .composer_attachments
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        if self.goal_modes.contains_key(&owner) && !attachments.is_empty() {
            self.state.update(cx, |state, cx| {
                state.error = Some("Send attachments in a message before setting a goal.".into());
                cx.notify();
            });
            return;
        }
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
                    chat.update(cx, |chat, cx| chat.deliver_queued(id, true, cx));
                }
            }
            return;
        }
        if self.state.read(cx).terminal_draft().is_some() {
            let text = if self.goal_modes.contains_key(&owner) {
                format!("/goal {text}")
            } else {
                text
            };
            self.submit_terminal_prompt(&text, &attachments, window, cx);
            self.goal_modes.remove(&owner);
            self.sync_goal_placeholder(window, cx);
            return;
        }
        if chat.as_ref().is_some_and(|chat| chat.read(cx).is_loading())
            || self.state.read(cx).is_starting()
        {
            return;
        }
        let command = self
            .goal_modes
            .get(&owner)
            .cloned()
            .or_else(|| self.command_menu.read(cx).selected_command(&text, cx));
        let input = del_agent::UserInput {
            skills: command_menu::selected_skills(&self.composer.read(cx).content()),
            command,
            text,
            attachments,
        };
        self.goal_modes.remove(&owner);
        self.sync_goal_placeholder(window, cx);
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
                st.selected_account().is_some()
                    || st.status_of(provider).is_some_and(providers::ready),
            )
        };
        let (shell, state_p, state_f) = (cx.entity(), self.state.clone(), self.state.clone());
        let shell_stop = shell.clone();
        let shell_attach = shell.clone();
        let shell_paste = shell.clone();
        let shell_voice = shell.clone();
        let shell_goal = shell.clone();
        let retry_shell = cx.entity();
        let owner = self.composer_owner(cx);
        let has_attachments = self
            .composer_attachments
            .get(&owner)
            .is_some_and(|images| !images.is_empty());
        let folder_branch = locked
            .then(|| {
                let state = self.state.read(cx);
                state
                    .project_info
                    .get(&cwd)
                    .map(|info| info.current.clone())
                    .filter(|b| !b.is_empty())
            })
            .flatten();
        let composer = ComposerCard::new(
            &self.composer,
            COMPOSER_MAX_WIDTH,
            provider,
            ready,
            self.model_picker.clone(),
            if locked {
                sessions::folder_name(&self.state.read(cx).project_root(&cwd))
            } else {
                sessions::folder_name(&cwd)
            },
            locked,
            running,
        )
        .folder_tooltip(folder_branch.map(|branch| format!("Branch: {branch}")))
        .accounts(self.state.clone())
        .provider_menu(self.provider_menu_open, {
            let shell = cx.entity();
            move |open, _, cx| {
                shell.update(cx, |this, cx| {
                    this.provider_menu_open = open;
                    cx.notify();
                })
            }
        })
        .goal_mode(self.goal_modes.contains_key(&self.composer_owner(cx)))
        .on_cancel_goal(move |window, cx| {
            shell_goal.update(cx, |this, cx| this.cancel_goal_mode(window, cx))
        })
        .dictation(
            self.dictation
                .as_ref()
                .map_or(dictation::Phase::Idle, |voice| voice.phase),
        )
        .on_voice(move |window, cx| {
            shell_voice.update(cx, |this, cx| this.toggle_dictation(window, cx))
        })
        .show_folder(false)
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
            .when(self.state.read(cx).pending_error().is_some(), |column| {
                column.child(
                    Button::new("retry-first-prompt")
                        .ghost()
                        .small()
                        .label("Retry message")
                        .on_click(move |_, window, cx| {
                            retry_shell.update(cx, |this, cx| {
                                if this.state.read(cx).mode() == SessionMode::Terminal {
                                    let input = this.state.read(cx).pending_input().cloned();
                                    if let Some(input) = input {
                                        this.submit_terminal_prompt(
                                            &input.text,
                                            &input.attachments,
                                            window,
                                            cx,
                                        );
                                    }
                                } else {
                                    this.state.update(cx, |state, cx| state.retry_pending(cx));
                                }
                            });
                        }),
                )
            })
            .child(self.render_followups(cx))
            .child(self.render_goal_bar(cx))
            .children(self.render_agent_messages(cx))
            .child(
                div()
                    .relative()
                    .child(composer)
                    .child(self.ambient.read(cx).region_probe(0, zpx(14.0)))
                    .child(
                        deferred(
                            div()
                                .absolute()
                                .bottom_full()
                                .mb_2()
                                .left_0()
                                .w_full()
                                .child(self.command_menu.clone()),
                        )
                        .with_priority(10),
                    ),
            )
    }

    /// Session tabs in the top bar, with a plus button for a new session. The
    /// bar is also the window's drag area, so tabs stop mouse-down from
    /// starting a drag.
    fn render_topbar(
        &self,
        window: &Window,
        nav_width: Pixels,
        right_extent: f32,
        right_progress: f32,
        right_visible: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = appearance::secondary_text();
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
        let now = std::time::Instant::now();
        let reduce_motion = cx.reduce_motion();
        let tab_moving = std::cell::Cell::new(false);
        let hint_epoch = self.hint_hold.epoch();
        if dragging_tab.is_none() {
            self.tab_shifts.borrow_mut().clear();
        }
        {
            let live: HashSet<u64> = tabs.iter().map(|(_, uid, _, _)| *uid).collect();
            self.close_slots.borrow_mut().retain(|uid, _| live.contains(uid));
            self.tab_shifts.borrow_mut().retain(|uid, _| live.contains(uid));
        }
        let drag_source = dragging_tab.and_then(|uid| {
            tabs.iter()
                .position(|(tab, tab_uid, _, _)| tab.is_some() && *tab_uid == uid)
        });
        let tab = |id: SharedString, active: bool| {
            h_flex()
                .id(id.clone())
                .group(id)
                // The close target must fit inside the tab's animated clip.
                .h(zpx(TAB_HEIGHT))
                .pl_2()
                .pr_1()
                .gap_2()
                .items_center()
                .rounded(zpx(8.0))
                .cursor_pointer()
                .text_size(zpx(TEXT_MD))
                .text_color(if active { rgb(0xf5f5f5) } else { muted })
                .when(active, |t| t.bg(rgba(0xffffff14)))
                .when(!active, |t| t.hover(|t| t.bg(rgba(0xffffff0a))))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };

        let bar = TitleBar::new()
            .h(zpx(self.metrics.topbar_height))
            .min_h(zpx(self.metrics.topbar_height))
            .flex_none()
            .border_b(px(0.))
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
                            // Linear layout: navigation above the sidebar, tabs from
                            // the chat page's left edge.
                            .child(self.render_nav(nav_width, cx))
                            .child(
                                tab_zone::track(tab_motion::viewport(), self.tab_zone.clone(), tab_zone::Zone::Main)
                                    .h_full()
                                    .pl(zpx(self.metrics.page_inset))
                                    .overflow_hidden()
                                    .child(h_flex()
                                    .id("tab-scroll")
                                    .flex_shrink_1()
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
                                            let terminal = matches!(&tab_key, Some(Tab::Terminal(_)));
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
                                                Some(Tab::Terminal(id) | Tab::Session(id)) => self.state.read(cx).title_loading(id),
                                                _ => false,
                                            };

                                            let group = SharedString::from(format!("tab-{name}"));
                                            let mut el = tab(group.clone(), active);
                                            if let Some(tab_key) = tab_key.clone() {
                                                let (select, close) =
                                                    (self.state.clone(), self.state.clone());
                                                let (select_tab, close_tab) =
                                                    (tab_key.clone(), tab_key);
                                                let working = self.state.read(cx).tab_working(&select_tab);
                                                let indicator_id = Some(AppState::tab_unread_id(&select_tab));
                                                let drag_shell = cx.entity();
                                                let drop_shell = drag_shell.clone();
                                                let tab_focus = self.state.read(cx).control_focus.control(&format!("tab-{name}"), cx);
                                                // Keyboard focus shows the close glyph as hover does.
                                                let tab_focused = keyboard_control::keyboard_within(&tab_focus, window, cx);
                                                // The close slot eases between closed, hint/spinner and close widths,
                                                // so the title's fade edge glides instead of jumping.
                                                let slot_target = if tab_focused || self.hovered_tab == Some(uid) {
                                                    crate::icons::TAB_CLOSE_SIZE
                                                } else if shortcut_hint || working {
                                                    crate::icons::BUTTON_SIZE
                                                } else {
                                                    0.
                                                };
                                                let slot_width = {
                                                    let mut slots = self.close_slots.borrow_mut();
                                                    let tween = slots.entry(uid).or_insert_with(|| Tween::settled(slot_target, CLOSE_SLOT_MOTION));
                                                    tween.retarget(slot_target, now, reduce_motion);
                                                    let (width, moving) = tween.sample(now);
                                                    if moving { tab_moving.set(true); }
                                                    width
                                                };
                                                el = el.keyboard_control_with(&tab_focus).when(tab_focused, |tab| {
                                                    tab.child(keyboard_control::reveal_when_focused(self.tab_scroll.clone(), false))
                                                });
                                                let drag = DraggedTab {
                                                    uid,
                                                    title: title.clone(),
                                                    provider,
                                                    terminal,
                                                    start_pointer_y: Pixels::ZERO,
                                                    cursor_offset_x: Pixels::ZERO,
                                                    strip_bounds: self.tab_strip_bounds.clone(),
                                                    start_pointer_x: self.tab_drag_origin.clone(),
                                                };
                                                let start_x = drag.start_pointer_x.clone();
                                                el = el
                                                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                                        if *hovered {
                                                            this.hovered_tab = Some(uid);
                                                        } else if this.hovered_tab == Some(uid) {
                                                            this.hovered_tab = None;
                                                        }
                                                        cx.notify();
                                                    }))
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
                                                    .child(if terminal { mode_ui::terminal_mark(provider) } else { mode_ui::tab_mark(provider) })
                                                    .child(if title_loading {
                                                        div().flex_1().min_w_0().truncate().child(
                                                            gpui_kit::component::shimmer::ShimmerText::new(title)
                                                                .id(SharedString::from(format!("title-shimmer-{name}")))
                                                                .duration(ui::SHIMMER_PERIOD))
                                                    } else { tab_title(title).when(preview, |title| title.italic().opacity(0.65)) })
                                                    .when_some(indicator_id.as_ref().filter(|id| { let (from, target, start) = self.state.read(cx).unread_motion(id); !working && (target > 0. || (from > 0. && start.is_some_and(|start| start.elapsed().as_millis() < 180))) }), |tab, id| tab.child(sidebar::unread_dot(id, self.state.read(cx), cx)))
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "close-{name}"
                                                            )))
                                                            .keyboard_control()
                                                            .rounded_sm()
                                                            .h(zpx(crate::icons::TAB_CLOSE_SIZE))
                                                            .w(zpx(slot_width))
                                                            .overflow_hidden()
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .flex_none()
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
                                                                tab_action_content(
                                                                    group.clone(),
                                                                    shortcut_number.filter(|_| shortcut_hint),
                                                                    muted.into(),
                                                                    working,
                                                                    tab_focused,
                                                                    (!reduce_motion).then_some(hint_epoch),
                                                                ),
                                                            ),
                                                    );
                                            } else {
                                                // Closing: same look, no handlers.
                                                el = el
                                                    .child(mode_ui::tab_mark(provider))
                                                    .child(tab_title(title))
                                                    .child(div().size(zpx(crate::icons::TAB_CLOSE_SIZE)).flex_none());
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
                                            // Interruptible: a new target eases on from where the
                                            // tab is now, never from the previous slot.
                                            let shift = if dragging_tab.is_some() {
                                                let target = offset_for(drag_target);
                                                let mut shifts = self.tab_shifts.borrow_mut();
                                                let tween = shifts.entry(uid).or_insert_with(|| Tween::settled(target, TAB_SHIFT_MOTION));
                                                tween.retarget(target, now, reduce_motion);
                                                let (value, moving) = tween.sample(now);
                                                if moving { tab_moving.set(true); }
                                                value
                                            } else {
                                                0.
                                            };
                                            let shifted = div()
                                                .flex_none()
                                                .when(dragging_tab == Some(uid), |tab| {
                                                    tab.opacity(0.0)
                                                })
                                                .child(clip)
                                                .relative()
                                                .left(zpx(shift));
                                            let ghost = (!live).then(|| {
                                                self.state.read(cx).ghosts.iter().find(|g| g.uid == uid).cloned()
                                            }).flatten();
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
                                                    Animation::new(if live { tab_motion::ENTER } else { tab_motion::EXIT })
                                                        .with_easing(|t| 1. - (1. - t).powi(3)),
                                                    move |slot, t| {
                                                        // A closing tab shrinks from how open it was, so one
                                                        // closed mid-opening never jumps to full width first.
                                                        let t = match &ghost {
                                                            Some(ghost) => ghost.fraction(std::time::Instant::now()),
                                                            None if !live => 1.0 - t,
                                                            None => if animate_open { t } else { 1.0 },
                                                        };
                                                        slot.w(zpx((TAB_WIDTH + TAB_GAP) * t)).opacity(t)
                                                    },
                                                )
                                        },
                                    )))
                                    .children(crate::tab_transfer::edge_fades(&self.tab_scroll, shell_surface())),
                            )
                            .child(
                                div()
                                    .id("new-tab")
                                    .keyboard_control()
                                    .debug_selector(|| "new-main-tab".into())
                                    .ml(zpx(8.5))
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
                                    .motion_child(dragging_tab.is_none(), move |feedback| feedback.glyph(Glyph::Add).with_size(zpx(13.0)).text_color(muted)),
                            ),
                    )
                    .child(
                        h_flex()
                            .id("right-titlebar")
                            .relative()
                            .debug_selector(|| "right-titlebar".into())
                            .flex_none()
                            .h_full()
                            .min_w_0()
                            .w({
                                let collapsed = crate::icons::button_size() + window.rem_size() * 0.5;
                                collapsed + (zpx(right_extent) - collapsed) * right_progress
                            })
                            .when(right_visible, |header| {
                                header.child(
                                    tab_zone::track(div(), self.tab_zone.clone(), tab_zone::Zone::Right)
                                        .id("right-titlebar-tabs")
                                        .debug_selector(|| "right-titlebar-tabs".into())
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .opacity(right_progress)
                                        .child(self.right.update(cx, |panel, cx| panel.render_tab_strip(cx))),
                                ).child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .w(px(0.5))
                                        .flex()
                                        .items_center()
                                        .opacity(right_progress)
                                        .child(
                                            div()
                                                .w_full()
                                                .h(zpx(20.0))
                                                .bg(appearance::separator()),
                                        ),
                                )
                            })
                            .child(
                        div()
                            .id("toggle-right")
                            .keyboard_control()
                            .role(gpui::Role::Button)
                            .aria_label("Toggle right sidebar")
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Toggle right sidebar (⌘\\)").build(window, cx)
                            })
                            .flex_none()
                            .size(crate::icons::button_size())
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
                            .motion_child(true, move |feedback| feedback.glyph(Glyph::PanelRight)
                                .text_color(muted)),
                        ),
                    ),
            );
        if tab_moving.get() {
            window.request_animation_frame();
        }
        bar
    }

    fn render_page(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        tab_zone::track(div(), self.tab_zone.clone(), tab_zone::Zone::Main)
            .size_full()
            .min_w_0()
            .child(self.render_session_content(window, cx))
    }

    fn render_session_content(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let radius = zpx(self.metrics.page_radius);
        let (panel, selection) = {
            let st = self.state.read(cx);
            (Panel::Chat, st.selection.clone())
        };
        if let Selection::Session(id) = &selection
            && (self.mode_terminals.contains_key(id) || self.mode_switching.contains(id))
        {
            self.ambient.update(cx, |ambient, cx| {
                ambient.set_surface(ambient::AmbientSurface::Terminal, window, cx)
            });
            return self.render_mode_terminal(id, cx);
        }
        let chat = self.open_chat(cx);
        let landing = panel == Panel::Chat
            && !matches!(selection, Selection::Terminal(_))
            && chat.is_none()
            && self.state.read(cx).pending_prompt().is_none();
        let surface = if panel != Panel::Chat || self.search_open {
            ambient::AmbientSurface::Hidden
        } else if matches!(selection, Selection::Terminal(_)) {
            ambient::AmbientSurface::Terminal
        } else if landing {
            ambient::AmbientSurface::Landing
        } else {
            // A message has been sent: the background stays, softly blurred.
            ambient::AmbientSurface::Chat
        };
        self.ambient
            .update(cx, |ambient, cx| ambient.set_surface(surface, window, cx));
        if panel == Panel::Chat && matches!(selection, Selection::Terminal(_)) {
            return div()
                .relative()
                .size_full()
                .rounded(radius)
                .overflow_hidden()
                .bg(page_surface())
                .child(self.ambient.clone())
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
                Some(chat) => page.child(self.ambient.clone()).child(
                    v_flex()
                        .relative()
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
                                        Animation::new(Duration::from_millis(
                                            if cx.reduce_motion() { 0 } else { 120 },
                                        ))
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
                None => page
                    .child(self.ambient.clone())
                    .child(
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
                                        div().relative().w_full().child(self.render_composer(cx)),
                                    )
                                    // Project, branch and New worktree on the left and
                                    // Chat/Terminal on the right, each in its own container
                                    // flush with the composer's edges; the middle stays empty.
                                    .child(
                                        h_flex()
                                            .relative()
                                            .w_full()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                new_tab_group()
                                                    .relative()
                                                    .flex_shrink_1()
                                                    .min_w_0()
                                                    .child(self.render_new_session_options(cx))
                                                    .child(
                                                        self.ambient
                                                            .read(cx)
                                                            .region_probe(1, zpx(8.0)),
                                                    ),
                                            )
                                            .child(
                                                new_tab_group()
                                                    .relative()
                                                    .child(self.render_mode_picker(cx))
                                                    .child(
                                                        self.ambient
                                                            .read(cx)
                                                            .region_probe(2, zpx(8.0)),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(zpx(16.0))
                            .right(zpx(20.0))
                            .text_size(zpx(12.0))
                            .text_color(rgb(0x737373))
                            .child(concat!("v", env!("CARGO_PKG_VERSION"))),
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
        self.ensure_goal_ticker(cx);
        let zoom = del_core::zoom::percent();
        if self.zoom_percent != zoom {
            self.zoom_percent = zoom;
            for chat in self.chats.values() {
                chat.update(cx, |chat, cx| chat.remeasure_for_zoom(cx));
            }
        }
        if self.state.read(cx).sidebar_drag.active.is_some() && !cx.has_active_drag() {
            self.state
                .update(cx, |state, _| state.sidebar_drag = Default::default());
        }
        if self.dragging_tab.is_some() && !cx.has_active_drag() {
            self.dragging_tab = None;
            self.drag_target = None;
            self.tab_shifts.borrow_mut().clear();
        }
        if !window.is_window_active() && self.state.read(cx).shortcut_hints {
            self.hint_hold.cancel();
            self.state
                .update(cx, |state, _| state.shortcut_hints = false);
        }
        let mut notices = self.state.update(cx, |state, _| state.take_notices());
        for chat in self.chats.values() {
            let errors = chat.update(cx, |chat, _| std::mem::take(&mut chat.pending_errors));
            notices.extend(errors.iter().map(|error| {
                state::notices::Notice::new(state::notices::Severity::Error, error)
            }));
        }
        if !notices.is_empty() {
            let state = self.state.clone();
            window.defer(cx, move |window, cx| {
                for notice in notices {
                    push_notice(notice, &state, window, cx);
                }
            });
        }
        if self.dictation.as_ref().is_some_and(|voice| {
            voice.owner != self.composer_owner(cx) || self.state.read(cx).panel != Panel::Chat
        }) {
            self.cancel_dictation(cx);
        }
        let settings_open = self.state.read(cx).panel == Panel::Providers;
        // Reopened during its fade: keep the same dialog and cancel the exit.
        let reopened =
            settings_open && !self.settings_focused && self.settings_closing.take().is_some();
        if settings_open && !self.settings_focused && !reopened {
            let state = self.state.clone();
            let shell = cx.weak_entity();
            window.open_dialog(cx, move |dialog, window, cx| {
                let closing = shell
                    .upgrade()
                    .and_then(|shell| shell.read(cx).settings_closing);
                if closing.is_some() {
                    window.request_animation_frame();
                }
                let close_shell = shell.clone();
                let close_state = state.clone();
                let height = zpx(420.).min(window.viewport_size().height - zpx(48.));
                dialog
                    .width(zpx(720.).min(window.viewport_size().width - zpx(48.)))
                    .margin_top(((window.viewport_size().height - height) / 2.).max(zpx(24.)))
                    .p_0()
                    .close_button(false)
                    .entrance(if closing.is_some() {
                        gpui_kit::component::dialog::DialogEntrance::None
                    } else {
                        gpui_kit::component::dialog::DialogEntrance::FadeSlide
                    })
                    .when_some(closing, |dialog, started| {
                        dialog.opacity(search_ui::exit_opacity(started))
                    })
                    .bg(appearance::popup_surface())
                    .rounded(zpx(14.))
                    .overflow_hidden()
                    // Escape and outside clicks ask here first, so they leave through the
                    // fade with Done instead of the dialog's own instant close.
                    .on_cancel({
                        let state = state.clone();
                        move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                if state.panel == Panel::Providers {
                                    state.toggle_providers(cx);
                                }
                            });
                            false
                        }
                    })
                    .on_close(move |_, _, cx| {
                        // Closed some other way during the fade: nothing left to close later.
                        let _ = close_shell.update(cx, |this, _| this.settings_closing = None);
                        close_state.update(cx, |state, cx| {
                            if state.panel == Panel::Providers {
                                state.toggle_providers(cx);
                            }
                        });
                    })
                    .child(
                        h_flex()
                            .h(height)
                            .min_h_0()
                            .items_start()
                            .child(
                                div()
                                    .w(zpx(150.))
                                    .h_full()
                                    .flex_none()
                                    .p_3()
                                    .border_r_1()
                                    .border_color(appearance::separator())
                                    .child(signin::navigation(&state, cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .child(signin::render(&state, cx)),
                            ),
                    )
            });
        }
        if !settings_open && self.settings_focused {
            if cx.reduce_motion() {
                window.close_dialog(cx);
                self.focus_composer(window, cx);
            } else {
                let started = std::time::Instant::now();
                self.settings_closing = Some(started);
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor()
                        .timer(search_ui::EXIT_DURATION)
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.settings_closing == Some(started) {
                            this.settings_closing = None;
                            window.close_dialog(cx);
                            this.focus_composer(window, cx);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        self.settings_focused = settings_open;
        if self.settings_closing.is_some() {
            window.request_animation_frame();
        }
        self.sync_caffeine(cx);
        let terminal_processes = self
            .terminals
            .iter()
            .map(|(id, host)| (Tab::Terminal(id.clone()), host.read(cx).snapshot().pid))
            .chain(self.mode_terminals.iter().map(|(id, terminal)| {
                (
                    Tab::Session(id.clone()),
                    terminal.host.read(cx).snapshot().pid,
                )
            }))
            .collect();
        self.usage
            .update(cx, |usage, _| usage.terminal_processes = terminal_processes);
        self.sync_terminal_sidebar_actions(cx);
        // Settings navigation stays available without changing chat's collapse preference.
        let sidebar_visible = self.sidebar_visible || settings_open;
        if !cx.reduce_motion()
            && self
                .mode_slide_started
                .is_some_and(|started| started.elapsed() < Duration::from_millis(180))
        {
            window.request_animation_frame();
        }
        self.ensure_modes(window, cx);
        self.sync_workspaces(window, cx);
        let mode_key = (
            self.state.read(cx).selection.clone(),
            self.state.read(cx).mode(),
        );
        if self.state.read(cx).panel == Panel::Chat && self.mode_focus.as_ref() != Some(&mode_key) {
            self.mode_focus = Some(mode_key.clone());
            self.tab_zone.set(tab_zone::Zone::Main);
            // Session restoration can mount native tool views and move the
            // composer between layouts. Focus after the new subtree is mounted.
            let shell = cx.weak_entity();
            window.on_next_frame(move |window, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    if shell.mode_focus.as_ref() == Some(&mode_key) {
                        shell.focus_composer(window, cx);
                    }
                });
            });
        }
        self.ensure_chat(cx);
        self.sync_composer_draft(window, cx);
        let selected_uid = self.composer_owner(cx);
        if self.tab_reveal.is_none_or(|(uid, _)| uid != selected_uid) {
            self.tab_reveal = Some((selected_uid, std::time::Instant::now()));
        }
        self.sync_right(window, cx);
        self.track_navigation(cx);
        if self.bottom.open {
            self.ensure_panel_terminal(window, cx);
        }
        if let Some(requested) = self.pending_peer.clone() {
            let id = self
                .state
                .read(cx)
                .backend()
                .manager
                .resolve_peer(&requested);
            let is_primary = matches!(&self.state.read(cx).selection,
                Selection::Session(current) | Selection::Terminal(current) if current == &id);
            let in_main = self.workspace(cx).is_some_and(|workspace| {
                workspace.update(cx, |w, cx| {
                    if is_primary {
                        w.select_index(0, window, cx);
                        true
                    } else {
                        w.show_peer(&id, window, cx)
                    }
                })
            });
            let in_right = !in_main
                && self
                    .right
                    .update(cx, |panel, cx| panel.show_peer(&id, window, cx));
            if in_main || in_right {
                if in_right {
                    self.set_right(true, cx);
                }
                self.pending_peer = None;
            }
        }
        if let Some(workspace) = self.workspace(cx) {
            let migrated = workspace.update(cx, |workspace, cx| {
                workspace.migrate_to_sidebar(&self.right, window, cx)
            });
            if migrated {
                self.remember_right(cx);
                self.persist_right(cx);
                workspace.read(cx).persist(cx);
            }
        }
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
            if sidebar_visible { left_width } else { 0.0 },
        );
        if cx.reduce_motion() {
            self.sidebar_motion.settle(self.sidebar_open);
            self.right_motion.settle(self.right_open);
        }
        let (left_progress, left_animating) = self.sidebar_motion.sample(std::time::Instant::now());
        let left_progress = if settings_open { 1. } else { left_progress };
        // The title bar's navigation spans the sidebar, so the first tab starts
        // at the chat page's left edge; macOS reserves the traffic lights.
        let left_extent = if SIDEBAR_ENABLED && sidebar_visible {
            left_width * left_progress
        } else {
            0.0
        };
        let nav_width = gpui::px(workbench::navigation_width(
            left_extent,
            del_core::zoom::scaled(1.0),
            if cfg!(target_os = "macos") {
                80.0
            } else {
                12.0
            },
        ));
        let (progress, animating) = self.right_motion.sample(std::time::Instant::now());
        let right_progress = if right_visible { progress } else { 0. };
        if animating || left_animating {
            window.request_animation_frame();
        }
        self.right.update(cx, |panel, cx| {
            panel.set_tabs_in_titlebar(true);
            // Hide native content before the close animation squeezes its
            // viewport; a floating browser preview must retain the last size.
            panel.set_preview_visible(right_visible && self.right_open && !self.search_open, cx)
        });
        // Only the panel that just stopped being shown needs hiding; every other
        // cached panel was hidden when it was replaced.
        if self.right_previous.as_ref() != Some(&self.right)
            && let Some(previous) = self.right_previous.replace(self.right.clone())
        {
            previous.update(cx, |panel, cx| panel.set_preview_visible(false, cx));
        }

        // Load the title into the rename field when a rename starts.
        let renaming = self.state.read(cx).renaming.clone();
        let project_renaming = self.state.read(cx).project_renaming.clone();
        if renaming != self.renaming_loaded || project_renaming != self.project_renaming_loaded {
            let title = project_renaming
                .as_ref()
                .map(|path| self.state.read(cx).project_name(path))
                .or_else(|| {
                    renaming
                        .as_ref()
                        .map(|id| self.state.read(cx).current_title(id))
                });
            if let Some(title) = title {
                self.rename.update(cx, |input, cx| {
                    input.set_value(title, window, cx);
                    input.focus(window, cx);
                });
                // As in Finder: start with the whole name selected, ready to replace.
                cx.defer_in(window, |_, window, cx| {
                    window.dispatch_action(Box::new(gpui_kit::component::input::SelectAll), cx);
                });
            }
            self.renaming_loaded = renaming;
            self.project_renaming_loaded = project_renaming;
        }

        let preview_sessions = self.state.read(cx).tabs.clone();
        let preview_session = preview_sessions
            .iter()
            .find(|tab| tab.selection() == self.state.read(cx).selection)
            .cloned();
        self.browser_preview.update(cx, |preview, cx| {
            preview.set_session(preview_session, &preview_sessions, cx);
        });
        // Native WebKit surfaces paint above GPUI overlays. Do not float a
        // thumbnail over an open browser, even when it displays another tab.
        let browser_visible = self.right.read(cx).browser_surface_visible(cx)
            || self
                .workspace(cx)
                .is_some_and(|workspace| workspace.read(cx).browser_surface_visible(cx));
        let preview_target = self.browser_preview.read(cx).target();
        let preview_panel = preview_target
            .as_ref()
            .and_then(|target| self.find_agent_browser(&target.session, &target.tab_id, cx));
        self.browser_preview.update(cx, |preview, cx| {
            if let (Some(target), Some(panel)) = (preview_target, preview_panel) {
                preview.rebind(target.session, panel, target.tab_id, cx);
            }
            preview.set_suppressed(
                self.search_open || settings_open || browser_visible,
                self.metrics.topbar_height,
                cx,
            );
        });
        keyboard_control::restore_lost_focus(&self.shell_focus, &self.focus_watch, window, cx);
        v_flex()
            .relative()
            .size_full()
            .bg(shell_surface())
            .track_focus(&self.shell_focus)
            .key_context(
                if !self.search_open && self.state.read(cx).panel == Panel::Chat {
                    "Shell SessionNavigation"
                } else {
                    "Shell"
                },
            )
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::MoveDown, window, cx| {
                    if this
                        .command_menu
                        .update(cx, |menu, cx| menu.navigate(1, window, cx))
                    {
                        cx.stop_propagation();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::MoveUp, window, cx| {
                    if this
                        .command_menu
                        .update(cx, |menu, cx| menu.navigate(-1, window, cx))
                    {
                        cx.stop_propagation();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, action: &gpui_kit::component::input::Enter, window, cx| {
                    if !action.shift
                        && !action.secondary
                        && ((this.composer.read(cx).focus_handle(cx).is_focused(window)
                            && this.submit_mode_command(window, cx))
                            || this
                                .command_menu
                                .update(cx, |menu, cx| menu.accept(window, cx)))
                    {
                        cx.stop_propagation();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::IndentInline, window, cx| {
                    if this
                        .command_menu
                        .update(cx, |menu, cx| menu.accept(window, cx))
                    {
                        cx.stop_propagation();
                    }
                },
            ))
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::Escape, window, cx| {
                    if this.state.read(cx).project_renaming.is_some()
                        || this.state.read(cx).renaming.is_some()
                    {
                        this.state.update(cx, |s, cx| s.finish_rename(None, cx));
                        cx.stop_propagation();
                    } else if this.composer.read(cx).focus_handle(cx).is_focused(window)
                        && this.command_menu.update(cx, |menu, cx| menu.dismiss(cx))
                    {
                        cx.stop_propagation();
                    }
                },
            ))
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
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                let modifiers = event.modifiers;
                let command_only = modifiers.platform
                    && !modifiers.shift
                    && !modifiers.alt
                    && !modifiers.control
                    && !modifiers.function
                    && !this.search_open;
                match this.hint_hold.modifiers_changed(command_only) {
                    Some(generation) => {
                        cx.spawn_in(window, async move |this, cx| {
                            cx.background_executor()
                                .timer(ui::SHORTCUT_HINT_DELAY)
                                .await;
                            let _ = this.update(cx, |this, cx| {
                                if this.hint_hold.delay_elapsed(generation) && !this.search_open {
                                    this.state.update(cx, |state, cx| {
                                        state.shortcut_hints = true;
                                        cx.notify();
                                    });
                                }
                            });
                        })
                        .detach();
                    }
                    None if !command_only => this.hide_shortcut_hints(cx),
                    None => {}
                }
            }))
            // Any key, including the second half of ⌘N or ⌘K, ends the hold.
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, cx| {
                this.hide_shortcut_hints(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectProvider, window, cx| {
                this.open_composer_picker(keybinds::Picker::Provider, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectModel, window, cx| {
                this.open_composer_picker(keybinds::Picker::Model, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectEffort, window, cx| {
                this.open_composer_picker(keybinds::Picker::Effort, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectBranch, window, cx| {
                this.open_composer_picker(keybinds::Picker::Branch, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SearchConversations, window, cx| {
                this.open_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if !SIDEBAR_ENABLED || this.state.read(cx).panel == Panel::Providers {
                    return;
                }
                this.set_sidebar(!this.sidebar_open, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| {
                if this.state.read(cx).panel == Panel::Chat {
                    this.set_right(!this.right_open, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, window, cx| {
                this.navigate(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateForward, window, cx| {
                this.navigate(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminalPanel, window, cx| {
                this.toggle_bottom_panel(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.open_inner_tool(right_panel::Pane::Terminal, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenChanges, window, cx| {
                this.open_inner_tool(right_panel::Pane::Changes, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFiles, window, cx| {
                this.open_inner_tool(right_panel::Pane::Files, window, cx);
            }))
            .on_action(cx.listener(|this, action: &ReopenSideChat, window, cx| {
                if this.right.read(cx).side_history_parent(cx).as_ref() != Some(&action.parent) {
                    return;
                }
                let found = this.workspace(cx).is_some_and(|workspace| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.show_saved_side_chat(&action.id, window, cx)
                    })
                });
                if !found {
                    this.set_right(true, cx);
                    this.right.update(cx, |right, cx| {
                        right.reopen_side_chat(action.id.clone(), window, cx)
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSideHistory, window, cx| {
                if this.state.read(cx).panel == Panel::Chat {
                    this.set_right(true, cx);
                    this.right
                        .update(cx, |right, cx| right.show_side_history(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSideChat, window, cx| {
                this.open_inner_tool(right_panel::Pane::SideChat, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseWorkspaceTab, window, cx| {
                if this.state.read(cx).panel == Panel::Chat
                    && let Some(workspace) = this.workspace(cx)
                {
                    tab_zone::Zone::Right.close_tab(&workspace, &this.right, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.state.read(cx).panel == Panel::Chat
                    && this.active_tab_zone() == tab_zone::Zone::Right
                {
                    this.right
                        .update(cx, |panel, cx| panel.close_active_tab(window, cx));
                    return;
                }
                this.tab_zone.set(tab_zone::Zone::Main);
                this.state.update(cx, |state, cx| {
                    if state.panel == Panel::Providers {
                        state.toggle_providers(cx);
                    } else {
                        state.close_active_tab(cx);
                    }
                });
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
            .on_action(cx.listener(|this, _: &NewWorkspaceTab, window, cx| {
                if this.state.read(cx).panel == Panel::Chat
                    && this.active_tab_zone() == tab_zone::Zone::Right
                {
                    this.new_inner_tab(window, cx);
                } else {
                    this.tab_zone.set(tab_zone::Zone::Main);
                    tab_zone::Zone::Main.new_tab(&this.state, &this.right, window, cx);
                    this.focus_composer(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.tab_zone.set(tab_zone::Zone::Main);
                this.state.update(cx, |s, cx| s.new_session(cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextWorkspaceTab, window, cx| {
                this.set_right(true, cx);
                this.tab_zone.set(tab_zone::Zone::Right);
                this.right
                    .update(cx, |panel, cx| panel.select_relative(1, window, cx));
            }))
            .on_action(cx.listener(|this, _: &PrevWorkspaceTab, window, cx| {
                this.set_right(true, cx);
                this.tab_zone.set(tab_zone::Zone::Right);
                this.right
                    .update(cx, |panel, cx| panel.select_relative(-1, window, cx));
            }))
            .on_action(cx.listener(|this, _: &NextSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevSession, window, cx| {
                this.state.update(cx, |s, cx| s.select_relative(-1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextSidebarSession, window, cx| {
                this.state
                    .update(cx, |state, cx| state.select_sidebar_relative(1, cx));
                this.focus_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevSidebarSession, window, cx| {
                this.state
                    .update(cx, |state, cx| state.select_sidebar_relative(-1, cx));
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
            .child(self.render_topbar(
                window,
                nav_width,
                right_width + self.metrics.page_inset,
                right_progress,
                right_visible,
                cx,
            ))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .bg(shell_surface())
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<SidebarResize>, window, cx| {
                            let drag = event.drag(cx).clone();
                            this.resize_sidebar(&drag, window, cx);
                        },
                    ))
                    .when(SIDEBAR_ENABLED && sidebar_visible, |row| {
                        let sidebar = div()
                            .rounded_tr(zpx(self.metrics.page_radius))
                            .bg(appearance::sidebar_surface())
                            .relative()
                            .h_full()
                            .flex_none()
                            .overflow_hidden()
                            // Contents keep their full width, pinned to the clipped edge, so
                            // animating the width slides the sidebar instead of reflowing it.
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right_0()
                                    .w(zpx(left_width))
                                    .child(sidebar::render(
                                        &self.state,
                                        &self.rename,
                                        &self.usage,
                                        &self.updater,
                                        window,
                                        cx,
                                    )),
                            );
                        row.child(sidebar.w(zpx(left_width * left_progress)))
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(inset)
                            .pb(inset)
                            .child(
                                v_flex()
                                    .size_full()
                                    .min_h_0()
                                    .on_children_prepainted({
                                        let page_bounds = self.page_bounds.clone();
                                        move |bounds, window, _| {
                                            if let Some(bounds) = bounds.first()
                                                && page_bounds.replace(*bounds) != *bounds
                                            {
                                                window.request_animation_frame();
                                            }
                                        }
                                    })
                                    .on_drag_move(cx.listener(
                                        |this,
                                         event: &DragMoveEvent<workbench::PanelResize>,
                                         window,
                                         cx| {
                                            let drag = event.drag(cx).clone();
                                            this.resize_bottom_panel(&drag, window, cx);
                                        },
                                    ))
                                    .child(
                                        div()
                                            .w_full()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .when(
                                                self.bottom.open && self.bottom.maximized,
                                                |page| page.h(zpx(0.0)).flex_none(),
                                            )
                                            .when(
                                                !(self.bottom.open && self.bottom.maximized),
                                                |page| page.flex_1(),
                                            )
                                            .child(self.render_page(window, cx)),
                                    )
                                    .children(self.render_bottom_panel(cx)),
                            )
                            .when(SIDEBAR_ENABLED && sidebar_visible, |page| {
                                // Paint the target above both neighboring surfaces
                                // so the entire invisible strip gets the resize cursor.
                                page.child(
                                    self.resize_handle(
                                        false,
                                        left_width,
                                        if right_visible { right_width } else { 0.0 },
                                        cx,
                                    )
                                    .left(zpx((self.metrics.page_inset
                                        - LEFT_SIDEBAR_RESIZE_TARGET)
                                        / 2.))
                                    .w(zpx(LEFT_SIDEBAR_RESIZE_TARGET)),
                                )
                            }),
                    )
                    .when(right_visible, |row| {
                        let full = right_width + self.metrics.page_inset;
                        row.child(
                            tab_zone::track(div(), self.tab_zone.clone(), tab_zone::Zone::Right)
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
                                            .overflow_hidden()
                                            .bg(appearance::shell_surface())
                                            .child(self.right.clone()),
                                    ),
                                )
                                .child(
                                    self.resize_handle(
                                        true,
                                        right_width,
                                        if sidebar_visible { left_width } else { 0.0 },
                                        cx,
                                    )
                                    .left_0()
                                    .bottom(inset),
                                )
                                .w(zpx(full * right_progress)),
                        )
                    }),
            )
            .child(self.render_status_bar(cx))
            .child(self.browser_preview.clone())
            .when(
                self.search_open || settings_open || self.settings_closing.is_some(),
                |shell| {
                    // Black at this strength turns a light workspace muddy grey.
                    let shade =
                        div()
                            .absolute()
                            .inset_0()
                            .bg(gpui_kit::rgba(if appearance::light() {
                                0x1c18102e
                            } else {
                                0x00000066
                            }));
                    if let Some(started) = self.search_closing.or(self.settings_closing) {
                        let strength = search_ui::exit_opacity(started);
                        shell.child(
                            shade
                                .opacity(strength)
                                .child(backdrop::blur_layer(strength)),
                        )
                    } else if cx.reduce_motion() {
                        shell.child(shade.child(backdrop::blur_layer(1.)))
                    } else {
                        shell.child(
                            shade.with_animation(
                                "search-background-dim",
                                Animation::new(Duration::from_millis(180))
                                    .with_easing(ease_out_quint()),
                                |shade, progress| {
                                    shade
                                        .opacity(progress)
                                        .child(backdrop::blur_layer(progress))
                                },
                            ),
                        )
                    }
                },
            )
    }
}

/// The menu bar: the app menu with About, Settings, Hide and Quit; Edit for
/// text fields; Window with Minimize and Zoom.
fn app_menus() -> Vec<Menu> {
    use gpui_kit::component::input;
    vec![
        Menu {
            name: "Del".into(),
            disabled: false,
            items: vec![
                MenuItem::action("About Del", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", ToggleProviders),
                MenuItem::separator(),
                MenuItem::action("Hide Del", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Del", Quit),
                MenuItem::action("Quit Del Completely", QuitCompletely),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "Window".into(),
            disabled: false,
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
    ]
}

/// The saved window frame clamped onto a connected display, or the centred
/// default (70% × 76% of the primary display) when there is none or it no
/// longer fits anywhere.
fn initial_window_bounds(cx: &App) -> (Option<DisplayId>, WindowBounds) {
    let saved = state::window_bounds::path().and_then(|path| state::window_bounds::read(&path));
    if let Some(saved) = saved {
        let displays = cx.displays();
        let by_uuid = saved.display.as_ref().and_then(|uuid| {
            displays
                .iter()
                .find(|display| display.uuid().is_ok_and(|id| &id.to_string() == uuid))
                .cloned()
        });
        if let Some(display) = by_uuid.or_else(|| cx.primary_display())
            && let Some(bounds) = saved.restore_on(display.visible_bounds())
        {
            return (Some(display.id()), bounds);
        }
    }
    let window_size = cx.primary_display().map_or(WINDOW_SIZE, |display| {
        let available = display.visible_bounds().size;
        size(
            (available.width * 0.70).max(px(state::window_bounds::MIN_SIZE.0)),
            (available.height * 0.76).max(px(state::window_bounds::MIN_SIZE.1)),
        )
    });
    (
        None,
        WindowBounds::Windowed(Bounds::centered(None, window_size, cx)),
    )
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--mcp-stdio") {
        let (Ok(url), Ok(token)) = (std::env::var("DEL_MCP_URL"), std::env::var("DEL_MCP_TOKEN"))
        else {
            eprintln!("Del MCP proxy requires DEL_MCP_URL and DEL_MCP_TOKEN");
            std::process::exit(2);
        };
        del_mcp::stdio::run(&url, &token, std::io::stdin().lock(), std::io::stdout());
        return;
    }

    let app = gpui_kit::application().with_assets(assets::AppAssets);
    app.on_reopen(session_lifecycle::reopen);
    app.run(|cx| {
        // Must run before any gpui-component feature is used.
        gpui_kit::init(cx);
        assets::set_app_icon();
        del_terminal::init(cx);
        zoom::load();
        zoom::init(cx);
        appearance::load(cx);
        session_lifecycle::load();

        cx.bind_keys([
            KeyBinding::new("ctrl-m", SelectModel, None),
            KeyBinding::new("ctrl-shift-m", SelectEffort, None),
            KeyBinding::new("ctrl-shift-p", SelectProvider, None),
            KeyBinding::new("ctrl-shift-b", SelectBranch, None),
            KeyBinding::new("cmd-k", SearchConversations, None),
            KeyBinding::new("cmd-\\", ToggleFiles, None),
            KeyBinding::new("cmd-shift-b", ToggleFiles, None),
            KeyBinding::new("alt-cmd-b", ToggleFiles, None),
            KeyBinding::new("ctrl-`", ToggleTerminal, None),
            KeyBinding::new("cmd-j", ToggleTerminalPanel, None),
            KeyBinding::new("cmd-[", NavigateBack, None),
            KeyBinding::new("cmd-]", NavigateForward, None),
            KeyBinding::new("cmd-t", NewWorkspaceTab, None),
            KeyBinding::new("ctrl-t", NewWorkspaceTab, None),
            KeyBinding::new("cmd-n", NewSession, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("cmd-shift-w", CloseWorkspaceTab, None),
            KeyBinding::new("ctrl-shift-g", OpenChanges, None),
            KeyBinding::new("cmd-p", OpenFiles, None),
            KeyBinding::new("alt-cmd-s", OpenSideChat, None),
            KeyBinding::new("alt-shift-cmd-s", OpenSideHistory, None),
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
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("alt-cmd-q", QuitCompletely, None),
            KeyBinding::new("ctrl-tab", NextWorkspaceTab, None),
            KeyBinding::new("cmd-shift-]", NextSession, None),
            KeyBinding::new("ctrl-shift-tab", PrevWorkspaceTab, None),
            KeyBinding::new("cmd-shift-[", PrevSession, None),
            KeyBinding::new("cmd-l", FocusComposer, None),
            KeyBinding::new("cmd-.", StopSession, None),
            KeyBinding::new("cmd-,", ToggleProviders, None),
        ]);
        keybinds::bind_session_navigation(cx);
        // Lets UI work check Reduce Motion without touching the system setting.
        if std::env::var("DEL_BACKEND").as_deref() == Ok("mock")
            && std::env::var_os("DEL_REDUCE_MOTION").is_some()
        {
            cx.set_reduce_motion(true);
        }
        cx.on_action(|_: &Quit, cx| session_lifecycle::request_quit(cx));
        cx.on_action(|_: &QuitCompletely, cx| session_lifecycle::quit_completely(cx));
        cx.on_action(|_: &About, _| session_lifecycle::show_about());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &Minimize, cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, _| window.minimize_window());
            }
        });
        cx.on_action(|_: &Zoom, cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, _| window.zoom_window());
            }
        });
        cx.set_menus(app_menus());
        // Closing the window quits, which also runs the shutdown hook.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                session_lifecycle::quit_completely(cx);
            }
        })
        .detach();

        let metrics = ShellMetrics::default();
        let (light_x, light_y) = metrics.traffic_light_origin();

        let (display_id, window_bounds) = initial_window_bounds(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Del".into()),
                traffic_light_position: Some(point(px(light_x), px(light_y))),
                // Hides the system title bar so the top bar below is drawn
                // entirely by us.
                ..TitleBar::title_bar_options()
            }),
            window_bounds: Some(window_bounds),
            display_id,
            window_min_size: Some(size(
                px(state::window_bounds::MIN_SIZE.0),
                px(state::window_bounds::MIN_SIZE.1),
            )),
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

#[cfg(test)]
mod sidebar_width_tests {
    use super::{SidebarWidths, left_sidebar_open_for_drag, sidebar_width};

    #[test]
    fn held_drag_can_collapse_reopen_and_continue_resizing() {
        let mut open = true;
        for (requested, expected) in [
            (180., true),
            (132., true),
            (101., true),
            (100., false),
            (50., false),
            (101., false),
            (131., false),
            (132., true),
            (200., true),
            (100., false),
            (200., true),
        ] {
            open = left_sidebar_open_for_drag(requested, open);
            assert_eq!(open, expected, "requested width: {requested}");
            if open && requested >= 180. {
                assert_eq!(sidebar_width(requested, false, 1600., 0.), requested);
            }
        }
    }

    #[test]
    fn left_cap_preserves_default_minimum_and_right_widths() {
        assert_eq!(SidebarWidths::default().left, 264.);
        assert_eq!(sidebar_width(600., false, 1600., 0.), 320.);
        assert_eq!(sidebar_width(190., false, 1600., 0.), 190.);
        assert_eq!(sidebar_width(-100., false, 1600., 0.), 180.);
        assert_eq!(sidebar_width(400., false, 450., 100.), 180.);
        assert_eq!(sidebar_width(300., false, 650., 100.), 270.);
        assert_eq!(sidebar_width(f32::INFINITY, false, 1600., 0.), 320.);
        assert_eq!(sidebar_width(900., true, 1600., 180.), 800.);
        assert_eq!(
            sidebar_width(20., true, 1600., 180.),
            super::right_panel::PANEL_MIN_WIDTH
        );
    }

    #[test]
    fn persisted_left_width_is_clamped_and_right_width_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sidebar-widths.json");
        for (saved, expected) in [(600., 320.), (216., 216.), (190., 190.), (20., 180.)] {
            std::fs::write(&path, format!("{{\"left\":{saved},\"right\":725}}")).unwrap();
            let loaded = SidebarWidths::read(&path);
            assert_eq!(loaded.left, expected);
            assert_eq!(loaded.right, 725.);
        }
        std::fs::write(&path, b"invalid").unwrap();
        assert_eq!(SidebarWidths::read(&path).left, 264.);
    }
}

#[cfg(test)]
mod peer_tests;
