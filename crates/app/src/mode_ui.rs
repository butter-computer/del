//! Main-app session modes and the GPUI end of the MCP command channel.
use crate::mcp_bridge::{BridgeEvent, McpBridge};
use crate::sessions::TerminalRow;
use crate::state::{Selection, Tab};
use crate::terminal_host::{
    TerminalHost, TerminalHostEvent, TerminalHostSnapshot, TerminalHostStatus, TerminalLaunchKind,
    TerminalLaunchSpec, TerminalLauncher, TerminalLauncherEvent,
};
use crate::terminal_store::{TerminalRecord, TerminalStore};
use crate::{AppShell, composer_ui};
use del_agent::{Provider, SessionId};
use del_mcp::{HostError, HostResult, SessionInfo, SessionMode, UserAnswer};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};

pub(super) struct PendingQuestion {
    id: u64,
    caller: SessionId,
    question: String,
    options: Vec<String>,
    reply: oneshot::Sender<UserAnswer>,
}

pub(super) fn tab_mark(provider: Option<Provider>) -> AnyElement {
    match provider {
        Some(provider) => composer_ui::provider_logo(provider, 14.).into_any_element(),
        None => div().text_size(px(12.)).child(">_").into_any_element(),
    }
}

impl AppShell {
    pub(super) fn start_modes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let previous_question = self.questions.front().map(|q| q.id);
        let closed = self
            .state
            .update(cx, |s, _| std::mem::take(&mut s.closed_terminals));
        for id in closed {
            if let Some(host) = self.terminals.remove(&id) {
                host.update(cx, |host, cx| host.close(cx));
            }
            if let Some(bridge) = &self.mcp {
                bridge.handle().revoke(&id);
                bridge.handle().remove_terminal(&id);
            }
            self.questions.retain(|q| q.caller != id);
        }
        if previous_question != self.questions.front().map(|q| q.id) {
            self.mcp_answer
                .update(cx, |i, cx| i.set_value("", window, cx));
        }
        let draft_keys: Vec<_> = self
            .state
            .read(cx)
            .tabs
            .iter()
            .filter_map(|t| {
                if let Tab::Draft(k) = t {
                    Some(*k)
                } else {
                    None
                }
            })
            .collect();
        self.launchers.retain(|k, _| draft_keys.contains(k));
        self.launcher_cwds.retain(|k, _| draft_keys.contains(k));
        let state = self.state.read(cx);
        if state.mode() != SessionMode::Terminal {
            return;
        }
        if let Selection::Draft(key) = state.selection {
            let cwd = state.effective().2;
            if let std::collections::hash_map::Entry::Vacant(entry) = self.launchers.entry(key) {
                let launcher = cx.new(|cx| TerminalLauncher::new(cwd.clone(), window, cx));
                cx.subscribe_in(&launcher, window, move |this, _, event, window, cx| {
                    let TerminalLauncherEvent::Launch(spec) = event;
                    this.begin_terminal(spec.clone(), Some(key), None, None, None, window, cx);
                })
                .detach();
                entry.insert(launcher);
                self.launcher_cwds.insert(key, cwd);
            } else if self.launcher_cwds.get(&key) != Some(&cwd) {
                self.launchers[&key].update(cx, |l, cx| l.set_workspace(cwd.clone(), cx));
                self.launcher_cwds.insert(key, cwd);
            }
        }
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
            title: Some(spec.label()),
            status: "running".into(),
            reported_state: None,
        };
        let bridge = self.mcp.as_ref().map(McpBridge::handle);
        let prepare_info = info.clone();
        let runtime = self.state.read(cx).backend().runtime;
        let prepare = runtime.spawn_blocking(move || {
            if let Some(bridge) = bridge {
                bridge.upsert_terminal(prepare_info.clone(), None);
                if let Some(provider) = prepare_info.provider {
                    let config = bridge
                        .launch_config(&prepare_info.id, provider)
                        .map_err(|e| e.to_string())?;
                    return Ok((config.args, config.env));
                }
            } else if prepare_info.provider.is_some() {
                return Err("Agent tools are still starting. Try again in a moment.".into());
            }
            Ok((Vec::new(), Vec::new()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = prepare.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                if reply.as_ref().is_some_and(|r| r.is_closed()) {
                    if let Some(bridge) = &this.mcp {
                        bridge.handle().revoke(&id);
                        bridge.handle().remove_terminal(&id);
                    }
                    return;
                }
                let result = prepared.and_then(|(mut args, env)| {
                    if let Some(prompt) = prompt {
                        args.push("--".into());
                        args.push(prompt);
                    }
                    let host = TerminalHost::spawn_prepared(spec.clone(), args, env, window, cx)
                        .map_err(|e| e.to_string())?;
                    let event_id = id.clone();
                    cx.subscribe(&host, move |this, _, event, cx| {
                        if let TerminalHostEvent::Changed(snapshot) = event {
                            this.terminal_changed(&event_id, snapshot, cx);
                        }
                    })
                    .detach();
                    host.update(cx, |host, cx| host.focus(window, cx));
                    this.terminals.insert(id.clone(), host);
                    this.terminal_store.upsert(TerminalRecord {
                        id: id.clone(),
                        title: spec.label(),
                        launch: spec.clone(),
                    });
                    if let Err(e) = this.terminal_store.save(&TerminalStore::path()) {
                        this.set_mode_error(
                            format!("Terminal started, but history could not be saved: {e}"),
                            cx,
                        );
                    }
                    this.state.update(cx, |s, cx| {
                        s.terminal_started(
                            draft,
                            TerminalRow {
                                id: id.clone(),
                                title: spec.label(),
                                cwd: spec.cwd.clone(),
                                provider: spec.provider(),
                                active: true,
                                summary: String::new(),
                            },
                            cx,
                        )
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
                    if let Some(launcher) = draft.and_then(|key| this.launchers.get(&key)) {
                        launcher.update(cx, |l, cx| l.launch_failed(error.clone(), cx));
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
        let active = snapshot.status == TerminalHostStatus::Running;
        self.state.update(cx, |s, cx| {
            if let Some(row) = s.terminal_rows.iter_mut().find(|r| &r.id == id) {
                row.title = snapshot.title.clone();
                row.active = active;
                row.summary = snapshot.summary.clone().unwrap_or_default();
            }
            cx.notify();
        });
        if let Some(bridge) = &self.mcp {
            let provider = match snapshot.kind {
                TerminalLaunchKind::Codex => Some(Provider::Codex),
                TerminalLaunchKind::Claude => Some(Provider::Claude),
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
        cx.notify();
    }

    fn set_mode_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.error = Some(error);
            cx.notify();
        });
    }

    fn handle_bridge(&mut self, event: BridgeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let previous_question = self.questions.front().map(|q| q.id);
        match event {
            BridgeEvent::AskUser {
                request_id,
                caller,
                question,
                options,
                reply,
            } => {
                if !reply.is_closed() {
                    self.questions.push_back(PendingQuestion {
                        id: request_id,
                        caller,
                        question,
                        options,
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
        if previous_question != self.questions.front().map(|q| q.id) {
            self.mcp_answer
                .update(cx, |i, cx| i.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(super) fn render_mode_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.state.read(cx).mode();
        let mut row = h_flex().gap_1().pb_2();
        for (choice, label) in [
            (SessionMode::Chat, "Chat"),
            (SessionMode::Terminal, "Terminal"),
        ] {
            row = row.child(
                Button::new(SharedString::from(format!("new-mode-{label}")))
                    .ghost()
                    .small()
                    .label(label)
                    .when(mode == choice, |b| b.bg(rgba(0xffffff20)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.set_mode(choice, cx))
                    })),
            );
        }
        row
    }

    pub(super) fn render_terminal_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let selection = self.state.read(cx).selection.clone();
        let mut page = v_flex()
            .id("terminal-page")
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(0));
        match selection {
            Selection::Draft(key) => {
                let state = self.state.clone();
                page = page
                    .child(
                        h_flex()
                            .p_3()
                            .gap_3()
                            .items_center()
                            .child(self.render_mode_picker(cx))
                            .child(
                                Button::new("terminal-workspace")
                                    .ghost()
                                    .small()
                                    .label("Choose workspace…")
                                    .on_click(move |_, _, cx| {
                                        state.update(cx, |s, cx| s.pick_folder(cx))
                                    }),
                            ),
                    )
                    .when_some(self.launchers.get(&key).cloned(), |p, launcher| {
                        p.child(div().flex_1().min_h_0().child(launcher))
                    });
            }
            Selection::Terminal(id) => {
                if let Some(host) = self.terminals.get(&id) {
                    page = page.child(div().flex_1().min_h_0().min_w_0().child(host.clone()));
                    if host.read(cx).snapshot().status != TerminalHostStatus::Running {
                        page = page.child(self.restart_button(id, cx));
                    }
                } else {
                    page = page.child(v_flex().p_4().gap_3().child("This terminal is stopped. Restart opens a new process in its saved workspace.")
                        .child(self.restart_button(id,cx)));
                }
            }
            Selection::Session(_) => {}
        }
        if let Some(error) = self.state.read(cx).error.clone() {
            page = page.child(div().p_3().text_color(rgb(0xf08b8b)).child(error));
        }
        page.into_any_element()
    }

    fn restart_button(&self, id: SessionId, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("restart-terminal")
            .ghost()
            .small()
            .label("Restart terminal")
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(at) = self.questions.iter().position(|q| q.id == id)
            && let Some(question) = self.questions.remove(at)
        {
            let _ = question.reply.send(answer);
        }
        self.mcp_answer
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn render_agent_messages(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let active = match &self.state.read(cx).selection {
            Selection::Session(id) | Selection::Terminal(id) => Some(id),
            _ => None,
        };
        let summary = active.and_then(|id| self.reported.get(id)).cloned();
        if summary.is_none() && self.notices.is_empty() && self.questions.is_empty() {
            return None;
        }
        let mut area = v_flex()
            .w_full()
            .px_3()
            .gap_1()
            .text_size(px(12.))
            .when_some(summary, |area, summary| {
                area.child(div().text_color(rgb(0x999999)).child(summary))
            });
        if let Some((caller, title, body)) = self.notices.front() {
            area = area.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(format!(
                        "{} · {}: {}",
                        self.caller_name(caller, cx),
                        title,
                        body
                    )))
                    .child(
                        Button::new("dismiss-notice")
                            .ghost()
                            .small()
                            .label("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notices.pop_front();
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(question) = self.questions.front() {
            let id = question.id;
            let mut options = h_flex().gap_2().flex_wrap();
            for (index, text) in question.options.iter().enumerate() {
                let text = text.clone();
                options = options.child(
                    Button::new(("mcp-answer-option", index))
                        .ghost()
                        .small()
                        .label(text.clone())
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
                    .p_3()
                    .gap_2()
                    .rounded_md()
                    .bg(rgb(0x262629))
                    .child(format!(
                        "{} asks: {}",
                        self.caller_name(&question.caller, cx),
                        question.question
                    ))
                    .child(options)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.mcp_answer)))
                            .child(
                                Button::new("mcp-answer-send")
                                    .ghost()
                                    .small()
                                    .label("Send answer")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let text =
                                            this.mcp_answer.read(cx).value().trim().to_string();
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

    fn caller_name(&self, id: &SessionId, cx: &App) -> String {
        self.state
            .read(cx)
            .terminal_rows
            .iter()
            .find(|r| &r.id == id)
            .map(|r| r.title.clone())
            .unwrap_or_else(|| {
                let title = self.state.read(cx).current_title(id);
                if title.is_empty() {
                    id.to_string()
                } else {
                    title
                }
            })
    }
}
