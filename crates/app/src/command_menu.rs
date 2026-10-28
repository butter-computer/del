//! Composer autocomplete driven by the selected harness's command catalog.
use crate::{appearance::rgba, state::AppState, zoom::px as zpx};
use del_agent::{CommandKind, ComposerCommand, Provider};
use gpui_kit::{
    component::{
        input::{InlineToken, InputEvent, TextareaState},
        *,
    },
    prelude::*,
    *,
};
use std::path::PathBuf;

pub struct GoalSelected(pub ComposerCommand);
impl EventEmitter<GoalSelected> for CommandMenu {}

pub struct CommandMenu {
    input: Entity<TextareaState>,
    state: Entity<AppState>,
    context: Option<(Provider, Option<String>, PathBuf)>,
    commands: Vec<ComposerCommand>,
    chosen: Option<ComposerCommand>,
    matches: Vec<usize>,
    selected: usize,
    scroll: ListState,
    open: bool,
    loading: bool,
    error: Option<String>,
    generation: u64,
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

/// Complete a slash word after any already selected skills, without consuming
/// the other tokens or any trailing message text.
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
    let mut prefix_end = 0;
    for span in content.tokens() {
        let range = span.range();
        if range.start >= start {
            break;
        }
        if range.end > start || !text[prefix_end..range.start].trim().is_empty() {
            return None;
        }
        prefix_end = range.end;
    }
    if !text[prefix_end..start].trim().is_empty()
        || content
            .tokens()
            .iter()
            .any(|span| span.range().start < end && span.range().end > start)
    {
        return None;
    }
    Some(start..end)
}

fn allowed_after_skills(kind: CommandKind, after_skills: bool) -> bool {
    !after_skills || matches!(kind, CommandKind::Skill | CommandKind::Goal)
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
        cx.observe(&state, |this, _, cx| {
            let (provider, _, cwd) = this.state.read(cx).effective();
            if this.context.as_ref().is_some_and(|context| {
                context != &(provider, this.state.read(cx).selected_account(), cwd)
            }) {
                this.context = None;
                this.commands.clear();
                this.chosen = None;
                this.generation += 1;
                this.changed(cx);
            }
        })
        .detach();
        Self {
            input,
            state,
            context: None,
            commands: vec![],
            chosen: None,
            matches: vec![],
            selected: 0,
            scroll: ListState::new(0, ListAlignment::Top, zpx(150.)),
            open: false,
            loading: false,
            error: None,
            generation: 0,
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let open = query_range(&input.content(), input.selected_range()).is_some();
        if open && !self.open && !self.loading {
            self.context = None;
        }
        self.open = open;
        if self.open && self.context.is_none() {
            let (provider, _, cwd) = self.state.read(cx).effective();
            let account = self.state.read(cx).selected_account();
            self.context = Some((provider, account.clone(), cwd.clone()));
            self.loading = true;
            self.error = None;
            self.generation += 1;
            let generation = self.generation;
            let backend = self.state.read(cx).backend();
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
                    match result {
                        Ok(Ok(mut commands)) => {
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
                            this.commands = commands;
                        }
                        Ok(Err(error)) => this.error = Some(error.to_string()),
                        Err(error) => this.error = Some(error.to_string()),
                    }
                    this.filter(cx);
                });
            })
            .detach();
        }
        self.filter(cx);
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
        let after_skills = range.as_ref().is_some_and(|r| r.start > 0);
        self.matches = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, command)| {
                matches(command, query)
                    && allowed_after_skills(command.kind, after_skills)
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

    pub fn navigate(&mut self, delta: i32, cx: &mut Context<Self>) -> bool {
        if !self.open {
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
        if !self.open {
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
            input.set_value(format!("/{} ", command.name), window, cx);
            input.focus(window, cx);
        });
        self.open = false;
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
        h_flex()
            .id(("composer-command", ix))
            .role(Role::Button)
            .aria_label(format!("/{}: {}", command.name, command.description))
            .h(zpx(44.))
            .w_full()
            .px_2()
            .gap_2()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .bg(if ix == self.selected {
                rgba(0xffffff16)
            } else {
                rgba(0xffffff00)
            })
            .hover(|row| row.bg(rgba(0xffffff12)))
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
                    .gap_px()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(zpx(13.))
                                    .child(format!("/{}", command.name)),
                            )
                            .child(
                                div()
                                    .text_size(zpx(10.))
                                    .text_color(rgba(0xffffff66))
                                    .child(command.source.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(zpx(11.))
                            .text_color(rgba(0xffffff88))
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
        if self.state.read(cx).is_starting() {
            return true;
        }
        let enabled = match arguments {
            "" => self.state.read(cx).service_tier().is_none(),
            "on" => true,
            "off" => false,
            _ => {
                self.control_error("Use /fast on or /fast off", cx);
                return true;
            }
        };
        if enabled
            && self
                .state
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
        self.state
            .update(cx, |state, cx| state.set_fast_mode(enabled, cx));
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.open = false;
        true
    }

    fn control_error(&self, message: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.error = Some(message.into());
            cx.notify();
        });
    }
}

impl Render for CommandMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let height =
            zpx(44. * self.matches.len().min(6) as f32).min(window.viewport_size().height * 0.32);
        let message = self.error.clone().unwrap_or_else(|| {
            if self.loading {
                "Loading skills and commands…".into()
            } else {
                "No matching commands".into()
            }
        });
        v_flex()
            .id("composer-commands")
            .w_full()
            .p_1()
            .rounded_xl()
            .bg(crate::appearance::popup_surface())
            .shadow_lg()
            .when(self.matches.is_empty(), |menu| {
                menu.child(
                    div()
                        .px_3()
                        .py_3()
                        .text_size(zpx(12.))
                        .text_color(rgba(0xffffff88))
                        .child(message),
                )
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
                    .px_2()
                    .py_1()
                    .text_size(zpx(10.))
                    .text_color(rgba(0xffffff66))
                    .child("↑ ↓ Navigate   ↵ Select   esc Close"),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn goal_is_available_after_selected_skills() {
        use super::*;
        assert!(allowed_after_skills(CommandKind::Goal, true));
        assert!(allowed_after_skills(CommandKind::Skill, true));
        assert!(!allowed_after_skills(CommandKind::Action, true));
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
    fn menu_only_opens_for_unselected_prefix_commands() {
        use super::*;
        use gpui_kit::component::input::InputContent;
        for text in ["/", "/review"] {
            assert_eq!(
                query_range(&InputContent::new(text), text.len()..text.len()),
                Some(0..text.len())
            );
        }
        for text in ["/goal fix this", "see /review", "/goal\n"] {
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
