//! Composer autocomplete driven by the selected harness's command catalog.
use crate::{state::AppState, ui, zoom::px as zpx};
use del_agent::{CommandKind, ComposerCommand, Provider};
use gpui_kit::{
    component::{
        input::{InlineToken, InputEvent, TextareaState},
        *,
    },
    prelude::*,
    *,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct GoalSelected(pub ComposerCommand);
impl EventEmitter<GoalSelected> for CommandMenu {}

pub struct ModeSelected;
impl EventEmitter<ModeSelected> for CommandMenu {}

pub struct ControlError(pub String);
impl EventEmitter<ControlError> for CommandMenu {}

pub(crate) type CommandContext = (Provider, Option<String>, PathBuf);

fn local_commands(provider: Provider) -> Vec<ComposerCommand> {
    if provider == Provider::Acp {
        return vec![];
    }
    [
        (
            "terminal",
            "Continue this conversation in the provider terminal",
        ),
        ("chat", "Continue this conversation in Chat"),
    ]
    .into_iter()
    .map(|(name, description)| ComposerCommand {
        name: name.into(),
        description: description.into(),
        argument_hint: String::new(),
        kind: CommandKind::Action,
        source: "Del".into(),
        target: None,
    })
    .collect()
}

pub struct CommandMenu {
    input: Entity<TextareaState>,
    state: Option<Entity<AppState>>,
    backend: crate::chat::Backend,
    selection: Option<CommandContext>,
    context: Option<CommandContext>,
    commands: Vec<ComposerCommand>,
    cached: HashMap<CommandContext, (Vec<ComposerCommand>, Instant)>,
    last_refresh: Option<Instant>,
    chosen: Option<ComposerCommand>,
    matches: Vec<usize>,
    selected: usize,
    scroll: ListState,
    open: bool,
    loading: bool,
    error: Option<String>,
    generation: u64,
    /// Open state as last drawn, and a count of openings, so each opening
    /// replays the fade-in.
    presented: bool,
    openings: usize,
}

pub(super) fn selected_skills(
    content: &gpui_kit::component::input::InputContent,
) -> Vec<ComposerCommand> {
    content
        .tokens()
        .iter()
        .filter_map(|span| {
            let command: ComposerCommand =
                serde_json::from_str(span.token().id().strip_prefix("del-skill:")?).ok()?;
            (command.kind == CommandKind::Skill
                && content.text().get(span.range()) == Some(format!("/{}", command.name).as_str()))
            .then_some(command)
        })
        .collect()
}

pub(super) fn restore_skill_tokens(
    input: &del_agent::UserInput,
) -> gpui_kit::component::input::InputContent {
    use gpui_kit::component::input::InputContent;
    let mut content = InputContent::new(input.text.clone());
    let mut offset = 0;
    for word in input.text.split_whitespace() {
        let start = offset + input.text[offset..].find(word).unwrap();
        offset = start + word.len();
        let Some(skill) = input
            .skills
            .iter()
            .chain(input.command.iter())
            .find(|skill| {
                skill.kind == CommandKind::Skill
                    && word.strip_prefix('/') == Some(skill.name.as_str())
            })
        else {
            continue;
        };
        let token = InlineToken::new(
            format!(
                "del-skill:{}",
                serde_json::to_string(skill).expect("command metadata serializes")
            ),
            word.to_string(),
        )
        .with_label(skill.name.clone());
        content = content
            .with_token(start..offset, token)
            .expect("slash word is a valid token range");
    }
    content
}

/// Complete the slash word at the caret anywhere in the prompt, without
/// consuming surrounding text or already selected tokens.
fn query_range(
    content: &gpui_kit::component::input::InputContent,
    caret: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    if !caret.is_empty() {
        return None;
    }
    let text = content.text();
    let before = text.get(..caret.start)?;
    let start = before
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + before[i..].chars().next().unwrap().len_utf8());
    if !text.get(start..)?.starts_with('/') {
        return None;
    }
    let end = text[caret.start..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| caret.start + i);
    if content
        .tokens()
        .iter()
        .any(|span| span.range().start < end && span.range().end > start)
    {
        return None;
    }
    Some(start..end)
}

fn matches(command: &ComposerCommand, query: &str) -> bool {
    let query = query.to_lowercase();
    if query == "skills" {
        return command.kind == CommandKind::Skill;
    }
    command.name.to_lowercase().contains(&query)
        || command.description.to_lowercase().contains(&query)
}

impl CommandMenu {
    pub fn new(
        input: Entity<TextareaState>,
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let backend = state.read(cx).backend();
        let (provider, _, cwd) = state.read(cx).effective();
        let selection = (provider, state.read(cx).selected_account(), cwd);
        let mut menu = Self::for_side_chat(input, backend, cx);
        menu.state = Some(state.clone());
        menu.set_context(Some(selection), cx);
        cx.observe(&state, |this, state, cx| {
            let (provider, _, cwd) = state.read(cx).effective();
            this.set_context(Some((provider, state.read(cx).selected_account(), cwd)), cx);
        })
        .detach();
        menu
    }

    /// Side chats supply their own settings snapshot rather than consulting the
    /// selected main tab or borrowing their owner during a completion callback.
    pub fn for_side_chat(
        input: Entity<TextareaState>,
        backend: crate::chat::Backend,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.changed(cx);
            }
            if matches!(event, InputEvent::Blur) {
                this.open = false;
                cx.notify();
            }
        })
        .detach();
        Self {
            input,
            state: None,
            backend,
            selection: None,
            context: None,
            commands: vec![],
            cached: HashMap::new(),
            last_refresh: None,
            chosen: None,
            matches: vec![],
            selected: 0,
            scroll: ListState::new(0, ListAlignment::Top, zpx(150.)),
            open: false,
            loading: false,
            error: None,
            generation: 0,
            presented: false,
            openings: 0,
        }
    }

    pub fn set_context(&mut self, selection: Option<CommandContext>, cx: &mut Context<Self>) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        self.context = None;
        self.commands.clear();
        self.last_refresh = None;
        self.error = None;
        self.chosen = None;
        self.loading = false;
        self.generation += 1;
        if let Some((commands, refreshed)) =
            self.selection.as_ref().and_then(|key| self.cached.get(key))
        {
            self.commands = commands.clone();
            self.last_refresh = Some(*refreshed);
            self.context = self.selection.clone();
        }
        // Warm the selected provider/account/project before the first slash.
        self.preload(cx);
        self.changed(cx);
    }

    fn catalog(
        &self,
        provider: Provider,
        mut commands: Vec<ComposerCommand>,
    ) -> Vec<ComposerCommand> {
        if self.state.is_some() {
            let local = local_commands(provider);
            commands.retain(|command| !local.iter().any(|local| local.name == command.name));
            commands.extend(local);
        } else {
            commands.retain(|command| !matches!(command.name.as_str(), "chat" | "terminal"));
        }
        commands.sort_by_key(|command| {
            (
                match command.kind {
                    CommandKind::Skill => 0,
                    CommandKind::Fast | CommandKind::Goal => 1,
                    CommandKind::Action => 2,
                },
                command.name.clone(),
            )
        });
        commands
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let open = self.selection.is_some()
            && query_range(&input.content(), input.selected_range()).is_some();
        self.open = open;
        if self.open {
            self.preload(cx);
        }
        self.filter(cx);
    }

    fn preload(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let retry_after = if self.error.is_some() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(60)
        };
        if self.loading
            || (self.context.as_ref() == Some(&selection)
                && self
                    .last_refresh
                    .is_some_and(|at| at.elapsed() < retry_after))
        {
            return;
        }
        let (provider, account, cwd) = selection.clone();
        if self.context.is_none() {
            self.commands = self.catalog(provider, Vec::new());
        }
        self.context = Some(selection.clone());
        self.loading = true;
        self.error = None;
        self.generation += 1;
        let generation = self.generation;
        let backend = self.backend.clone();
        let task = backend.runtime.spawn(async move {
            backend
                .manager
                .account_commands(provider, account.as_deref(), &cwd)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                let refreshed = Instant::now();
                this.last_refresh = Some(refreshed);
                match result {
                    Ok(Ok(commands)) => {
                        this.commands = this.catalog(provider, commands);
                        this.cached
                            .insert(selection, (this.commands.clone(), refreshed));
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    Err(error) => this.error = Some(error.to_string()),
                }
                // Refresh in place; keep usable cached suggestions on failure.
                this.filter(cx);
            });
        })
        .detach();
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let content = input.content();
        let range = query_range(&content, input.selected_range());
        let query = range
            .as_ref()
            .map(|r| &content.text()[r.start + 1..r.end])
            .unwrap_or_default();
        let selected = selected_skills(&content);
        self.matches = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, command)| {
                matches(command, query)
                    && !selected
                        .iter()
                        .any(|skill| skill.name == command.name && skill.target == command.target)
            })
            .map(|(i, _)| i)
            .collect();
        let needle = query.to_lowercase();
        self.matches.sort_by_key(|index| {
            let name = self.commands[*index].name.to_lowercase();
            if name == needle {
                0
            } else if name.starts_with(&needle) {
                1
            } else if name.contains(&needle) {
                2
            } else {
                3
            }
        });
        self.selected = 0;
        self.scroll.reset(self.matches.len());
        cx.notify();
    }

    pub fn navigate(&mut self, delta: i32, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.open || !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        if !self.matches.is_empty() {
            self.selected =
                (self.selected as i32 + delta).rem_euclid(self.matches.len() as i32) as usize;
            self.scroll.scroll_to_reveal_item(self.selected);
        }
        cx.notify();
        true
    }

    pub fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.open || !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        if !self.matches.is_empty() {
            self.choose(window, cx);
        }
        true
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = self.open;
        self.open = false;
        cx.notify();
        was_open
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self
            .matches
            .get(self.selected)
            .map(|i| self.commands[*i].clone())
        else {
            return;
        };
        if command.kind == CommandKind::Goal {
            self.open = false;
            self.input.update(cx, |input, cx| {
                if let Some(range) = query_range(&input.content(), input.selected_range()) {
                    input.set_selected_range(range, cx);
                    input.replace("", window, cx);
                }
                input.focus(window, cx);
            });
            self.chosen = None;
            cx.emit(GoalSelected(command));
            cx.notify();
            return;
        }
        if command.kind == CommandKind::Skill {
            let id = format!(
                "del-skill:{}",
                serde_json::to_string(&command).expect("command metadata serializes")
            );
            let label = command.name.clone();
            let token = InlineToken::new(id, format!("/{}", command.name)).with_label(label);
            let result = self.input.update(cx, |input, cx| {
                let Some(range) = query_range(&input.content(), input.selected_range()) else {
                    return Ok(());
                };
                input.replace_range_with_token(range, token, window, cx)?;
                input.insert(" ", window, cx);
                input.focus(window, cx);
                Ok::<_, gpui_kit::component::input::InlineTokenError>(())
            });
            if let Err(error) = result {
                self.control_error(&error.to_string(), cx);
                return;
            }
            self.chosen = None;
            self.open = false;
            cx.notify();
            return;
        }
        self.chosen = Some(command.clone());
        self.input.update(cx, |input, cx| {
            if let Some(range) = query_range(&input.content(), input.selected_range()) {
                input.set_selected_range(range, cx);
                input.replace(format!("/{} ", command.name), window, cx);
            }
            input.focus(window, cx);
        });
        self.open = false;
        if command.source == "Del" && matches!(command.name.as_str(), "terminal" | "chat") {
            self.chosen = None;
            cx.emit(ModeSelected);
        }
        cx.notify();
    }

    pub fn selected_command(&self, text: &str, cx: &App) -> Option<ComposerCommand> {
        let (name, _) = del_agent::split_command(text)?;
        if let Some(command) = selected_skills(&self.input.read(cx).content())
            .into_iter()
            .next()
        {
            return Some(command);
        }
        self.chosen
            .as_ref()
            .filter(|command| command.name == name)
            .cloned()
    }

    fn row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let command = &self.commands[self.matches[ix]];
        let selector = format!("composer-command-{}", command.name);
        h_flex()
            .id(("composer-command", ix))
            .debug_selector(move || selector.clone())
            .role(Role::Button)
            .aria_label(format!("/{}: {}", command.name, command.description))
            .h(zpx(ui::ROW_TWO_LINE))
            .w_full()
            .px(zpx(ui::SPACE_MD))
            .gap(zpx(ui::SPACE_MD))
            .items_center()
            .rounded(zpx(ui::RADIUS_ROW))
            .cursor_pointer()
            // One highlight: pointing at a row selects it, as the arrow keys do.
            .when(ix == self.selected, |row| row.bg(ui::row_selected()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.selected != ix {
                    this.selected = ix;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ix;
                this.choose(window, cx);
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(zpx(1.))
                    .child(
                        h_flex()
                            .gap(zpx(ui::SPACE_MD))
                            .child(
                                div()
                                    .text_size(zpx(13.))
                                    .child(format!("/{}", command.name)),
                            )
                            .child(
                                div()
                                    .text_size(zpx(del_core::typography::TEXT_XS))
                                    .text_color(ui::faint_text())
                                    .child(command.source.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(zpx(del_core::typography::TEXT_XS))
                            .text_color(ui::secondary_text())
                            .truncate()
                            .child(command.description.clone()),
                    ),
            )
            .into_any_element()
    }

    /// Handle Del's cross-provider Fast control before it can become a prompt.
    pub fn submit_control(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.input.read(cx).value().to_string();
        let Some((name, arguments)) = del_agent::split_command(&text) else {
            return false;
        };
        if name == "skills" {
            self.input
                .update(cx, |input, cx| input.set_value("/", window, cx));
            self.changed(cx);
            return true;
        }
        if name == "goal"
            && arguments.is_empty()
            && let Some(command) = self
                .commands
                .iter()
                .find(|c| c.kind == CommandKind::Goal)
                .cloned()
        {
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.open = false;
            cx.emit(GoalSelected(command));
            return true;
        }
        if name != "fast" {
            return false;
        }
        let Some(state) = self.state.clone() else {
            return false;
        };
        if state.read(cx).is_starting() {
            return true;
        }
        let enabled = match arguments {
            "" => state.read(cx).service_tier().is_none(),
            "on" => true,
            "off" => false,
            _ => {
                self.control_error("Use /fast on or /fast off", cx);
                return true;
            }
        };
        if enabled
            && state
                .read(cx)
                .model_info()
                .and_then(|model| model.fast_tier)
                .is_none()
        {
            self.control_error(
                "Fast mode is unavailable for the selected model. Choose a supported model first.",
                cx,
            );
            return true;
        }
        state.update(cx, |state, cx| state.set_fast_mode(enabled, cx));
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.open = false;
        true
    }

    fn control_error(&self, message: &str, cx: &mut Context<Self>) {
        if let Some(state) = &self.state {
            state.update(cx, |state, cx| {
                state.error = Some(message.into());
                cx.notify();
            });
        } else {
            cx.emit(ControlError(message.into()));
        }
    }
}

impl Render for CommandMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            self.presented = false;
            return div().into_any_element();
        }
        if !self.presented {
            self.presented = true;
            self.openings += 1;
        }
        let target = zpx(ui::ROW_TWO_LINE * self.matches.len().min(6) as f32)
            .min(window.viewport_size().height * 0.32);
        // Filtering changes the row count with each keystroke; the list eases
        // to its new height from wherever it is instead of snapping.
        let height = gpui_kit::base::transition(
            (self.openings, "composer-commands-height"),
            target.as_f32(),
            gpui_kit::base::Transition::new(std::time::Duration::from_millis(140)),
            window,
            cx,
        );
        let height = px(height);
        let message = self.error.clone().unwrap_or_else(|| {
            if self.loading {
                "Loading skills and commands…".into()
            } else {
                "No matching commands".into()
            }
        });
        v_flex()
            .id("composer-commands")
            .debug_selector(|| "composer-commands".into())
            .w_full()
            .p(zpx(ui::SPACE_XS))
            .popover_style(cx)
            .rounded(zpx(ui::RADIUS_POPOVER))
            .when(self.matches.is_empty(), |menu| {
                menu.child(ui::empty_note(message).px(zpx(ui::SPACE_MD)))
            })
            .when(!self.matches.is_empty(), |menu| {
                menu.child(
                    list(
                        self.scroll.clone(),
                        cx.processor(|this, ix, _, cx| this.row(ix, cx)),
                    )
                    .h(height)
                    .w_full(),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .px(zpx(ui::SPACE_MD))
                    .py(zpx(ui::SPACE_XS))
                    .text_size(zpx(del_core::typography::TEXT_XS))
                    .text_color(ui::faint_text())
                    .child("↑ ↓ Navigate   ↵ Select   Esc Close"),
            )
            .with_animation(
                ("composer-commands-enter", self.openings),
                Animation::new(ui::POPOVER_ENTER).with_easing(ui::ease_out),
                |menu, delta| menu.opacity(delta),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {

    #[gpui::test]
    fn main_catalog_preloads_and_reopening_keeps_ready_suggestions(cx: &mut gpui::TestAppContext) {
        use super::*;
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        let (menu, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| TextareaState::new(window, cx));
            CommandMenu::new(input, state, cx)
        });
        cx.update(|window, cx| {
            menu.update(cx, |menu, cx| {
                assert!(!menu.open);
                assert!(
                    menu.loading,
                    "main catalog must load before a slash is typed"
                );
                assert_eq!(menu.context, menu.selection);
                // Complete the parked provider request with a ready catalog.
                menu.loading = false;
                menu.last_refresh = Some(Instant::now());
                menu.commands = local_commands(Provider::Codex);
                let generation = menu.generation;
                for _ in 0..2 {
                    menu.input.update(cx, |input, cx| {
                        input.set_value("please /", window, cx);
                        input.set_selected_range(8..8, cx);
                    });
                    menu.changed(cx);
                    assert!(menu.open);
                    assert!(!menu.loading, "opening must reuse the warm catalog");
                    assert_eq!(menu.matches.len(), 2);
                    assert_eq!(menu.generation, generation);
                    menu.dismiss(cx);
                }
                menu.last_refresh = Some(Instant::now() - Duration::from_secs(61));
                menu.changed(cx);
                assert!(menu.loading, "stale catalogs refresh in the background");
                assert_eq!(
                    menu.matches.len(),
                    2,
                    "refresh must retain ready suggestions"
                );
            })
        });
    }

    #[gpui::test]
    fn inline_completion_preserves_prompt_and_skill_metadata(cx: &mut gpui::TestAppContext) {
        use super::*;
        cx.update(gpui_kit::init);
        let state = crate::state::tests::draft(cx);
        let commands: Vec<_> = [
            CommandKind::Skill,
            CommandKind::Action,
            CommandKind::Goal,
            CommandKind::Fast,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, kind)| ComposerCommand {
            name: format!("command{i}"),
            description: String::new(),
            argument_hint: String::new(),
            kind,
            source: "test".into(),
            target: (kind == CommandKind::Skill).then(|| "/repo/skills/review/SKILL.md".into()),
        })
        .collect();
        let skill = commands[0].clone();
        let (menu, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| TextareaState::new(window, cx));
            input.update(cx, |input, cx| {
                input.set_value("please / later", window, cx);
                input.set_selected_range(8..8, cx);
            });
            let mut menu = CommandMenu::new(input, state, cx);
            // Supply the catalog locally; this test exercises menu filtering and
            // completion without launching or authenticating a provider CLI.
            menu.context = menu.selection.clone();
            menu.loading = true;
            menu.commands = commands;
            menu.changed(cx);
            menu
        });
        cx.update(|window, cx| {
            menu.update(cx, |menu, cx| {
                assert!(menu.open);
                assert_eq!(
                    menu.matches.len(),
                    4,
                    "all command kinds remain available after prose"
                );
                menu.choose(window, cx);
                let content = menu.input.read(cx).content();
                assert_eq!(content.text().as_ref(), "please /command0  later");
                assert_eq!(selected_skills(&content), vec![skill.clone()]);

                let input = del_agent::UserInput {
                    text: "please /command0 then / later".into(),
                    skills: vec![skill.clone()],
                    ..del_agent::UserInput::text("")
                };
                menu.input.update(cx, |input_state, cx| {
                    input_state.set_value(restore_skill_tokens(&input), window, cx);
                    input_state.set_selected_range(23..23, cx);
                });
                menu.open = true;
                menu.filter(cx);
                assert_eq!(
                    menu.matches.len(),
                    3,
                    "only the already selected skill is excluded"
                );
                menu.choose(window, cx);
                let content = menu.input.read(cx).content();
                assert_eq!(
                    content.text().as_ref(),
                    "please /command0 then /command1  later"
                );
                assert_eq!(selected_skills(&content), vec![skill]);
            });
        });
    }

    #[test]
    fn slash_suggestions_open_after_prompt_text() {
        use super::*;
        use gpui_kit::component::input::InputContent;
        for text in [
            "cool cool is everything lit as of now /",
            "please /review",
            "héllo\n/skills",
        ] {
            let start = text.rfind('/').unwrap();
            assert_eq!(
                query_range(&InputContent::new(text), text.len()..text.len()),
                Some(start..text.len()),
            );
        }
        let text = "please /review this diff";
        assert_eq!(query_range(&InputContent::new(text), 10..10), Some(7..14));
    }

    #[test]
    fn goal_is_available_after_selected_skills() {
        use super::*;
        let content = gpui_kit::component::input::InputContent::new("/docs /morning /goal")
            .with_token(0..5, InlineToken::new("docs", "/docs"))
            .unwrap()
            .with_token(6..14, InlineToken::new("morning", "/morning"))
            .unwrap();
        assert_eq!(query_range(&content, 20..20), Some(15..20));
    }

    #[test]
    fn skill_token_keeps_exact_provider_target_and_plain_text_stays_plain() {
        use super::*;
        use gpui_kit::component::input::InputContent;
        let command = ComposerCommand {
            name: "review".into(),
            description: "Review".into(),
            argument_hint: String::new(),
            kind: CommandKind::Skill,
            source: "repo".into(),
            target: Some("/repo/specific/SKILL.md".into()),
        };
        let token = InlineToken::new(
            format!("del-skill:{}", serde_json::to_string(&command).unwrap()),
            "/review",
        )
        .with_label("review");
        let content = InputContent::new("/review check this")
            .with_token(0..7, token)
            .unwrap();
        let saved_draft = content.clone();
        assert_eq!(selected_skills(&saved_draft), vec![command]);
        assert_eq!(saved_draft.text().as_ref(), "/review check this");
        assert_eq!(
            selected_skills(&InputContent::new("/review check this")),
            vec![]
        );
        assert_eq!(selected_skills(&InputContent::new("")), vec![]);
    }

    #[test]
    fn multiple_skills_keep_paths_through_queue_and_editing() {
        use super::*;
        let mut input = del_agent::UserInput::text("/review /plugin:check please inspect this");
        input.skills = ["review", "plugin:check"]
            .into_iter()
            .map(|name| ComposerCommand {
                name: name.into(),
                description: String::new(),
                argument_hint: String::new(),
                kind: CommandKind::Skill,
                source: "test".into(),
                target: Some(format!("/skills/{name}/SKILL.md")),
            })
            .collect();
        let restored: del_agent::UserInput =
            serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        let content = restore_skill_tokens(&restored);
        assert_eq!(selected_skills(&content), input.skills);
        assert_eq!(content.text().as_ref(), input.text);
        assert_eq!(content.tokens().len(), 2);
        let content = gpui_kit::component::input::InputContent::new("/review /plugin:check /thi")
            .with_token(0..7, content.tokens()[0].token().clone())
            .unwrap()
            .with_token(8..21, content.tokens()[1].token().clone())
            .unwrap();
        assert_eq!(query_range(&content, 26..26), Some(22..26));
    }

    #[test]
    fn filtering_matches_name_or_description_case_insensitively() {
        use super::*;
        let command = |name: &str, description: &str, kind| ComposerCommand {
            name: name.into(),
            description: description.into(),
            argument_hint: String::new(),
            kind,
            source: "test".into(),
            target: None,
        };
        let review = command("Review", "Inspect the diff", CommandKind::Skill);
        let compact = command("compact", "Summarize context", CommandKind::Action);
        assert!(matches(&review, ""));
        assert!(matches(&review, "rev"));
        assert!(matches(&review, "REVIEW"));
        assert!(matches(&review, "diff"));
        assert!(!matches(&review, "compact"));
        assert!(matches(&compact, "summar"));
        // `/skills` lists only skills, rather than matching by text.
        assert!(matches(&review, "skills"));
        assert!(!matches(&compact, "skills"));
        assert!(!matches(
            &command("skills-sync", "Sync skills", CommandKind::Action),
            "Skills"
        ));
    }

    #[test]
    fn local_mode_commands_are_offered_except_for_acp() {
        use super::*;
        for provider in [Provider::Codex, Provider::Claude, Provider::OpenCode] {
            let names: Vec<_> = local_commands(provider)
                .into_iter()
                .map(|command| {
                    assert_eq!(command.source, "Del");
                    assert_eq!(command.kind, CommandKind::Action);
                    assert!(command.argument_hint.is_empty());
                    command.name
                })
                .collect();
            assert_eq!(names, ["terminal", "chat"]);
        }
        assert!(local_commands(Provider::Acp).is_empty());
    }

    #[test]
    fn query_covers_the_whole_slash_word_around_the_caret() {
        use super::*;
        use gpui_kit::component::input::InputContent;
        let content = InputContent::new("/review");
        assert_eq!(query_range(&content, 4..4), Some(0..7));
        assert_eq!(query_range(&content, 0..0), Some(0..7));
        // Selections and invalid UTF-8 caret offsets never open it.
        assert_eq!(query_range(&content, 1..4), None);
        assert_eq!(
            query_range(&InputContent::new("héllo /re"), 10..10),
            Some(7..10)
        );
        assert_eq!(query_range(&InputContent::new("é"), 1..1), None);
        assert_eq!(query_range(&InputContent::new("/re"), 9..9), None);
        // Unicode whitespace before the slash is still only whitespace.
        let text = "\u{3000}/re";
        assert_eq!(
            query_range(&InputContent::new(text), text.len()..text.len()),
            Some(3..text.len())
        );
    }

    #[test]
    fn menu_only_opens_for_unselected_slash_words() {
        use super::*;
        use gpui_kit::component::input::InputContent;
        for text in ["/", "/review"] {
            assert_eq!(
                query_range(&InputContent::new(text), text.len()..text.len()),
                Some(0..text.len())
            );
        }
        for text in [
            "/goal fix this",
            "/goal\n",
            "https://example.com",
            "src/main.rs",
        ] {
            assert_eq!(
                query_range(&InputContent::new(text), text.len()..text.len()),
                None
            );
        }
        let content = InputContent::new("/review /plug:check details")
            .with_token(0..7, InlineToken::new("skill", "/review"))
            .unwrap();
        assert_eq!(query_range(&content, 19..19), Some(8..19));
        assert_eq!(query_range(&content, 7..7), None);
        assert_eq!(query_range(&content, 8..19), None);
    }
}
