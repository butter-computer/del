//! Independent side conversations with durable local context and scoped settings.
use crate::appearance::rgba;
use crate::chat::{Backend, ChatView};
use crate::zoom::px as zpx;
use del_agent::{ForkLifetime, ForkSource, Provider, SessionConfig, SessionId, UserInput};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Context {
    pub backend: Backend,
    pub source: ForkSource,
    pub accounts: Vec<crate::accounts::Account>,
}

#[derive(Clone)]
pub struct SendToMain {
    pub parent: SessionId,
    pub text: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct SavedChat {
    snapshot: Option<del_agent::ForkSnapshot>,
    draft: UserInput,
    queue: Vec<crate::followups::Followup>,
}

pub struct SideChat {
    context: Context,
    storage_id: String,
    saved: SavedChat,
    session: Option<SessionId>,
    parent: Option<SessionId>,
    chat: Option<Entity<ChatView>>,
    input: Entity<TextareaState>,
    attachments: Vec<del_agent::Attachment>,
    models: Vec<del_agent::ModelInfo>,
    model_picker: Entity<crate::model_picker::ModelPicker>,
    models_loading: bool,
    model_generation: u64,
    save_pending: bool,
    storage_error: bool,
    importing: bool,
    loading: bool,
    closed: bool,
    error: Option<String>,
}
impl EventEmitter<SendToMain> for SideChat {}
impl EventEmitter<crate::chat::OpenLink> for SideChat {}

impl SideChat {
    pub fn new(context: Context, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        Self::restore(context, uuid::Uuid::new_v4().to_string(), window, cx)
    }

    pub fn storage_id(&self) -> String {
        self.storage_id.clone()
    }

    fn path(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        {
            None
        }
        #[cfg(not(test))]
        {
            Some(
                del_agent::data_dir()?
                    .join("side-chats")
                    .join(format!("{}.json", self.storage_id)),
            )
        }
    }

    pub fn restore(
        context: Context,
        storage_id: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let storage_id = uuid::Uuid::parse_str(&storage_id)
            .map(|id| id.to_string())
            .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
        let input = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .placeholder("Ask anything...")
                .auto_grow(3, 8)
                .submit_on_enter(true);
            input.set_submit_on_enter(true, cx);
            input
        });
        cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.submit(window, cx);
            }
            this.save_soon(cx);
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            this.save(cx);
            async {}
        })
        .detach();
        let owner = cx.entity().downgrade();
        let model_picker =
            cx.new(|cx| crate::model_picker::ModelPicker::for_side_chat(owner, window, cx));
        let mut view = Self {
            context,
            storage_id,
            saved: SavedChat::default(),
            session: None,
            parent: None,
            chat: None,
            input,
            attachments: vec![],
            models: vec![],
            model_picker,
            models_loading: false,
            model_generation: 0,
            save_pending: false,
            storage_error: false,
            importing: false,
            loading: false,
            closed: false,
            error: None,
        };
        if let Some(path) = view.path() {
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<SavedChat>(&bytes) {
                    Ok(saved) => view.saved = saved,
                    Err(error) => {
                        view.storage_error = true;
                        view.error = Some(format!(
                            "Could not restore this side chat: {error}. Its saved file has been preserved."
                        ));
                        return view;
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    view.storage_error = true;
                    view.error = Some(format!("Could not read this side chat: {error}"));
                    return view;
                }
            }
        }
        view.input.update(cx, |input, cx| {
            input.set_value(view.saved.draft.text.clone(), window, cx)
        });
        view.attachments = view.saved.draft.attachments.clone();
        for item in &mut view.saved.queue {
            item.sending = false;
            item.error = Some("Restored after reopening. Review before sending.".into());
        }
        view.start(cx);
        view
    }

    fn save_soon(&mut self, cx: &mut gpui::Context<Self>) {
        if self.closed || self.save_pending {
            return;
        }
        self.save_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.save_pending = false;
                this.save(cx);
            });
        })
        .detach();
    }

    pub fn save(&mut self, cx: &mut gpui::Context<Self>) {
        if self.closed || self.storage_error {
            return;
        }
        // Retain the last good snapshot if the provider has just shut down.
        if let Some(id) = &self.session {
            match self.context.backend.manager.export_fork(id) {
                Ok(snapshot) => self.saved.snapshot = Some(snapshot),
                Err(error) if self.saved.snapshot.is_none() => {
                    self.error = Some(error.to_string());
                    return;
                }
                Err(_) => {}
            }
        }
        if self.saved.snapshot.is_none() {
            return;
        }
        self.saved.draft = UserInput::text(self.input.read(cx).value().to_string());
        self.saved.draft.attachments = self.attachments.clone();
        if let Some(chat) = &self.chat {
            self.saved.queue = chat.read(cx).queue.items.clone();
        }
        if let Some(path) = self.path() {
            let result = serde_json::to_vec(&self.saved)
                .map_err(std::io::Error::other)
                .and_then(|bytes| {
                    crate::sessions::write_atomic(&path, &bytes, crate::sessions::PRIVATE)
                });
            if let Err(error) = result {
                self.error = Some(format!("Could not save side chat: {error}"));
                cx.notify();
            }
        }
    }

    fn start(&mut self, cx: &mut gpui::Context<Self>) {
        if self.closed || self.storage_error || self.loading || self.session.is_some() {
            return;
        }
        self.loading = true;
        self.error = None;
        let context = self.context.clone();
        let snapshot = self.saved.snapshot.clone();
        let work = context.backend.runtime.spawn(async move {
            if let Some(snapshot) = snapshot {
                context.backend.manager.restore_fork(snapshot).await
            } else {
                context
                    .backend
                    .manager
                    .fork(context.source, ForkLifetime::Temporary)
                    .await
            }
        });
        self.finish_start(work, cx);
    }

    fn finish_start(
        &mut self,
        work: tokio::task::JoinHandle<del_agent::Result<del_agent::ForkResult>>,
        cx: &mut gpui::Context<Self>,
    ) {
        let cleanup = self.context.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = work
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let id = result.as_ref().ok().map(|r| r.handle.id.clone());
            let updated = this.update(cx, |this, cx| {
                if this.closed {
                    return false;
                }
                this.loading = false;
                match result {
                    Ok(fork) => {
                        let old = this.session.replace(fork.handle.id.clone());
                        this.parent = fork.parent;
                        let chat = cx.new(|cx| {
                            ChatView::new(
                                this.context.backend.clone(),
                                Some(fork.handle.id),
                                None,
                                cx,
                            )
                        });
                        chat.update(cx, |chat, _| chat.queue.items = this.saved.queue.clone());
                        cx.observe(&chat, |this, chat, cx| {
                            let errors =
                                chat.update(cx, |chat, _| std::mem::take(&mut chat.pending_errors));
                            if let Some(error) = errors.last() {
                                this.error = Some(error.clone());
                            }
                            this.save_soon(cx);
                            cx.notify();
                        })
                        .detach();
                        cx.subscribe(&chat, |_, _, event: &crate::chat::OpenLink, cx| {
                            cx.emit(crate::chat::OpenLink(event.0.clone()))
                        })
                        .detach();
                        this.chat = Some(chat);
                        this.save(cx);
                        this.load_models(cx);
                        if let Some(old) = old {
                            let backend = this.context.backend.clone();
                            backend.runtime.spawn(async move {
                                let _ = backend.manager.delete(&old).await;
                            });
                        }
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.load_models(cx);
                    }
                }
                cx.notify();
                true
            });
            if !updated.unwrap_or(false)
                && let Some(id) = id
            {
                cleanup.runtime.spawn(async move {
                    let _ = cleanup.manager.delete(&id).await;
                });
            }
        })
        .detach();
    }

    fn record(&self) -> Option<del_agent::SessionRecord> {
        self.session
            .as_ref()
            .and_then(|id| self.context.backend.manager.session_record(id).ok())
    }
    fn configuration(&self) -> Option<(SessionConfig, Option<String>)> {
        if let Some(record) = self.record() {
            return Some((record.config(), record.account_id));
        }
        if let Some(snapshot) = &self.saved.snapshot {
            return Some((snapshot.config.clone(), snapshot.account_id.clone()));
        }
        match &self.context.source {
            ForkSource::Session(id) => self
                .context
                .backend
                .manager
                .session_record(id)
                .ok()
                .map(|record| (record.config(), record.account_id)),
            ForkSource::New { config, account_id } => Some((config.clone(), account_id.clone())),
        }
    }
    fn load_models(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(record) = self.record() else {
            return;
        };
        self.models_loading = true;
        self.model_generation += 1;
        let generation = self.model_generation;
        let backend = self.context.backend.clone();
        let work = backend.runtime.spawn(async move {
            backend
                .manager
                .account_models(record.provider, record.account_id.as_deref(), &record.cwd)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = work
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                if this.closed || this.model_generation != generation {
                    return;
                }
                this.models_loading = false;
                match result {
                    Ok(models) => this.models = models,
                    Err(error) => this.error = Some(format!("Could not load models: {error}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn change_settings(
        &mut self,
        config: SessionConfig,
        account: Option<String>,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.storage_error
            || self.loading
            || self.chat.as_ref().is_some_and(|chat| {
                chat.read(cx).is_loading()
                    || chat.read(cx).is_running()
                    || !chat.read(cx).queue.items.is_empty()
            })
        {
            return;
        }
        let id = self.session.clone();
        self.save(cx);
        self.loading = true;
        self.error = None;
        self.models.clear();
        self.model_generation += 1;
        let backend = self.context.backend.clone();
        let source = self.context.source.clone();
        let snapshot = self.saved.snapshot.clone();
        let work = backend.runtime.spawn(async move {
            if let Some(id) = id {
                backend.manager.reconfigure_fork(&id, config, account).await
            } else if let Some(mut snapshot) = snapshot {
                snapshot.config = config;
                snapshot.account_id = account;
                backend.manager.restore_fork(snapshot).await
            } else {
                backend
                    .manager
                    .fork_with_settings(source, config, account)
                    .await
            }
        });
        self.finish_start(work, cx);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if (text.trim().is_empty() && self.attachments.is_empty()) || self.loading || self.importing
        {
            return;
        }
        let Some(chat) = &self.chat else {
            return;
        };
        if chat.read(cx).is_loading() {
            return;
        }
        self.error = None;
        let mut input = UserInput::text(text);
        input.attachments = std::mem::take(&mut self.attachments);
        chat.update(cx, |chat, cx| {
            chat.submit_input(input, false, cx);
        });
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.save(cx);
        cx.notify();
    }

    fn attach(
        &mut self,
        sources: Vec<crate::attachments::ImportSource>,
        cx: &mut gpui::Context<Self>,
    ) {
        if sources.is_empty() || self.importing {
            return;
        }
        self.importing = true;
        let work = self.context.backend.runtime.spawn_blocking(move || {
            let directory = del_agent::data_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("attachments");
            sources
                .into_iter()
                .map(|source| source.import(&directory))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.importing = false;
                match result {
                    Ok(results) => {
                        for result in results {
                            match result {
                                Ok(file) => this.attachments.push(file),
                                Err(error) => this.error = Some(error),
                            }
                        }
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                this.save(cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn pick_attachments(&mut self, cx: &mut gpui::Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                let _ = this.update(cx, |this, cx| {
                    this.attach(
                        paths
                            .into_iter()
                            .map(crate::attachments::ImportSource::File)
                            .collect(),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    #[cfg(test)]
    fn latest_reply(&self, cx: &App) -> Option<String> {
        self.chat
            .as_ref()?
            .read(cx)
            .search_messages()
            .iter()
            .rev()
            .find(|m| m.role == "Assistant" && !m.text.trim().is_empty())
            .map(|m| m.text.clone())
    }

    pub fn forget(&mut self, cx: &mut gpui::Context<Self>) {
        let path = self.path();
        self.release(cx);
        if let Some(path) = path {
            let _ = std::fs::remove_file(path);
        }
    }
    pub fn release(&mut self, cx: &mut gpui::Context<Self>) {
        self.save(cx);
        self.closed = true;
        self.chat = None;
        self.close_backend();
    }
    fn close_backend(&mut self) {
        if let Some(id) = self.session.take() {
            let backend = self.context.backend.clone();
            backend.runtime.spawn(async move {
                let _ = backend.manager.delete(&id).await;
            });
        }
    }
}
impl Drop for SideChat {
    fn drop(&mut self) {
        self.close_backend();
    }
}

#[derive(Clone)]
struct Settings {
    config: SessionConfig,
    account_id: Option<String>,
}
impl Settings {
    fn config(&self) -> SessionConfig {
        self.config.clone()
    }
}
impl std::ops::Deref for Settings {
    type Target = SessionConfig;
    fn deref(&self) -> &SessionConfig {
        &self.config
    }
}

impl SideChat {
    fn settings(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let Some((config, account_id)) = self.configuration() else {
            return div().into_any_element();
        };
        let record = Settings { config, account_id };
        let locked = self.settings_locked(cx);
        let view = cx.entity();
        let accounts = self.context.accounts.clone();
        let current = record.clone();
        let account = crate::composer_ui::provider_button(
            "side-account",
            record.provider,
            !self.loading,
            locked,
        )
        .dropdown_menu(move |menu, window, cx| {
            crate::providers::VISIBLE
                .iter()
                .fold(menu, |menu, &provider| {
                    let view = view.clone();
                    let accounts = accounts.clone();
                    let current = current.clone();
                    let default_view = view.clone();
                    let default_current = current.clone();
                    let default_account = accounts
                        .iter()
                        .find(|a| a.provider == provider && a.is_default && !a.requires_login)
                        .map(|a| a.id.clone());
                    let (logo, color) = crate::composer_ui::provider_brand(provider);
                    menu.submenu_with_icon(
                        Some(crate::icons::path(logo).text_color(crate::appearance::rgb(color))),
                        crate::providers::display_name(provider),
                        window,
                        cx,
                        move |menu, _, _| {
                            let choices = std::iter::once((
                                None,
                                if provider == Provider::OpenCode {
                                    "System"
                                } else {
                                    "Account 1"
                                }
                                .to_owned(),
                            ))
                            .chain(
                                accounts
                                    .iter()
                                    .filter(|a| a.provider == provider && !a.requires_login)
                                    .map(|a| (Some(a.id.clone()), a.display())),
                            );
                            choices.fold(menu, |menu, (id, label)| {
                                let view = view.clone();
                                let mut config = current.config();
                                if config.provider != provider {
                                    config.model = None;
                                    config.reasoning_effort = None;
                                    config.service_tier = None;
                                }
                                config.provider = provider;
                                menu.item(
                                    PopupMenuItem::element({
                                        let selected = current.provider == provider
                                            && current.account_id == id;
                                        move |_, _| {
                                            crate::composer_ui::account_menu_label(
                                                label.clone(),
                                                None,
                                                selected,
                                            )
                                        }
                                    })
                                    .on_click(
                                        move |_, _, cx| {
                                            view.update(cx, |this, cx| {
                                                this.change_settings(config.clone(), id.clone(), cx)
                                            })
                                        },
                                    ),
                                )
                            })
                        },
                    )
                    .submenu_on_click(move |_, _, cx| {
                        let mut config = default_current.config();
                        if config.provider != provider {
                            config.model = None;
                            config.reasoning_effort = None;
                            config.service_tier = None;
                        }
                        config.provider = provider;
                        default_view.update(cx, |this, cx| {
                            this.change_settings(config, default_account.clone(), cx)
                        });
                    })
                })
        });
        h_flex()
            .items_center()
            .flex_wrap()
            .min_w_0()
            .gap_0p5()
            .child(account)
            .child(self.model_picker.clone())
            .into_any_element()
    }

    fn settings_locked(&self, cx: &App) -> bool {
        self.storage_error
            || self.loading
            || self.chat.as_ref().is_some_and(|chat| {
                let chat = chat.read(cx);
                chat.is_loading() || chat.is_running() || !chat.queue.items.is_empty()
            })
    }

    pub(crate) fn model_selection(&self, cx: &App) -> crate::model_picker::ModelSelection {
        let config = self.configuration().map(|(config, _)| config);
        crate::model_picker::ModelSelection {
            above: true,
            provider: config.as_ref().map_or(Provider::Codex, |c| c.provider),
            model: config.as_ref().and_then(|c| c.model.clone()),
            effort: config.as_ref().and_then(|c| c.reasoning_effort.clone()),
            tier: config.as_ref().and_then(|c| c.service_tier.clone()),
            locked: config.is_none() || self.settings_locked(cx),
            catalog: crate::state::ModelCatalog {
                models: self.models.clone(),
                loading: self.models_loading,
                loaded: !self.models.is_empty(),
                error: None,
            },
        }
    }

    pub(crate) fn change_model(
        &mut self,
        change: crate::model_picker::ModelChange,
        cx: &mut gpui::Context<Self>,
    ) {
        use crate::model_picker::ModelChange;
        if let ModelChange::Refresh(refresh) = change {
            if !self.models_loading && (refresh || self.models.is_empty()) {
                self.load_models(cx);
                cx.notify();
            }
            return;
        }
        if self.settings_locked(cx) {
            return;
        }
        let Some((mut config, account)) = self.configuration() else {
            return;
        };
        let info = self
            .models
            .iter()
            .find(|m| config.model.as_ref().map_or(m.is_default, |id| &m.id == id));
        match change {
            ModelChange::Model(model) => {
                config.model = model;
                config.reasoning_effort = None;
                config.service_tier = None;
            }
            ModelChange::Effort(effort) => {
                if let Some(value) = &effort {
                    let Some(info) = info.filter(|info| info.supports_effort(value)) else {
                        return;
                    };
                    if config.model.is_none() {
                        config.model = Some(info.id.clone());
                    }
                }
                config.reasoning_effort = effort;
            }
            ModelChange::Fast(enabled) => {
                config.service_tier = if enabled {
                    let Some(tier) = info.and_then(|m| m.fast_tier.clone()) else {
                        return;
                    };
                    Some(tier)
                } else {
                    None
                };
            }
            ModelChange::Reset => {
                config.reasoning_effort = None;
                config.service_tier = None;
            }
            ModelChange::Refresh(_) => unreachable!(),
        }
        self.change_settings(config, account, cx);
    }
}

impl Render for SideChat {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let running = self.chat.as_ref().is_some_and(|c| c.read(cx).is_running());
        let ready = self.chat.as_ref().is_some_and(|c| !c.read(cx).is_loading())
            && !self.loading
            && !self.importing;
        let empty = self
            .chat
            .as_ref()
            .is_none_or(|c| c.read(cx).search_messages().is_empty());
        let queued = self
            .chat
            .as_ref()
            .map_or(0, |c| c.read(cx).queue.items.len());
        let failed: Vec<_> = self
            .chat
            .as_ref()
            .map(|chat| {
                chat.read(cx)
                    .queue
                    .items
                    .iter()
                    .filter(|item| item.error.is_some() && !item.sending)
                    .map(|item| (item.id.clone(), item.input.text.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let send = cx.entity();
        let stop = send.clone();
        let attach = send.clone();
        let paste = send.clone();
        let settings = self.settings(cx);
        let attachments =
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(self.attachments.iter().enumerate().map(|(index, file)| {
                    Button::new(("side-attachment", index))
                        .ghost()
                        .small()
                        .label(format!(
                            "{} ×",
                            file.path.file_name().unwrap_or_default().to_string_lossy()
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if index < this.attachments.len() {
                                this.attachments.remove(index);
                                this.save(cx);
                                cx.notify();
                            }
                        }))
                }));
        let composer = crate::composer_ui::ComposerCard::new(
            &self.input,
            640.,
            self.record().map_or(Provider::Codex, |r| r.provider),
            ready,
            None,
            String::new(),
            true,
            running,
        )
        .settings(settings)
        .show_folder(false)
        .starting(!ready)
        .attachments(!self.attachments.is_empty())
        .attachment_content((!self.attachments.is_empty()).then(|| attachments.into_any_element()))
        .on_send(move |window, cx| send.update(cx, |this, cx| this.submit(window, cx)))
        .on_stop(move |_, cx| {
            stop.update(cx, |this, cx| {
                if let Some(chat) = &this.chat {
                    chat.update(cx, |chat, cx| chat.stop(cx));
                }
            })
        })
        .on_attach(move |_, cx| attach.update(cx, |this, cx| this.pick_attachments(cx)))
        .on_paste(move |item, _, cx| {
            let sources = crate::attachments::clipboard_sources(item);
            if sources.is_empty() {
                return false;
            }
            paste.update(cx, |this, cx| this.attach(sources, cx));
            true
        });
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .whitespace_normal()
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .px_3()
                        .py_2()
                        .gap_1()
                        .text_size(zpx(12.))
                        .child(error)
                        .when(
                            self.session.is_none() && !self.loading && !self.storage_error,
                            |v| {
                                v.child(
                                    Button::new("retry-side-start")
                                        .ghost()
                                        .small()
                                        .label("Retry")
                                        .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                                )
                            },
                        ),
                )
            })
            .child(if empty {
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .justify_center()
                    .items_center()
                    .p_4()
                    .gap_2()
                    .text_size(zpx(13.))
                    .child(
                        div().mb_2().child(
                            crate::icons::path("icons/side-chat.svg")
                                .with_size(zpx(28.))
                                .text_color(rgba(0xffffff80)),
                        ),
                    )
                    .child("Side chat")
                    .child(
                        div()
                            .w_full()
                            .max_w(zpx(320.))
                            .min_w_0()
                            .whitespace_normal()
                            .text_color(rgba(0xffffff80))
                            .text_center()
                            .child("Explore a side question with the main chat’s context."),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .when_some(self.chat.clone(), |v, chat| v.child(chat))
                    .into_any_element()
            })
            .children(failed.into_iter().map(|(id, text)| {
                Button::new(SharedString::from(format!("retry-{id}")))
                    .ghost()
                    .small()
                    .label(format!(
                        "Edit unsent: {}",
                        text.chars().take(32).collect::<String>()
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(chat) = &this.chat
                            && let Some(input) =
                                chat.update(cx, |chat, cx| chat.take_queued(&id, cx))
                        {
                            this.input.update(cx, |state, cx| {
                                state.set_value(input.text, window, cx);
                                state.focus(window, cx);
                            });
                            this.attachments = input.attachments;
                            this.error = None;
                            this.save(cx);
                        }
                    }))
            }))
            .when(queued > 0, |v| {
                v.child(
                    div()
                        .px_3()
                        .text_size(zpx(11.))
                        .text_color(rgba(0xffffff80))
                        .child(format!("{queued} queued")),
                )
            })
            .child(
                h_flex()
                    .flex_none()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .justify_center()
                    .child(composer),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;
    use std::{sync::Arc, time::Duration};

    #[::gpui::test]
    fn side_chat_uses_its_own_transcript_and_queue(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::init);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let agent: Arc<dyn del_agent::AgentBackend> =
            Arc::new(del_agent::MockBackend::new(del_agent::Provider::Codex));
        let manager = del_agent::SessionManager::new(dir.path().join("index"), [agent]).unwrap();
        let config = del_agent::SessionConfig {
            cwd: dir.path().into(),
            provider: del_agent::Provider::Codex,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: del_agent::PermissionMode::Plan,
        };
        let parent = runtime.block_on(manager.start_session(config)).unwrap();
        let context = Context {
            backend: Backend {
                runtime: runtime.handle().clone(),
                manager: manager.clone(),
            },
            source: ForkSource::Session(parent.id.clone()),
            accounts: vec![],
        };
        let (side, cx) = cx.add_window_view(|window, cx| SideChat::new(context, window, cx));
        for _ in 0..200 {
            cx.run_until_parked();
            if side.read_with(cx, |s, cx| {
                s.chat.as_ref().is_some_and(|c| !c.read(cx).is_loading())
            }) {
                break;
            }
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
        }
        side.read_with(cx, |s, cx| {
            assert!(s.error.is_none(), "{:?}", s.error);
            assert!(
                s.chat
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .search_messages()
                    .is_empty()
            );
            assert_eq!(s.parent, Some(parent.id.clone()));
        });
        cx.update(|window, cx| {
            side.update(cx, |side, cx| {
                side.input.update(cx, |input, cx| {
                    input.set_value("a side question", window, cx)
                });
                side.submit(window, cx);
            })
        });
        for _ in 0..200 {
            cx.run_until_parked();
            if side.read_with(cx, |s, cx| {
                s.latest_reply(cx).is_some() && !s.chat.as_ref().unwrap().read(cx).is_running()
            }) {
                break;
            }
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
        }
        side.read_with(cx, |s, cx| {
            assert!(s.latest_reply(cx).is_some());
            assert!(
                s.chat
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .search_messages()
                    .iter()
                    .any(|m| m.text == "a side question")
            );
            assert!(manager.is_temporary(s.session.as_ref().unwrap()));
        });
        assert_eq!(
            manager.subscribe_with_history(&parent.id).unwrap().0.len(),
            1
        );
        assert_eq!(manager.list_sessions().len(), 1);
        // The shared picker changes this fork and preserves its transcript, never its parent.
        cx.update(|_, cx| {
            side.update(cx, |side, cx| {
                side.change_model(
                    crate::model_picker::ModelChange::Model(Some("side-only-model".into())),
                    cx,
                );
            })
        });
        for _ in 0..200 {
            cx.run_until_parked();
            if side.read_with(cx, |side, cx| !side.settings_locked(cx)) {
                break;
            }
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
        }
        side.read_with(cx, |side, cx| {
            assert!(side.error.is_none(), "{:?}", side.error);
            assert_eq!(
                side.model_selection(cx).model.as_deref(),
                Some("side-only-model")
            );
            assert_eq!(
                side.saved
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .config
                    .model
                    .as_deref(),
                Some("side-only-model")
            );
            assert!(side.latest_reply(cx).is_some());
        });
        assert!(
            manager
                .list_sessions()
                .iter()
                .find(|s| s.id == parent.id)
                .unwrap()
                .model
                .is_none()
        );
        let id = side.read_with(cx, |s, _| s.session.clone().unwrap());
        // Exercise the same release path used by either tab surface.
        cx.update(|_, cx| side.update(cx, |side, cx| side.release(cx)));
        for _ in 0..200 {
            if !manager.is_live(&id) {
                break;
            }
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
        }
        assert!(!manager.is_live(&id));
        assert!(manager.is_live(&parent.id));
        runtime.block_on(manager.shutdown());
    }
}
