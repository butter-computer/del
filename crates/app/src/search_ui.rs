//! Cmd-K actions, recent conversations, and navigation into chat/terminal transcripts.
use crate::{
    AppShell, appearance::{rgb, rgba}, conversation_search as search, state::Tab, zoom::px as zpx,
};
use gpui_kit::{
    component::{
        dialog::DialogEntrance,
        input::{Input, InputEvent, InputState},
        text::{TextView, TextViewState, TextViewStyle},
        *,
    },
    prelude::*,
    *,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct Palette {
    input: Entity<InputState>,
    documents: Vec<Arc<search::Document>>,
    hits: Vec<search::Hit>,
    previews: HashMap<usize, Preview>,
    selected: usize,
    scroll: ListState,
    loading: bool,
    indexing: bool,
    errors: Vec<String>,
    generation: u64,
    runtime: tokio::runtime::Handle,
    chosen_action: Option<PaletteAction>,
}
#[derive(Clone, Copy)]
enum PaletteAction {
    NewConversation,
    Settings,
    Files,
    Terminal,
}
impl PaletteAction {
    const ALL: [Self; 4] = [Self::NewConversation, Self::Settings, Self::Files, Self::Terminal];

    fn label(self) -> &'static str {
        match self {
            Self::NewConversation => "New conversation",
            Self::Settings => "Open Settings",
            Self::Files => "Toggle files",
            Self::Terminal => "Open terminal",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::NewConversation => "icons/pencil.svg",
            Self::Settings => "icons/settings.svg",
            Self::Files => "icons/folder.svg",
            Self::Terminal => "icons/square-terminal.svg",
        }
    }
    fn shortcut(self) -> &'static str {
        match self {
            Self::NewConversation => "⌘ N",
            Self::Settings => "⌘ ,",
            Self::Files => "⇧ ⌘ B",
            Self::Terminal => "⌘ J",
        }
    }
    fn keywords(self) -> &'static str {
        match self {
            Self::NewConversation => "new conversation chat session draft",
            Self::Settings => "open settings preferences appearance opacity accounts providers",
            Self::Files => "toggle files folder sidebar panel",
            Self::Terminal => "open terminal shell console panel",
        }
    }
}

struct Preview {
    view: Entity<TextViewState>,
    highlighted: bool,
}

pub enum PaletteEvent {
    Open(search::Hit),
    Close,
}
impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    fn new(runtime: tokio::runtime::Handle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search commands and conversations…"));
        cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.search(cx);
            }
        })
        .detach();
        Self {
            input,
            documents: vec![],
            hits: vec![],
            previews: HashMap::new(),
            selected: 0,
            scroll: ListState::new(0, ListAlignment::Top, zpx(200.)),
            loading: true,
            indexing: true,
            errors: vec![],
            generation: 0,
            runtime,
            chosen_action: None,
        }
    }
    fn search(&mut self, cx: &mut Context<Self>) {
        self.selected = 0;
        self.chosen_action = None;
        if self.indexing {
            self.reset_scroll(cx);
            cx.notify();
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let query = self.input.read(cx).value().to_string();
        let documents = self.documents.clone();
        self.loading = true;
        self.hits.clear();
        self.previews.clear();
        self.reset_scroll(cx);
        let task = self.runtime.spawn_blocking(move || {
            if query.trim().is_empty() {
                documents
                    .iter()
                    .filter(|document| !document.source.archived)
                    .take(8)
                    .map(|document| search::Hit {
                        document: document.clone(),
                        message: None,
                        range: 0..0,
                        snippet: document.source.cwd.to_string_lossy().into_owned(),
                        snippet_range: 0..0,
                    })
                    .collect()
            } else {
                search::search(&documents, &query)
            }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(hits) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.hits = hits;
                    this.selected = 0;
                    this.reset_scroll(cx);
                    this.loading = false;
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn actions(&self, cx: &App) -> Vec<PaletteAction> {
        let query = self.input.read(cx).value().trim().to_lowercase();
        PaletteAction::ALL
            .into_iter()
            .filter(|action| query.split_whitespace().all(|word| action.keywords().contains(word)))
            .collect()
    }

    fn reset_scroll(&mut self, cx: &App) {
        let actions = self.actions(cx).len();
        self.scroll.reset(
            actions + self.hits.len() + usize::from(actions > 0) + usize::from(!self.hits.is_empty()),
        );
    }

    fn selected_list_index(&self, cx: &App) -> usize {
        let actions = self.actions(cx).len();
        self.selected + usize::from(actions > 0) + usize::from(self.selected >= actions)
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let actions = self.actions(cx);
        if let Some(action) = actions.get(self.selected) {
            self.chosen_action = Some(*action);
            cx.emit(PaletteEvent::Close);
        } else if let Some(hit) = self.hits.get(self.selected.saturating_sub(actions.len())) {
            cx.emit(PaletteEvent::Open(hit.clone()));
        }
    }

    fn action_row(&self, action: PaletteAction, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .id(("palette-action", ix))
            .role(Role::Button)
            .aria_label(action.label())
            .w_full()
            .h(zpx(42.))
            .px(zpx(12.))
            .gap(zpx(12.))
            .items_center()
            .rounded(zpx(8.))
            .cursor_pointer()
            .bg(if ix == self.selected { rgba(0xffffff0c) } else { rgba(0xffffff00) })
            .hover(|row| row.bg(rgba(0xffffff09)))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                this.open(cx);
            }))
            .child(div().flex_none().text_color(rgba(0xffffff80)).child(crate::icons::path(action.icon())))
            .child(div().flex_1().min_w_0().truncate().text_size(zpx(14.)).child(action.label()))
            .child(key_badge(action.shortcut()))
            .into_any_element()
    }

    fn list_row(&mut self, mut ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let actions = self.actions(cx);
        if !actions.is_empty() {
            if ix == 0 {
                return section_label("Actions").into_any_element();
            }
            ix -= 1;
            if let Some(action) = actions.get(ix) {
                return self.action_row(*action, ix, cx);
            }
            ix -= actions.len();
        }
        if ix == 0 {
            return section_label(if self.input.read(cx).value().trim().is_empty() {
                "Recent conversations"
            } else {
                "Conversations"
            }).into_any_element();
        }
        self.result(ix - 1, cx)
    }

    fn result(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(hit) = self.hits.get(ix).cloned() else {
            return div().into_any_element();
        };
        let recent = self.input.read(cx).value().trim().is_empty();
        let action_count = self.actions(cx).len();
        let preview = if hit
            .message
            .is_some_and(|i| hit.document.messages[i].role == "Assistant")
        {
            let preview = self.previews.entry(ix).or_insert_with(|| {
                let view = cx.new(|cx| TextViewState::markdown(&hit.snippet, cx));
                cx.observe(&view, |_, _, cx| cx.notify()).detach();
                Preview {
                    view,
                    highlighted: false,
                }
            });
            if !preview.highlighted {
                preview.highlighted = preview.view.update(cx, |view, cx| {
                    let Some(range) = view
                        .rendered_text()
                        .range_for_source(hit.snippet_range.clone())
                    else {
                        return false;
                    };
                    view.set_range_highlights(
                        [gpui_kit::base::text::RangeHighlight::new(
                            range,
                            crate::appearance::rgba(0xffffff24),
                        )],
                        cx,
                    )
                    .is_ok()
                });
            }
            let palette = cx.entity().downgrade();
            TextView::new(&preview.view)
                .style(
                    TextViewStyle::default()
                        .paragraph_gap(rems(0.))
                        .heading_font_size(|_, _| zpx(12.)),
                )
                .max_lines(1)
                .selectable(false)
                .stream_fade(false)
                .on_link_click(move |_, _, _, cx| {
                    // A preview link opens its conversation, like the rest of the row.
                    cx.stop_propagation();
                    let _ = palette.update(cx, |this, cx| {
                        this.selected = this.actions(cx).len() + ix;
                        this.open(cx);
                    });
                })
                .into_any_element()
        } else {
            StyledText::new(hit.snippet.clone())
                .with_highlights([(
                    hit.snippet_range.clone(),
                    HighlightStyle {
                        background_color: Some(crate::appearance::rgba(0xffffff24).into()),
                        color: Some(crate::appearance::rgb(0xffffff).into()),
                        ..Default::default()
                    },
                )])
                .into_any_element()
        };
        let source = &hit.document.source;
        let kind = if matches!(source.tab, Tab::Terminal(_)) {
            "Terminal"
        } else {
            "Chat"
        };
        let role = hit
            .message
            .map(|i| hit.document.messages[i].role)
            .unwrap_or("Title");
        let detail = if recent {
            kind.to_owned()
        } else {
            format!("{kind} · {role}{}", if source.archived { " · Archived" } else { "" })
        };
        h_flex()
            .w_full()
            .id(("search-result", ix))
            .role(Role::Button)
            .aria_label(format!("{}: {}", source.title, hit.snippet))
            .px(zpx(12.))
            .h(zpx(if recent { 50. } else { 64. }))
            .gap(zpx(12.))
            .items_center()
            .overflow_hidden()
            .rounded(zpx(8.))
            .cursor_pointer()
            .bg(if action_count + ix == self.selected {
                rgba(0xffffff0c)
            } else {
                rgba(0xffffff00)
            })
            .hover(|row| row.bg(rgba(0xffffff09)))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = this.actions(cx).len() + ix;
                this.open(cx);
            }))
            .child(
                div()
                    .flex_none()
                    .text_color(rgba(0xffffff80))
                    .child(crate::icons::path(if matches!(source.tab, Tab::Terminal(_)) {
                        "icons/square-terminal.svg"
                    } else {
                        "icons/messages-square.svg"
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(zpx(3.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(zpx(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(zpx(14.))
                                    .text_color(rgb(0xeeeeef))
                                    .child(source.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(zpx(11.))
                                    .text_color(rgba(0xffffff70))
                                    .child(detail),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(zpx(12.))
                            .line_height(relative(1.4))
                            .text_color(rgba(0xffffff88))
                            .child(preview),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgba(0xffffff60))
                    .child(crate::icons::path("icons/return.svg")),
            )
            .into_any_element()
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let empty = self.input.read(cx).value().trim().is_empty();
        let actions = self.actions(cx).len();
        let row_height = if empty { 50. } else { 64. };
        let groups = usize::from(actions > 0) + usize::from(!self.hits.is_empty());
        let content_height = 42. * actions as f32 + row_height * self.hits.len() as f32 + 32. * groups as f32;
        let max_height = (window.viewport_size().height - zpx(260.))
            .max(zpx(84.))
            .min(zpx(440.));
        let results_height = zpx(content_height).min(max_height);
        v_flex()
            .id("conversation-search")
            .w_full()
            .text_color(rgb(0xeeeeef))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.key == "escape" || (key.modifiers.platform && key.key == "k") {
                    this.chosen_action = None;
                    cx.emit(PaletteEvent::Close);
                } else if !key.modifiers.platform && !key.modifiers.control && !key.modifiers.alt {
                    let count = this.actions(cx).len() + this.hits.len();
                    match key.key.as_str() {
                        "up" | "down" if count > 0 => {
                            this.selected = if key.key == "down" {
                                (this.selected + 1) % count
                            } else {
                                (this.selected + count - 1) % count
                            };
                            this.scroll.scroll_to_reveal_item(this.selected_list_index(cx));
                            cx.notify();
                        }
                        "enter" => this.open(cx),
                        _ => return,
                    }
                } else {
                    return;
                }
                cx.stop_propagation();
                window.prevent_default();
            }))
            .child(
                h_flex()
                    .h(zpx(72.))
                    .flex_none()
                    .px(zpx(20.))
                    .gap(zpx(12.))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgba(0xffffff70))
                            .child(crate::icons::icon(IconName::Search)),
                    )
                    .child(
                        Input::new(&self.input)
                            .large()
                            .flex_1()
                            .min_w_0()
                            .px_0()
                            .text_size(zpx(16.))
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false),
                    ),
            )
            .child(
                v_flex()
                    .px(zpx(8.))
                    .pb(zpx(8.))
                    .when(content_height > 0., |body| {
                        body.child(
                            list(self.scroll.clone(), move |ix, _, cx| {
                                entity.update(cx, |this, cx| this.list_row(ix, cx))
                            })
                            .h(results_height)
                            .w_full(),
                        )
                    })
                    .when(self.hits.is_empty(), |body| {
                        body.child(
                            div()
                                .px(zpx(12.))
                                .py(zpx(14.))
                                .text_size(zpx(12.))
                                .text_color(rgba(0xffffff70))
                                .child(if self.loading {
                                    if empty { "Loading recent conversations…" } else { "Searching conversations…" }
                                } else if empty {
                                    "Your recent conversations will appear here."
                                } else if actions > 0 {
                                    "No matching conversations."
                                } else {
                                    "No matching commands or conversations."
                                }),
                        )
                    }),
            )
            .when(!self.errors.is_empty(), |panel| {
                panel.child(
                    div()
                        .px(zpx(20.))
                        .pb(zpx(12.))
                        .text_size(zpx(11.))
                        .text_color(rgba(0xe8b04aff))
                        .child(format!(
                            "{} transcript(s) could not be read. Other results are available.",
                            self.errors.len()
                        )),
                )
            })
            .child(
                h_flex()
                    .h(zpx(46.))
                    .flex_none()
                    .px(zpx(16.))
                    .gap(zpx(18.))
                    .items_center()
                    .border_t_1()
                    .border_color(rgba(0xffffff0a))
                    .bg(rgb(0x1b1b1d))
                    .text_size(zpx(12.))
                    .text_color(rgba(0xffffff80))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(zpx(4.))
                            .child(key_badge("↑"))
                            .child(key_badge("↓"))
                            .child(div().ml(zpx(3.)).child("Navigate")),
                    )
                    .child(key_hint("Enter", "Select"))
                    .child(key_hint("Esc", "Close")),
            )
    }
}

fn section_label(label: &'static str) -> impl IntoElement {
    div()
        .h(zpx(32.))
        .px(zpx(12.))
        .flex()
        .items_center()
        .text_size(zpx(12.))
        .text_color(rgba(0xffffff70))
        .child(label)
}

fn key_badge(label: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .px(zpx(5.))
        .min_w(zpx(22.))
        .h(zpx(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(zpx(4.))
        .bg(rgba(0xffffff0d))
        .text_size(zpx(11.))
        .text_color(rgb(0xd4d4d6))
        .child(label)
}

fn key_hint(key: &'static str, label: &'static str) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(zpx(7.))
        .child(key_badge(key))
        .child(label)
}

const EXIT_DURATION: std::time::Duration = std::time::Duration::from_millis(140);

pub(super) fn exit_opacity(started: std::time::Instant) -> f32 {
    let t = (started.elapsed().as_secs_f32() / EXIT_DURATION.as_secs_f32()).min(1.);
    1. - t * t * (3. - 2. * t)
}

impl AppShell {
    fn close_search(
        &mut self,
        hit: Option<search::Hit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_palette(hit, None, window, cx);
    }

    fn close_palette(
        &mut self,
        hit: Option<search::Hit>,
        action: Option<PaletteAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_open || self.search_closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            window.close_dialog(cx);
            self.search_open = false;
            if let Some(hit) = hit {
                self.open_search_hit(hit, window, cx);
            }
            if let Some(action) = action {
                self.run_palette_action(action, window, cx);
            }
            cx.notify();
            return;
        }
        self.search_closing = Some(std::time::Instant::now());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(EXIT_DURATION).await;
            let _ = this.update_in(cx, |this, window, cx| {
                window.close_dialog(cx);
                this.search_open = false;
                this.search_closing = None;
                if let Some(hit) = hit {
                    this.open_search_hit(hit, window, cx);
                }
                if let Some(action) = action {
                    this.run_palette_action(action, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_palette_action(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            PaletteAction::NewConversation => {
                self.state.update(cx, |state, cx| state.new_session(cx));
                self.focus_composer(window, cx);
            }
            PaletteAction::Settings => {
                self.state.update(cx, |state, cx| {
                    if state.panel != crate::state::Panel::Providers {
                        state.toggle_providers(cx);
                    }
                });
            }
            PaletteAction::Files | PaletteAction::Terminal => {
                self.state.update(cx, |state, cx| {
                    if state.panel == crate::state::Panel::Providers {
                        state.toggle_providers(cx);
                    }
                });
                if matches!(action, PaletteAction::Files) {
                    self.set_right(!self.right_open, cx);
                    self.focus_composer(window, cx);
                } else {
                    if !self.right_open {
                        self.set_right(true, cx);
                    }
                    self.right.update(cx, |right, cx| right.show_terminal(window, cx));
                }
            }
        }
    }

    fn search_sources(&self, cx: &App) -> Vec<search::Source> {
        let state = self.state.read(cx);
        let mut sources: Vec<_> = state
            .rows()
            .into_iter()
            .chain(state.archived_rows())
            .map(|row| {
                let live = self
                    .chats
                    .get(&row.id)
                    .filter(|chat| !chat.read(cx).is_loading())
                    .map(|chat| chat.read(cx).search_messages());
                search::Source {
                    tab: Tab::Session(row.id.clone()),
                    title: row.title,
                    provider: Some(row.provider),
                    cwd: row.cwd,
                    conversation_id: Some(row.id.0),
                    archived: row.archived,
                    live,
                }
            })
            .collect();
        sources.extend(
            self.terminal_store
                .records
                .iter()
                .map(|record| search::Source {
                    tab: Tab::Terminal(record.id.clone()),
                    title: state.current_title(&record.id),
                    provider: record.launch.provider(),
                    cwd: record.launch.cwd.clone(),
                    conversation_id: record.conversation_id.clone(),
                    archived: state.terminal_archived(&record.id),
                    live: None,
                }),
        );
        sources
    }

    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            self.close_search(None, window, cx);
            return;
        }
        self.state.update(cx, |state, cx| {
            state.shortcut_hints = false;
            cx.notify();
        });
        self.settle_dictation(window, cx);
        self.cancel_dictation(cx);
        let runtime = self.state.read(cx).backend().runtime;
        let palette = cx.new(|cx| Palette::new(runtime.clone(), window, cx));
        palette.update(cx, |palette, cx| palette.reset_scroll(cx));
        cx.subscribe_in(&palette, window, |this, palette, event, window, cx| {
            let hit = match event {
                PaletteEvent::Open(hit) => Some(hit.clone()),
                PaletteEvent::Close => None,
            };
            let action = if matches!(event, PaletteEvent::Close) {
                palette.read(cx).chosen_action
            } else {
                None
            };
            this.close_palette(hit, action, window, cx);
        })
        .detach();
        let sources = self.search_sources(cx);
        let index = self.search_index.clone();
        let job = runtime.spawn_blocking(move || {
            index
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .refresh(sources)
        });
        let weak = palette.downgrade();
        cx.spawn(async move |_, cx| match job.await {
            Ok(snapshot) => {
                let _ = weak.update(cx, |palette, cx| {
                    palette.indexing = false;
                    palette.documents = snapshot.documents;
                    palette.errors = snapshot.errors;
                    palette.search(cx);
                });
            }
            Err(error) => {
                let _ = weak.update(cx, |palette, cx| {
                    palette.indexing = false;
                    palette.errors.push(error.to_string());
                    palette.loading = false;
                    cx.notify();
                });
            }
        })
        .detach();
        self.search_open = true;
        let shell = cx.weak_entity();
        let content = palette.clone();
        let page_bounds = self.page_bounds.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let closing = shell
                .upgrade()
                .and_then(|shell| shell.read(cx).search_closing);
            if closing.is_some() {
                window.request_animation_frame();
            }
            let cancel_shell = shell.clone();
            let shell = shell.clone();
            // Align with the actual conversation surface, including resized sidebars.
            let bounds = page_bounds.get();
            let width = zpx(620.).min((bounds.size.width - zpx(32.)).max(zpx(0.)));
            let offset = bounds.center().x - window.viewport_size().width / 2.;
            dialog
                .bg(rgb(0x18181a))
                .p_0()
                .rounded(zpx(18.))
                .border_1()
                .border_color(rgba(0xffffff14))
                .overflow_hidden()
                .entrance(if closing.is_some() {
                    DialogEntrance::None
                } else {
                    DialogEntrance::FadeSlide
                })
                .when_some(closing, |dialog, started| {
                    dialog.opacity(exit_opacity(started))
                })
                .width(width)
                .relative()
                .left(offset)
                .keyboard(false)
                .close_button(false)
                .on_cancel(move |_, window, cx| {
                    let _ = cancel_shell.update(cx, |this, cx| this.close_search(None, window, cx));
                    false
                })
                .on_close(move |_, _, cx| {
                    let _ = shell.update(cx, |this, cx| {
                        this.search_open = false;
                        cx.notify();
                    });
                })
                .child(content.clone())
        });
        // The dialog must first mount its focus scope before the input can focus.
        window.defer(cx, move |window, cx| {
            palette.update(cx, |palette, cx| {
                palette
                    .input
                    .update(cx, |input, cx| input.focus(window, cx))
            });
        });
        cx.notify();
    }

    fn open_search_hit(&mut self, hit: search::Hit, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_search_generation += 1;
        let tab = hit.document.source.tab.clone();
        // Results are snapshots: reject a row deleted while the palette was open.
        let exists = match &tab {
            Tab::Session(id) => self
                .state
                .read(cx)
                .backend()
                .manager
                .list_sessions()
                .iter()
                .any(|r| &r.id == id),
            Tab::Terminal(id) => self.terminal_store.records.iter().any(|r| &r.id == id),
            Tab::Draft(_) => false,
        };
        if !exists {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.select_tab(&tab, cx);
            state.keep_tab(&tab, cx);
        });
        match tab {
            Tab::Session(_) => {
                self.ensure_chat(cx);
                if let Some(chat) = self.open_chat(cx) {
                    chat.update(cx, |chat, cx| chat.reveal_search_hit(hit, cx));
                }
            }
            Tab::Terminal(id) => {
                let restoring = !self.terminals.contains_key(&id);
                self.ensure_modes(window, cx);
                self.reveal_terminal_hit(id, hit, restoring, window, cx);
            }
            Tab::Draft(_) => {}
        }
        self.mode_focus = Some((
            self.state.read(cx).selection.clone(),
            self.state.read(cx).mode(),
        ));
        self.focus_composer(window, cx);
    }

    fn reveal_terminal_hit(
        &mut self,
        id: del_agent::SessionId,
        hit: search::Hit,
        restoring: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message_ix) = hit.message else {
            return;
        };
        let message = &hit.document.messages[message_ix];
        let range = hit.range.clone();
        let context = message.text.clone();
        self.terminal_search_generation += 1;
        let generation = self.terminal_search_generation;
        cx.spawn_in(window, async move |this, cx| {
            let mut restore_wait = None;
            // Resuming a CLI paints asynchronously. Retry while it restores its history.
            for attempt in 0..120 {
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                let done = this.update_in(cx, |this, window, cx| {
                    if this.terminal_search_generation != generation
                        || this.state.read(cx).selection != crate::state::Selection::Terminal(id.clone()) {
                        return true;
                    }
                    if let Some(host) = this.terminals.get(&id) {
                        if restoring {
                            let now = std::time::Instant::now();
                            let output = &host.read(cx).snapshot().last_output;
                            let wait = restore_wait.get_or_insert_with(|| RestoreWait::new(output, now));
                            if !wait.ready(output, now) {
                                if attempt == 119 {
                                    window.push_notification(
                                        gpui_kit::component::notification::Notification::info(
                                            "The terminal is still restoring output. Search again once it finishes to jump to this passage."
                                        ), cx);
                                }
                                return false;
                            }
                        }
                        let terminal = host.read(cx).terminal_view();
                        if terminal.update(cx, |terminal, cx| terminal.reveal_search_match(&context, range.clone(), cx)) {
                            return true;
                        }
                    }
                    if attempt == 119 {
                        window.push_notification(
                            gpui_kit::component::notification::Notification::info(
                                "Opened the terminal. This passage is saved in its history but isn't currently in the terminal scrollback."
                            ), cx);
                    }
                    false
                }).unwrap_or(true);
                if done { break; }
            }
        }).detach();
    }
}

/// A resumed TUI can paint the match before the rest of the transcript. Wait
/// for the visible output to settle, ignoring cursor-only wakeups.
struct RestoreWait {
    started: std::time::Instant,
    stable_since: std::time::Instant,
    output: String,
}
impl RestoreWait {
    fn new(output: &str, now: std::time::Instant) -> Self {
        Self {
            started: now,
            stable_since: now,
            output: output.to_owned(),
        }
    }
    fn ready(&mut self, output: &str, now: std::time::Instant) -> bool {
        if self.output != output {
            self.output = output.to_owned();
            self.stable_since = now;
        }
        !output.trim().is_empty()
            && now.duration_since(self.started) >= std::time::Duration::from_secs(2)
            && now.duration_since(self.stable_since) >= std::time::Duration::from_secs(1)
    }
}

pub fn new_index() -> Arc<Mutex<search::Index>> {
    Arc::new(Mutex::new(search::Index::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::gpui;
    use core::prelude::v1::test;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn resumed_terminal_waits_past_early_match_and_later_restoration_chunks() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut wait = RestoreWait::new("", now);
        assert!(!wait.ready("", now + Duration::from_secs(3)));
        assert!(!wait.ready("first chunk with match", now + Duration::from_secs(4)));
        assert!(!wait.ready("first chunk with match", now + Duration::from_millis(4400)));
        assert!(!wait.ready(
            "remaining transcript and prompt",
            now + Duration::from_millis(4800)
        ));
        assert!(!wait.ready(
            "remaining transcript and prompt",
            now + Duration::from_millis(5500)
        ));
        assert!(wait.ready(
            "remaining transcript and prompt",
            now + Duration::from_millis(5800)
        ));
        // Cursor-only redraws don't change the text or restart the settling clock.
        assert!(wait.ready(
            "remaining transcript and prompt",
            now + Duration::from_secs(6)
        ));
    }

    struct PreviewFixture(Entity<Palette>);
    impl Render for PreviewFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(600.))
                .child(self.0.update(cx, |palette, cx| palette.result(0, cx)))
        }
    }
    #[gpui::test]
    fn markdown_preview_renders_formatting_and_maps_search_highlight(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let palette = cx.new(|cx| Palette::new(runtime.handle().clone(), window, cx));
            palette.update(cx, |palette, _| {
                let document = Arc::new(search::Document {
                    source: search::Source {
                        tab: Tab::Session(del_agent::SessionId::new("markdown")),
                        title: "Preview".into(),
                        provider: Some(del_agent::Provider::Claude),
                        cwd: "/tmp".into(),
                        conversation_id: None,
                        archived: false,
                        live: None,
                    },
                    messages: Arc::new(vec![search::Message {
                        role: "Assistant",
                        text: "**Cats** and *purrs*, `code`, [a link](https://example.com).".into(),
                    }]),
                });
                palette.hits = search::search(&[document], "Cats");
            });
            PreviewFixture(palette)
        });
        for _ in 0..8 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        let palette = root.read_with(cx, |root, _| root.0.clone());
        palette.read_with(cx, |palette, cx| {
            let preview = &palette.previews[&0];
            assert!(preview.highlighted);
            let rendered = preview.view.read(cx).rendered_text();
            assert_eq!(rendered.as_str().trim(), "Cats and purrs, code, a link.");
            let range = rendered
                .range_for_source(palette.hits[0].snippet_range.clone())
                .unwrap();
            assert_eq!(&rendered.as_str()[range], "Cats");
        });
    }

    struct Fixture(Entity<Palette>);
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(620.)).h(px(480.)).child(self.0.clone())
        }
    }
    #[gpui::test]
    fn palette_keyboard_opens_selected_match_and_escape_closes(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let opened = Rc::new(RefCell::new(Vec::new()));
        let events = opened.clone();
        let (root, cx) = cx.add_window_view(|window, cx| {
            let palette = cx.new(|cx| Palette::new(runtime.handle().clone(), window, cx));
            cx.subscribe(&palette, move |_, _, event, _| {
                events.borrow_mut().push(match event {
                    PaletteEvent::Open(hit) => hit.range.start as i32,
                    PaletteEvent::Close => -1,
                })
            })
            .detach();
            palette.update(cx, |palette, cx| {
                let document = Arc::new(search::Document {
                    source: search::Source {
                        tab: Tab::Terminal(del_agent::SessionId::new("terminal")),
                        title: "Test task".into(),
                        provider: Some(del_agent::Provider::Claude),
                        cwd: "/tmp".into(),
                        conversation_id: None,
                        archived: false,
                        live: None,
                    },
                    messages: Arc::new(vec![search::Message {
                        role: "You",
                        text: "match second match".into(),
                    }]),
                });
                palette.hits = search::search(&[document], "match");
                palette.scroll.reset(palette.hits.len());
                palette.loading = false;
                palette
                    .input
                    .update(cx, |input, cx| input.focus(window, cx));
            });
            Fixture(palette)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("down enter up enter escape");
        cx.run_until_parked();
        assert_eq!(&*opened.borrow(), &[13, 0, -1]);
        let palette = root.read_with(cx, |root, _| root.0.clone());
        palette.read_with(cx, |palette, _| assert_eq!(palette.selected, 0));
    }
}
