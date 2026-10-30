//! Main-app session modes and the GPUI end of the MCP command channel.
use crate::appearance::{rgb, rgba};
use crate::mcp_bridge::{BridgeEvent, McpBridge};
use crate::sessions::TerminalRow;
use crate::state::Selection;
use crate::terminal_host::{
    TerminalHost, TerminalHostEvent, TerminalHostSnapshot, TerminalHostStatus, TerminalLaunchKind,
    TerminalLaunchSpec,
};
use crate::terminal_store::{TerminalRecord, TerminalStore};
use crate::zoom::px as zpx;
use crate::{AppShell, composer_ui};
use del_agent::{Provider, SessionId};
use del_mcp::{HostError, HostResult, SessionInfo, SessionMode, UserAnswer};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

/// Agent questions waiting in the UI, overall and per asking session.
const MAX_QUESTIONS: usize = 16;
const MAX_QUESTIONS_PER_SESSION: usize = 3;

pub(super) struct PendingQuestion {
    id: u64,
    caller: SessionId,
    question: String,
    options: Vec<String>,
    input: Entity<InputState>,
    reply: oneshot::Sender<UserAnswer>,
}

pub(super) fn tab_mark(provider: Option<Provider>) -> AnyElement {
    match provider {
        Some(provider) => composer_ui::provider_logo(provider).into_any_element(),
        None => crate::icons::path("icons/terminal.svg").into_any_element(),
    }
}

pub(super) fn terminal_mark(provider: Option<Provider>) -> AnyElement {
    div()
        .relative()
        .size(zpx(18.))
        .flex_none()
        .child(
            crate::icons::path("icons/terminal.svg")
                .size(zpx(16.))
                .text_color(rgba(0xffffffcc)),
        )
        .when_some(provider, |icon, provider| {
            icon.child(
                div()
                    .absolute()
                    .right(zpx(-2.))
                    .bottom(zpx(-2.))
                    .size(zpx(11.))
                    .rounded(zpx(3.))
                    .bg(crate::appearance::popup_surface())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(composer_ui::provider_logo(provider).size(zpx(9.))),
            )
        })
        .into_any_element()
}

impl AppShell {
    pub(super) fn start_modes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for profile in crate::browser::pending_deletions() {
            self.delete_browser_profile(profile, cx);
        }
        let backend = self.state.read(cx).backend();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let config_dir = del_agent::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("mcp")
            .join(format!("app-{}", std::process::id()));
        let start = backend
            .runtime
            .spawn(McpBridge::start(backend.manager, tx, config_dir));
        cx.spawn_in(window, async move |this, cx| {
            match start.await {
                Ok(Ok(bridge)) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        match bridge.handle().chat_browser_tools() {
                            Ok(tools) => this
                                .state
                                .read(cx)
                                .backend()
                                .manager
                                .set_browser_tools(tools),
                            Err(error) => this.state.update(cx, |state, _| {
                                state.error = Some(format!("Browser tools unavailable: {error}"))
                            }),
                        }
                        this.state.update(cx, |state, cx| {
                            state.browser_tools_ready = true;
                            cx.notify();
                        });
                        this.mcp = Some(bridge);
                        cx.notify();
                    });
                }
                result => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.state.update(cx, |s, cx| {
                            s.error = Some(format!(
                                "Agent tools could not start: {}",
                                match result {
                                    Ok(Err(e)) => e.to_string(),
                                    Err(e) => e.to_string(),
                                    _ => unreachable!(),
                                }
                            ));
                            cx.notify();
                        });
                    });
                    return;
                }
            }
            while let Some(event) = rx.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.handle_bridge(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            if let Some(bridge) = &this.mcp {
                bridge.shutdown();
            }
            for host in this.terminals.values() {
                host.update(cx, |host, cx| host.close(cx));
            }
            del_terminal::shutdown_all(Duration::from_millis(200));
            async {}
        })
        .detach();
    }

    pub(super) fn ensure_modes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self
            .state
            .update(cx, |s, _| std::mem::take(&mut s.closed_terminals));
        for id in closed {
            self.terminal_update_restarts
                .retain(|pending| pending != &id);
            self.terminal_opening.remove(&id);
            self.terminal_attempts.remove(&id);
            if let Some(host) = self.terminals.remove(&id) {
                host.update(cx, |host, cx| host.close(cx));
            }
            if let Some(bridge) = &self.mcp {
                bridge.handle().revoke(&id);
                bridge.handle().remove_terminal(&id);
            }
            self.questions.retain(|q| q.caller != id);
        }
        for id in std::mem::take(&mut self.terminal_update_restarts) {
            if let Some(record) = self
                .terminal_store
                .records
                .iter()
                .find(|r| r.id == id)
                .cloned()
            {
                self.begin_terminal(record.launch, None, Some(id), None, None, window, cx);
            }
        }
        let selection = self.state.read(cx).selection.clone();
        if self.mcp.is_some()
            && let Selection::Terminal(id) = selection
            && !self.terminals.contains_key(&id)
            && !self.terminal_attempts.contains(&id)
            && let Some(record) = self
                .terminal_store
                .records
                .iter()
                .find(|r| r.id == id)
                .cloned()
        {
            self.begin_terminal(record.launch, None, Some(id), None, None, window, cx);
        }
    }

    /// A terminal draft uses the same composer until its first prompt is sent.
    pub(super) fn submit_terminal_prompt(
        &mut self,
        prompt: &str,
        attachments: &[del_agent::Attachment],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((key, config)) = self.state.read(cx).terminal_draft() else {
            return false;
        };
        if self.state.read(cx).is_starting() {
            return true;
        }
        let mut spec = match TerminalLaunchSpec::from_config(&config) {
            Ok(spec) => spec,
            Err(error) => {
                self.set_mode_error(error.to_string(), cx);
                return true;
            }
        };
        spec.account_id = self.state.read(cx).selected_account();
        let prompt = crate::attachments::terminal_prompt(prompt, attachments);
        let mut options = self.state.read(cx).draft_launch_options();
        if options.token.is_empty() {
            self.state.update(cx, |s, cx| {
                s.set_launch_options(options.branch.clone(), options.worktree, cx)
            });
            options = self.state.read(cx).draft_launch_options();
        }
        self.state.update(cx, |state, cx| {
            state.begin_terminal_prompt(key, prompt.to_owned(), cx)
        });
        self.composer_attachments.remove(&self.composer_owner(cx));
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        let runtime = self.state.read(cx).backend().runtime;
        let prepare = runtime.spawn_blocking(move || {
            spec.cwd = crate::projects::prepare(
                &spec.cwd,
                &options,
                &del_agent::data_dir().unwrap_or_else(std::env::temp_dir),
            )?;
            Ok::<_, String>(spec)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = prepare.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(spec) => {
                    this.begin_terminal(spec, Some(key), None, Some(prompt), None, window, cx)
                }
                Err(error) => this
                    .state
                    .update(cx, |s, cx| s.terminal_start_failed(key, error, cx)),
            });
        })
        .detach();
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_terminal(
        &mut self,
        spec: TerminalLaunchSpec,
        draft: Option<u64>,
        existing: Option<SessionId>,
        prompt: Option<String>,
        reply: Option<oneshot::Sender<HostResult<SessionInfo>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if reply.as_ref().is_some_and(|r| r.is_closed()) {
            return;
        }
        let resuming = existing.is_some();
        let id = existing.unwrap_or_else(|| {
            SessionId::new(format!(
                "terminal-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        });
        if !self.terminal_opening.insert(id.clone()) {
            return;
        }
        self.terminal_attempts.insert(id.clone());
        let previous = self
            .terminal_store
            .records
            .iter()
            .find(|r| r.id == id)
            .cloned();
        let saved_records = self.terminal_store.records.clone();
        let conversation_id = previous
            .as_ref()
            .and_then(|r| r.conversation_id.clone())
            .or_else(|| {
                (!resuming && spec.kind == TerminalLaunchKind::Claude)
                    .then(|| uuid::Uuid::new_v4().to_string())
            });
        let saved_title = previous
            .as_ref()
            .map(|r| r.title.clone())
            .unwrap_or_else(|| spec.label());
        if let Some(old) = self.terminals.remove(&id) {
            old.update(cx, |host, cx| host.close(cx));
            if let Some(bridge) = &self.mcp {
                bridge.handle().revoke(&id);
            }
        }
        let info = SessionInfo {
            id: id.clone(),
            provider: spec.provider(),
            mode: SessionMode::Terminal,
            command: Some(spec.kind.label().into()),
            cwd: spec.cwd.clone(),
            title: Some(saved_title.clone()),
            status: "running".into(),
            reported_state: None,
        };
        let bridge = self.mcp.as_ref().map(McpBridge::handle);
        let prepare_info = info.clone();
        let runtime = self.state.read(cx).backend().runtime;
        if let (Some(program), Some(provider)) = (spec.program.clone(), spec.provider()) {
            // Never replace the CLI under another running chat or terminal.
            let manager = self.state.read(cx).backend().manager;
            runtime.spawn(async move {
                if crate::provider_updates::provider_in_use(&manager, provider) {
                    return;
                }
                if let Err(error) =
                    crate::provider_updates::ensure_program(program.into(), provider, |_| {}).await
                {
                    eprintln!("[del update] terminal background check failed: {error}");
                }
            });
        }
        let profile_env = match spec.account() {
            Ok(account) => account.map(|a| a.environment()).unwrap_or_default(),
            Err(error) => {
                self.terminal_opening.remove(&id);
                if let Some(key) = draft {
                    self.state.update(cx, |s, cx| {
                        s.terminal_start_failed(key, error.to_string(), cx)
                    });
                } else {
                    self.set_mode_error(error.to_string(), cx);
                }
                return;
            }
        };
        let codex_root = spec.codex_root();
        let opencode_model = spec.model.clone();
        let opencode_variant = spec.variant.clone();
        let prepare = runtime.spawn(async move {
            let conversation_id = if prepare_info.provider == Some(Provider::OpenCode) && !resuming {
                Some(del_opencode::create_terminal_session_with_environment(&prepare_info.cwd, opencode_model.as_deref(), opencode_variant.as_deref(), &profile_env).await.map_err(|e| e.to_string())?)
            } else { conversation_id };
            tokio::task::spawn_blocking(move || {
            let conversation_id = if resuming && prepare_info.provider == Some(Provider::Codex) && conversation_id.is_none() {
                previous.as_ref().and_then(|record| crate::terminal_conversation::recover_legacy_codex(
                    &codex_root, record, &saved_records))
            } else { conversation_id };
            if resuming && prepare_info.provider.is_some() && conversation_id.is_none() {
                return Err("The original conversation could not be identified uniquely. Its saved history has been preserved.".into());
            }
            let (mut args, env) = if let Some(bridge) = bridge {
                bridge.upsert_terminal(prepare_info.clone(), None);
                if let Some(provider) = prepare_info.provider {
                    let config = bridge.launch_config(&prepare_info.id, provider).map_err(|e| e.to_string())?;
                    (config.args, config.env)
                } else { (Vec::new(), Vec::new()) }
            } else if prepare_info.provider.is_some() {
                return Err("Agent tools are still starting. Try again in a moment.".into());
            } else { (Vec::new(), Vec::new()) };
            if prepare_info.provider == Some(Provider::Codex) {
                // Del checks updates in the background, so CLI update prompts
                // must not block terminal startup.
                args.extend(["-c".into(), "check_for_update_on_startup=false".into()]);
            }
            if let Some(conversation) = &conversation_id {
                args.extend(resume_args(prepare_info.provider, conversation, resuming)?);
            }
            Ok((args, env, conversation_id))
            }).await.map_err(|e| e.to_string())?
        });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepare.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                let still_opening = this.terminal_opening.remove(&id);
                if !still_opening || reply.as_ref().is_some_and(|r| r.is_closed()) {
                    if let Some(bridge) = &this.mcp {
                        bridge.handle().revoke(&id);
                        bridge.handle().remove_terminal(&id);
                    }
                    return;
                }
                let result = prepared.and_then(|(mut args, env, conversation_id)| {
                    if let Some(prompt) = prompt {
                        args.push(
                            if spec.kind == TerminalLaunchKind::OpenCode {
                                "--prompt"
                            } else {
                                "--"
                            }
                            .into(),
                        );
                        args.push(prompt);
                    }
                    let mut launch = spec.clone();
                    launch.title = saved_title.clone();
                    let host = TerminalHost::spawn_prepared(launch, args, env, window, cx)
                        .map_err(|e| e.to_string())?;
                    let event_id = id.clone();
                    cx.subscribe(&host, move |this, _, event, cx| {
                        if matches!(
                            event,
                            TerminalHostEvent::Terminal(del_terminal::TerminalEvent::Input)
                        ) {
                            this.state.update(cx, |state, cx| {
                                state.keep_tab(&crate::state::Tab::Terminal(event_id.clone()), cx)
                            });
                        }
                        if let TerminalHostEvent::Changed(snapshot) = event {
                            this.terminal_changed(&event_id, snapshot, cx);
                        }
                    })
                    .detach();
                    if !resuming || this.state.read(cx).selection == Selection::Terminal(id.clone())
                    {
                        host.update(cx, |host, cx| host.focus(window, cx));
                    }
                    if let Some(provider) = spec.provider() {
                        crate::provider_updates::terminal_started(
                            provider,
                            host.read(cx).snapshot().pid,
                        );
                    }
                    this.terminals.insert(id.clone(), host);
                    this.terminal_store.upsert(TerminalRecord {
                        id: id.clone(),
                        title: saved_title.clone(),
                        launch: spec.clone(),
                        pinned: false,
                        conversation_id,
                    });
                    if let Err(e) = this.terminal_store.save(&TerminalStore::path()) {
                        this.set_mode_error(
                            format!("Terminal started, but history could not be saved: {e}"),
                            cx,
                        );
                    }
                    this.state.update(cx, |s, cx| {
                        let row = TerminalRow {
                            account_id: spec.account_id.clone(),
                            id: id.clone(),
                            title: saved_title.clone(),
                            cwd: spec.cwd.clone(),
                            provider: spec.provider(),
                            active: true,
                            summary: String::new(),
                            pinned: false,
                        };
                        if resuming {
                            s.terminal_reopened(row, cx);
                        } else {
                            s.terminal_started(draft, row, cx);
                        }
                    });
                    if let Some(bridge) = &this.mcp {
                        bridge.handle().upsert_terminal(info.clone(), None);
                    }
                    Ok(info)
                });
                if let Err(error) = &result {
                    if let Some(bridge) = &this.mcp {
                        bridge.handle().revoke(&id);
                        bridge.handle().remove_terminal(&id);
                    }
                    if let Some(key) = draft {
                        this.state.update(cx, |state, cx| {
                            state.terminal_start_failed(key, error.clone(), cx)
                        });
                    }
                    this.set_mode_error(error.clone(), cx);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(result.map_err(HostError::Failed));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh titles by exact provider identity. Saved terminals that are not
    /// open are refreshed once at startup; afterwards only running terminals are
    /// polled, backing off from 5 s to 30 s while nothing changes. Keep all I/O
    /// off the UI thread and retain stable names on errors.
    pub(super) fn watch_terminal_titles(&self, cx: &mut Context<Self>) {
        const INITIAL: Duration = Duration::from_secs(5);
        const MAX_BACKOFF: Duration = Duration::from_secs(30);
        cx.spawn(async move |this, cx| {
            let mut refresh_all = true;
            let mut interval = INITIAL;
            let mut previously_running: Vec<SessionId> = Vec::new();
            loop {
                let Ok((records, running, backend)) = this.update(cx, |this, cx| {
                    let mut running: Vec<SessionId> = this
                        .terminals
                        .iter()
                        .filter(|(_, host)| {
                            *host.read(cx).status()
                                == crate::terminal_host::TerminalHostStatus::Running
                        })
                        .map(|(id, _)| id.clone())
                        .collect();
                    running.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                    let records = this
                        .terminal_store
                        .records
                        .iter()
                        .filter(|record| refresh_all || running.contains(&record.id))
                        .cloned()
                        .collect::<Vec<_>>();
                    (records, running, this.state.read(cx).backend())
                }) else {
                    break;
                };
                refresh_all = false;
                // A terminal (re)opening is polled promptly again.
                if running != previously_running {
                    interval = INITIAL;
                    previously_running = running;
                }
                let lookup = backend.runtime.spawn(async move {
                    let mut titles = Vec::new();
                    for record in records {
                        let title = if let (Some(provider), Some(identity)) =
                            (record.launch.provider(), record.conversation_id.as_ref())
                        {
                            tokio::time::timeout(
                                Duration::from_secs(3),
                                backend.manager.account_session_display_title(
                                    provider,
                                    record.launch.account_id.as_deref(),
                                    &SessionId::new(identity.clone()),
                                    &record.launch.cwd,
                                ),
                            )
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .flatten()
                        } else {
                            None
                        };
                        titles.push((record.id, record.conversation_id, title));
                    }
                    titles
                });
                let titles = lookup.await.unwrap_or_default();
                let outcome = this.update(cx, |this, cx| {
                    let mut changed = false;
                    for (id, identity, title) in titles {
                        if !this
                            .terminal_store
                            .records
                            .iter()
                            .any(|r| r.id == id && r.conversation_id == identity)
                        {
                            continue;
                        }
                        if let Some(host) = this.terminals.get(&id) {
                            host.update(cx, |host, cx| host.refresh_title(title.clone(), cx));
                        }
                        if let Some(title) = title
                            && this.terminal_store.set_title(&id, &title)
                        {
                            changed = true;
                            this.state.update(cx, |s, cx| {
                                if let Some(row) = s.terminal_rows.iter_mut().find(|r| r.id == id) {
                                    row.title = title;
                                }
                                cx.notify();
                            });
                        }
                    }
                    if changed && let Err(error) = this.terminal_store.save(&TerminalStore::path())
                    {
                        this.set_mode_error(
                            format!("Could not save conversation title: {error}"),
                            cx,
                        );
                    }
                    changed
                });
                let Ok(changed) = outcome else {
                    break;
                };
                interval = if changed {
                    INITIAL
                } else {
                    (interval * 2).min(MAX_BACKOFF)
                };
                cx.background_executor().timer(interval).await;
            }
        })
        .detach();
    }

    fn terminal_changed(
        &mut self,
        id: &SessionId,
        snapshot: &TerminalHostSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self
            .terminals
            .get(id)
            .is_none_or(|host| host.read(cx).snapshot().pid != snapshot.pid)
        {
            return;
        }
        if let TerminalHostStatus::Exited { code } = snapshot.status
            && let Some(provider) = self
                .terminal_store
                .records
                .iter()
                .find(|r| &r.id == id)
                .and_then(|r| r.launch.provider())
            && crate::provider_updates::completed_cli_update(provider, &snapshot.last_output, code)
            && self
                .terminal_update_restarted
                .insert((id.clone(), snapshot.pid))
        {
            self.terminal_update_restarts.insert(id.clone());
            cx.notify();
        }
        if let Some(hint) = snapshot.conversation_hint.clone()
            && self
                .terminal_store
                .records
                .iter()
                .find(|r| &r.id == id)
                .is_some_and(|r| {
                    !r.conversation_id
                        .as_ref()
                        .is_some_and(|saved| saved.starts_with(&hint))
                })
            && self.terminal_identity_pending.insert(id.clone())
        {
            let id = id.clone();
            let pid = snapshot.pid;
            let runtime = self.state.read(cx).backend().runtime;
            let codex_root = self
                .terminal_store
                .records
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.launch.codex_root())
                .unwrap_or_else(crate::terminal_conversation::codex_root);
            let lookup_hint = hint.clone();
            let lookup = runtime.spawn(async move {
                for _ in 0..20 {
                    let hint = lookup_hint.clone();
                    let codex_root = codex_root.clone();
                    let found = tokio::task::spawn_blocking(move || {
                        crate::terminal_conversation::resolve_codex_prefix(&codex_root, &hint)
                    })
                    .await
                    .ok()
                    .flatten();
                    if found.is_some() {
                        return found;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                None
            });
            cx.spawn(async move |this, cx| {
                let found = lookup.await.ok().flatten();
                let _ = this.update(cx, |this, cx| {
                    this.terminal_identity_pending.remove(&id);
                    if this.terminals.get(&id).is_none_or(|host| {
                        let current = host.read(cx).snapshot();
                        current.pid != pid || current.conversation_hint.as_ref() != Some(&hint)
                    }) {
                        return;
                    }
                    if let Some(found) = found
                        && let Some(record) =
                            this.terminal_store.records.iter_mut().find(|r| r.id == id)
                    {
                        record.conversation_id = Some(found);
                        if let Err(error) = this.terminal_store.save(&TerminalStore::path()) {
                            this.set_mode_error(
                                format!("Could not save conversation identity: {error}"),
                                cx,
                            );
                        }
                    }
                });
            })
            .detach();
        }
        let active = snapshot.status == TerminalHostStatus::Running;
        self.state.update(cx, |s, cx| {
            // Only repaint (and re-sort the sidebar) when a visible field moved.
            let mut changed = if snapshot.title_loading {
                s.terminal_title_loading.insert(id.clone())
            } else {
                s.terminal_title_loading.remove(id)
            };
            if let Some(row) = s.terminal_rows.iter_mut().find(|r| &r.id == id) {
                let summary = snapshot.summary.as_deref().unwrap_or_default();
                if row.title != snapshot.title || row.active != active || row.summary != summary {
                    row.title = snapshot.title.clone();
                    row.active = active;
                    row.summary = summary.to_owned();
                    changed = true;
                }
            }
            if changed {
                cx.notify();
            }
        });
        if self.terminal_store.set_title(id, &snapshot.title)
            && let Err(error) = self.terminal_store.save(&TerminalStore::path())
        {
            self.set_mode_error(format!("Could not save terminal title: {error}"), cx);
        }
        if let Some(bridge) = &self.mcp {
            let provider = match snapshot.kind {
                TerminalLaunchKind::Codex => Some(Provider::Codex),
                TerminalLaunchKind::Claude => Some(Provider::Claude),
                TerminalLaunchKind::OpenCode => Some(Provider::OpenCode),
                _ => None,
            };
            bridge.handle().upsert_terminal(
                SessionInfo {
                    id: id.clone(),
                    provider,
                    mode: SessionMode::Terminal,
                    command: Some(snapshot.kind.label().into()),
                    cwd: snapshot.cwd.clone(),
                    title: Some(snapshot.title.clone()),
                    status: if active { "running" } else { "stopped" }.into(),
                    reported_state: None,
                },
                Some(snapshot.last_output.clone()),
            );
            if !active {
                bridge.handle().revoke(id);
            }
        }
        // Visible changes notify through AppState, which AppShell observes.
    }

    /// Keep the browser's session and panel alive without selecting either.
    fn browser_panel(
        &mut self,
        tab: &crate::state::Tab,
        key: String,
        cwd: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<crate::right_panel::RightPanel> {
        self.state
            .update(cx, |state, cx| state.open_background_tab(tab, cx));
        let owner = self.state.read(cx).uid_of(tab);
        if owner == self.right_owner {
            return self.right.clone();
        }
        if let Some(cached) = self.right_cache.get(&owner) {
            return cached.panel.clone();
        }
        let saved = self.right_store.get(&key);
        let open = saved.restore_open();
        // Restore without showing: a hidden panel must not start shells.
        let mut quiet = saved;
        quiet.open = false;
        let panel = cx.new(|cx| {
            let mut panel = crate::right_panel::RightPanel::new(cwd, window, cx);
            panel.restore(quiet, window, cx);
            panel
        });
        self.right_cache.insert(
            owner,
            crate::CachedRight {
                panel: panel.clone(),
                open,
                key: Some(key.clone()),
            },
        );
        // Like watch_right, plus persisting while it stays in the background.
        cx.observe(&panel, move |this, panel, cx| {
            if this.right == panel {
                this.remember_right(cx);
            } else {
                this.remember_background_right(&panel, key.clone(), cx);
            }
        })
        .detach();
        panel
    }

    fn remember_background_right(
        &mut self,
        panel: &Entity<crate::right_panel::RightPanel>,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let open = self
            .right_cache
            .values()
            .find(|cached| &cached.panel == panel)
            .is_some_and(|cached| cached.open);
        if self.right_store.set(key, panel.read(cx).snapshot(open, cx)) {
            self.persist_right(cx);
        }
    }

    pub(super) fn set_mode_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.error = Some(error);
            cx.notify();
        });
    }

    fn handle_bridge(&mut self, event: BridgeEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            BridgeEvent::Browser {
                caller,
                allowed,
                request,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                self.remember_right(cx);
                // Only the sessions the server allowed this caller are listed
                // or addressable, whether by session_id or by tab_id.
                let manager = self.state.read(cx).backend().manager;
                let records = manager.list_sessions();
                let mut sessions = Vec::new();
                let mut places = Vec::new();
                for id in &allowed {
                    let (title, terminal, cwd) =
                        if let Some(record) = records.iter().find(|r| &r.id == id) {
                            (record.title.clone(), false, record.cwd.clone())
                        } else if let Some(row) = self
                            .state
                            .read(cx)
                            .terminal_rows
                            .iter()
                            .find(|r| &r.id == id)
                        {
                            (Some(row.title.clone()), true, row.cwd.clone())
                        } else {
                            continue;
                        };
                    let key = format!(
                        "{}:{}",
                        if terminal { "terminal" } else { "chat" },
                        id.as_str()
                    );
                    let tab = if terminal {
                        crate::state::Tab::Terminal(id.clone())
                    } else {
                        crate::state::Tab::Session(id.clone())
                    };
                    let owner = self.state.read(cx).uid_of(&tab);
                    let open = self.state.read(cx).tabs.contains(&tab);
                    let mut tabs = if open && owner == self.right_owner {
                        self.right.read(cx).agent_browser_tabs(cx)
                    } else if let Some(cached) = self.right_cache.get(&owner).filter(|_| open) {
                        cached.panel.read(cx).agent_browser_tabs(cx)
                    } else {
                        self.right_store.browser_tabs(&key)
                    };
                    if let Some(workspace) = self.workspaces.get(&owner).filter(|_| open) {
                        tabs.extend(workspace.read(cx).agent_browser_tabs(cx));
                    }
                    sessions.push(serde_json::json!({"session_id":id.as_str(),"title":title,"mode":if terminal { "terminal" } else { "chat" },"tabs":tabs}));
                    places.push((tab, key, cwd));
                }
                let selected = match &self.state.read(cx).selection {
                    Selection::Session(id) | Selection::Terminal(id) => Some(id.clone()),
                    Selection::Draft(_) => None,
                };
                if request.action == "list_tabs"
                    && request.session_id.is_none()
                    && request.tab_id.is_none()
                {
                    let active = selected.filter(|id| allowed.contains(id));
                    let _ = reply.send(Ok(
                        serde_json::json!({"sessions":sessions,"active_session_id":active.as_ref().map(SessionId::as_str)}),
                    ));
                    return;
                }
                let by_tab = request.tab_id.as_ref().and_then(|id| {
                    sessions.iter().position(|session| {
                        session["tabs"].as_array().is_some_and(|tabs| {
                            tabs.iter().any(|tab| tab["id"].as_str() == Some(id))
                        })
                    })
                });
                let position = |id: &str| {
                    sessions
                        .iter()
                        .position(|s| s["session_id"].as_str() == Some(id))
                };
                let target = match &request.session_id {
                    Some(id) => position(id),
                    None => by_tab
                        .or_else(|| position(caller.as_str()))
                        .or_else(|| (sessions.len() == 1).then_some(0))
                        .or_else(|| selected.as_ref().and_then(|id| position(id.as_str()))),
                };
                let Some(target) = target else {
                    let _ = reply.send(Err(HostError::Failed(
                        "Choose a session_id from browser list_tabs".into(),
                    )));
                    return;
                };
                let (tab, key, cwd) = places.swap_remove(target);
                let panel = self.browser_panel(&tab, key.clone(), cwd, window, cx);
                let host = panel.clone();
                // New pages belong to the session's outer sidebar strip, even
                // when the caller is inspecting a page moved into the main zone.
                let mut request = request;
                let panel = if request.action == "new_tab" {
                    request.tab_id = None;
                    panel
                } else {
                    request
                        .tab_id
                        .as_ref()
                        .and_then(|id| self.find_agent_browser(&tab, id, cx))
                        .unwrap_or(panel)
                };
                let foreground = host == self.right;
                let show_preview = !matches!(request.action.as_str(), "list_tabs" | "close_tab");
                let result = if request.action == "close_tab" && panel != host {
                    let id = panel.read(cx).active_browser_id();
                    let id = request.tab_id.as_deref().unwrap_or(&id);
                    let closed = host.update(cx, |host, cx| host.close_browser_id(id, window, cx));
                    if !closed {
                        let owner = self.state.read(cx).uid_of(&tab);
                        if let Some(workspace) = self.workspaces.get(&owner) {
                            workspace.update(cx, |workspace, cx| {
                                workspace.close_browser_id(id, window, cx)
                            });
                        }
                    }
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(Ok(serde_json::json!({"tabs":host.read(cx).agent_browser_tabs(cx),"active_tab":null})));
                    Ok(rx)
                } else {
                    panel.update(cx, |panel, cx| panel.browser_command(request, window, cx))
                };
                let browser_id = panel.read(cx).active_browser_id();
                if !foreground {
                    // Keep tab_id lookups and restarts in sync with tabs the
                    // agent opened or closed in the background.
                    self.remember_background_right(&host, key, cx);
                }
                match result {
                    Err(error) => {
                        let _ = reply.send(Err(HostError::Failed(error)));
                    }
                    Ok(result) => {
                        cx.spawn_in(window, async move |this, cx| {
                            let mut reply = reply;
                            // The MCP request may time out first; stop waiting
                            // then instead of holding the reply.
                            let result = tokio::select! {
                                result = result => result,
                                _ = reply.closed() => return,
                            };
                            let result = result
                                .map_err(|_| HostError::Failed("Browser closed".into()))
                                .and_then(|result| result.map_err(HostError::Failed));
                            if result.is_ok() && show_preview {
                                let _ = this.update_in(cx, |this, _, cx| {
                                    if this.state.read(cx).tabs.contains(&tab) {
                                        this.browser_preview.update(cx, |preview, cx| {
                                            preview.record(tab, panel, browser_id, cx);
                                        });
                                    }
                                });
                            }
                            let _ = reply.send(result);
                        })
                        .detach();
                    }
                }
            }
            BridgeEvent::AskUser {
                request_id,
                caller,
                question,
                options,
                reply,
            } => {
                // Bound what agents can queue; a dropped reply fails the tool call.
                let mine = self.questions.iter().filter(|q| q.caller == caller).count();
                if !reply.is_closed()
                    && self.questions.len() < MAX_QUESTIONS
                    && mine < MAX_QUESTIONS_PER_SESSION
                {
                    self.questions.push_back(PendingQuestion {
                        id: request_id,
                        caller,
                        question,
                        options,
                        input: cx.new(|cx| InputState::new(window, cx).placeholder("Your answer…")),
                        reply,
                    });
                }
            }
            BridgeEvent::QuestionClosed { request_id } => {
                self.questions.retain(|q| q.id != request_id);
            }
            BridgeEvent::Status {
                caller,
                state,
                summary,
            } => {
                self.state.update(cx, |s, cx| {
                    if state == del_mcp::ReportedState::Working {
                        s.terminal_working.insert(caller.clone());
                    } else {
                        s.terminal_working.remove(&caller);
                    }
                    cx.notify();
                });
                if state == del_mcp::ReportedState::Done {
                    self.state.update(cx, |s, cx| s.reply_finished(&caller, cx));
                }
                self.reported
                    .insert(caller, format!("{state:?}: {summary}"));
            }
            BridgeEvent::Notify {
                caller,
                title,
                body,
            } => {
                self.notices.push_back((caller, title, body));
                while self.notices.len() > 30 {
                    self.notices.pop_front();
                }
            }
            BridgeEvent::SpawnTerminal {
                caller: _,
                request,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
                let kind = match request.provider {
                    Provider::Codex => TerminalLaunchKind::Codex,
                    Provider::Claude => TerminalLaunchKind::Claude,
                    Provider::OpenCode => TerminalLaunchKind::OpenCode,
                    Provider::Acp => {
                        let _ = reply.send(Err(HostError::Failed(
                            "ACP is a chat provider; spawn a Codex or Claude terminal".into(),
                        )));
                        return;
                    }
                };
                match TerminalLaunchSpec::agent(kind, request.cwd) {
                    Ok(spec) => self.begin_terminal(
                        spec,
                        None,
                        None,
                        Some(request.prompt),
                        Some(reply),
                        window,
                        cx,
                    ),
                    Err(e) => {
                        let _ = reply.send(Err(HostError::Failed(e.to_string())));
                    }
                }
            }
            BridgeEvent::SendTerminal {
                caller: _,
                target,
                text,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self
                        .terminals
                        .get(&target)
                        .ok_or_else(|| HostError::UnknownSession(target.to_string()))
                        .and_then(|host| {
                            host.update(cx, |h, cx| h.send_text(&text, true, cx))
                                .map_err(|e| HostError::Failed(e.to_string()))
                        });
                    let _ = reply.send(result);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_new_session_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let cwd = s.effective().2;
        let info = s.project_info.get(&cwd).cloned().unwrap_or_default();
        let options = s.draft_launch_options();
        let enabled = !s.is_starting();
        let git = !info.current.is_empty();
        let selected = options
            .branch
            .clone()
            .unwrap_or_else(|| info.current.clone());
        let label = if git {
            selected.clone()
        } else if s.project_info.contains_key(&cwd) {
            "No Git repository".into()
        } else {
            "Checking Git…".into()
        };
        let state = self.state.clone();
        let toggle_state = self.state.clone();
        let worktree = options.worktree;
        let branch = options.branch.clone();
        h_flex()
            .size_full()
            .px_2()
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("new-workspace")
                            .ghost()
                            .disabled(!enabled)
                            .h(zpx(24.))
                            .px_2()
                            .min_w_0()
                            .max_w(zpx(150.))
                            .text_size(zpx(13.))
                            .tooltip(cwd.display().to_string())
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(crate::icons::icon(IconName::Folder))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .child(crate::sessions::folder_name(&cwd)),
                                    ),
                            )
                            .text_color(rgba(0xffffff8c))
                            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.pick_folder(cx))),
                    )
                    .when(git, |row| row.child(self.branch_picker.clone())),
            )
            .when(!git, |row| {
                row.child(
                    Button::new("no-git-repository")
                        .ghost()
                        .compact()
                        .disabled(true)
                        .child(
                            div()
                                .relative()
                                .size(crate::icons::size())
                                .child(
                                    crate::icons::path("icons/git-unavailable.svg")
                                        .size(crate::icons::size()),
                                )
                                .child(
                                    div().absolute().right_0().bottom_0().size(zpx(7.)).child(
                                        crate::icons::icon(IconName::Close)
                                            .with_size(zpx(7.))
                                            .text_color(rgb(0xff646b)),
                                    ),
                                ),
                        )
                        .text_color(rgba(0xffffff70))
                        .accessibility_label(label.clone())
                        .tooltip(label),
                )
            })
            .when(git, |row| {
                row.child(
                    gpui_kit::component::checkbox::Checkbox::new("new-worktree")
                        .small()
                        .focus_ring(false)
                        .label("New worktree")
                        .checked(worktree)
                        .disabled(!enabled)
                        .on_change(move |checked, _, cx| {
                            toggle_state.update(cx, |s, cx| {
                                s.set_launch_options(branch.clone(), *checked, cx)
                            })
                        }),
                )
            })
            .child(self.render_mode_picker(cx))
    }

    fn mode_slide_position(&self, cx: &App) -> f32 {
        let target = if self.state.read(cx).mode() == SessionMode::Terminal {
            1.0
        } else {
            0.0
        };
        let Some(started) = self.mode_slide_started else {
            return target;
        };
        let t = (started.elapsed().as_secs_f32() / 0.18).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.mode_slide_from + (target - self.mode_slide_from) * eased
    }

    pub(super) fn render_mode_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.mode_slide_position(cx);
        let mut row = h_flex().relative().h(zpx(24.0)).gap_1().flex_none().child(
            div()
                .absolute()
                .top_0()
                .left(zpx(44.0 * position))
                .h(zpx(24.0))
                .w(zpx(40.0 + 20.0 * position))
                .rounded_md()
                .bg(rgba(0xffffff20)),
        );
        for (choice, label, width) in [
            (SessionMode::Chat, "Chat", 40.0),
            (SessionMode::Terminal, "Terminal", 60.0),
        ] {
            row = row.child(
                div()
                    .id(SharedString::from(format!("new-mode-{label}")))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .relative()
                    .w(zpx(width))
                    .h(zpx(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(zpx(12.0))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.state.read(cx).mode() == choice || this.state.read(cx).is_starting()
                        {
                            return;
                        }
                        this.mode_slide_from = this.mode_slide_position(cx);
                        this.mode_slide_started = Some(std::time::Instant::now());
                        this.state.update(cx, |s, cx| s.set_mode(choice, cx));
                        cx.notify();
                    })),
            );
        }
        row
    }

    pub(super) fn render_terminal_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let selection = self.state.read(cx).selection.clone();
        let mut page = v_flex().id("terminal-page").size_full().min_h_0().min_w_0();
        match selection {
            Selection::Terminal(id) => {
                if let Some(host) = self.terminals.get(&id) {
                    page = page.child(div().flex_1().min_h_0().min_w_0().child(host.clone()));
                    if host.read(cx).snapshot().status != TerminalHostStatus::Running {
                        page = page.child(self.restart_button(id, cx));
                    }
                } else {
                    if self.terminal_opening.contains(&id) || !self.terminal_attempts.contains(&id)
                    {
                        page = page.child(div().p_4().child("Reopening conversation…"));
                    } else {
                        page = page.child(
                            v_flex()
                                .p_4()
                                .gap_3()
                                .child("Could not reopen this conversation.")
                                .child(self.restart_button(id, cx)),
                        );
                    }
                }
            }
            Selection::Draft(_) | Selection::Session(_) => {}
        }

        page.children(self.render_agent_messages(cx).map(|messages| {
            div()
                .w_full()
                .max_w(zpx(crate::COMPOSER_MAX_WIDTH))
                .p_3()
                .child(messages)
        }))
        .into_any_element()
    }

    fn restart_button(&self, id: SessionId, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("restart-terminal")
            .ghost()
            .small()
            .label("Reopen conversation")
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(record) = this
                    .terminal_store
                    .records
                    .iter()
                    .find(|r| r.id == id)
                    .cloned()
                {
                    this.begin_terminal(
                        record.launch,
                        None,
                        Some(id.clone()),
                        None,
                        None,
                        window,
                        cx,
                    );
                }
            }))
    }

    fn answer_question(
        &mut self,
        id: u64,
        answer: UserAnswer,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(at) = self.questions.iter().position(|q| q.id == id)
            && let Some(question) = self.questions.remove(at)
        {
            let _ = question.reply.send(answer);
        }
        cx.notify();
    }

    pub(super) fn render_agent_messages(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let active = match &self.state.read(cx).selection {
            Selection::Session(id) | Selection::Terminal(id) => id.clone(),
            _ => return None,
        };
        let summary = self.reported.get(&active).cloned();
        let notice = self
            .notices
            .iter()
            .position(|(caller, _, _)| caller == &active);
        let question = self
            .questions
            .iter()
            .find(|q| q.caller == active && !q.reply.is_closed());
        if summary.is_none() && notice.is_none() && question.is_none() {
            return None;
        }
        let mut area = v_flex()
            .w_full()
            .min_w_0()
            .mb_2()
            .gap_2()
            .text_size(zpx(12.))
            .when_some(summary, |area, summary| {
                area.child(div().text_color(rgb(0x999999)).child(summary))
            });
        if let Some(notice) = notice {
            let (_, title, body) = &self.notices[notice];
            area = area.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(format!("{}: {}", title, body)))
                    .child(
                        Button::new("dismiss-notice")
                            .ghost()
                            .small()
                            .label("Dismiss")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.notices.remove(notice);
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(question) = question {
            let id = question.id;
            let answer_input = question.input.clone();
            let mut options = h_flex().gap_2().flex_wrap();
            for (index, text) in question.options.iter().enumerate() {
                let text = text.clone();
                options = options.child(
                    Button::new(("mcp-answer-option", index))
                        .ghost()
                        .small()
                        .h_auto()
                        .max_w_full()
                        .py_2()
                        .border_1()
                        .border_color(rgba(0xffffff18))
                        .accessibility_label(text.clone())
                        .child(div().min_w_0().whitespace_normal().child(text.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.answer_question(
                                id,
                                UserAnswer::Option {
                                    index,
                                    text: text.clone(),
                                },
                                window,
                                cx,
                            )
                        })),
                );
            }
            area = area.child(
                v_flex()
                    .id(("agent-question", id))
                    .w_full()
                    .min_w_0()
                    .max_h(zpx(280.))
                    .overflow_y_scroll()
                    .p_3()
                    .gap_3()
                    .rounded(zpx(12.))
                    .bg(crate::appearance::popup_surface())
                    .child(
                        div()
                            .text_size(zpx(14.))
                            .whitespace_normal()
                            .child(question.question.clone()),
                    )
                    .child(options)
                    .child(Input::new(&question.input))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("mcp-answer-send")
                                    .ghost()
                                    .small()
                                    .label("Send answer")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let text = answer_input.read(cx).value().trim().to_string();
                                        if !text.is_empty() {
                                            this.answer_question(
                                                id,
                                                UserAnswer::Text { text },
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                Button::new("mcp-answer-dismiss")
                                    .ghost()
                                    .small()
                                    .label("Dismiss")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.answer_question(id, UserAnswer::Dismissed, window, cx)
                                    })),
                            ),
                    ),
            );
        }
        Some(area)
    }
}

/// Resume by exact identity; never replay the opening prompt or use --last.
fn resume_args(
    provider: Option<Provider>,
    conversation: &str,
    resuming: bool,
) -> Result<Vec<String>, String> {
    if provider == Some(Provider::OpenCode) {
        if !conversation.starts_with("ses_")
            || !conversation
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("Invalid OpenCode conversation ID".into());
        }
        return Ok(vec!["--session".into(), conversation.into()]);
    }
    uuid::Uuid::parse_str(conversation).map_err(|_| "Invalid saved conversation ID".to_owned())?;
    Ok(match provider {
        Some(Provider::Codex) if resuming => vec!["resume".into(), conversation.into()],
        Some(Provider::Claude) => vec![
            if resuming { "--resume" } else { "--session-id" }.into(),
            conversation.into(),
        ],
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn reopened_agents_use_the_saved_identity_without_replaying_a_prompt() {
        let id = "01a16585-21de-7e41-9a31-9857f8268084";
        assert_eq!(
            resume_args(Some(Provider::Codex), id, true).unwrap(),
            ["resume", id]
        );
        assert_eq!(
            resume_args(Some(Provider::Claude), id, true).unwrap(),
            ["--resume", id]
        );
        assert_eq!(
            resume_args(Some(Provider::Claude), id, false).unwrap(),
            ["--session-id", id]
        );
        assert!(resume_args(Some(Provider::Codex), "--last", true).is_err());
    }
}
