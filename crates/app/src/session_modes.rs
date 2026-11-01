//! Local /terminal and /chat commands, with exclusive native conversation ownership.
use crate::{
    AppShell,
    state::{Selection, Tab},
    terminal_host::{TerminalHost, TerminalHostEvent, TerminalLaunchSpec},
};
use del_agent::{Provider, SessionId, SessionManager};
use del_mcp::{SessionInfo, SessionMode};
use gpui_kit::{
    component::button::{Button, ButtonVariants},
    component::*,
    prelude::*,
    *,
};
use std::time::Duration;

struct HandoffLease {
    manager: SessionManager,
    id: SessionId,
    bridge: crate::mcp_bridge::BridgeHandle,
    _terminal: Option<crate::mcp_bridge::TerminalLease>,
}
impl Drop for HandoffLease {
    fn drop(&mut self) {
        self.bridge.detach_terminal(&self.id);
        self.manager.release_terminal(&self.id);
    }
}

pub(super) struct ModeTerminal {
    pub host: Entity<TerminalHost>,
    session: Option<del_terminal::TerminalShutdown>,
    lease: Option<HandoffLease>,
    runtime: tokio::runtime::Handle,
}
impl ModeTerminal {
    fn stop(&mut self) -> tokio::task::JoinHandle<()> {
        let session = self.session.take();
        let lease = self.lease.take();
        self.runtime.spawn_blocking(move || {
            if let Some(session) = session {
                session.close(Duration::from_millis(500));
            }
            drop(lease);
        })
    }
}
impl Drop for ModeTerminal {
    fn drop(&mut self) {
        if self.session.is_some() {
            drop(self.stop());
        }
    }
}

pub(super) fn mode_command(text: &str) -> Option<SessionMode> {
    match text.trim() {
        "/terminal" => Some(SessionMode::Terminal),
        "/chat" => Some(SessionMode::Chat),
        _ => None,
    }
}

impl AppShell {
    pub(super) fn submit_mode_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mode) = mode_command(self.composer.read(cx).value().as_ref()) else {
            return false;
        };
        self.command_menu.update(cx, |menu, cx| menu.dismiss(cx));
        let selection = self.state.read(cx).selection.clone();
        if self.state.read(cx).is_starting() {
            return true;
        }
        match selection {
            Selection::Draft(_) => {
                if mode == SessionMode::Terminal
                    && self.state.read(cx).effective().0 == Provider::Acp
                {
                    self.set_mode_error(
                        "Terminal mode supports Codex, Claude, and OpenCode.".into(),
                        cx,
                    );
                    return true;
                }
                self.state.update(cx, |s, cx| s.set_mode(mode, cx));
            }
            Selection::Session(id) => match mode {
                SessionMode::Terminal => self.switch_chat_to_terminal(id, window, cx),
                SessionMode::Chat => self.switch_terminal_to_chat(id, window, cx),
            },
            Selection::Terminal(id) => {
                if mode == SessionMode::Chat {
                    self.import_terminal_chat(id, window, cx);
                }
            }
        }
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        true
    }

    pub(super) fn switch_chat_to_terminal(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.mode_terminals.get(&id) {
            terminal.host.update(cx, |host, cx| host.focus(window, cx));
            return;
        }
        if self.mode_switching.contains(&id) {
            return;
        }
        let manager = self.state.read(cx).backend().manager;
        let prepared = (|| {
            let record = manager.session_record(&id).map_err(|e| e.to_string())?;
            let mut spec =
                TerminalLaunchSpec::from_config(&record.config()).map_err(|e| e.to_string())?;
            spec.account_id = record.account_id.clone();
            spec.permission_mode = Some(record.permission_mode);
            spec.account().map_err(|e| e.to_string())?;
            spec.title = record.title.clone().unwrap_or_else(|| spec.label());
            let args = crate::mode_ui::resume_args(Some(record.provider), id.as_str(), true)?;
            let bridge = self
                .mcp
                .as_ref()
                .map(crate::mcp_bridge::McpBridge::handle)
                .ok_or("Agent tools are still starting. Try again in a moment.")?;
            Ok::<_, String>((spec, args, bridge))
        })();
        let (spec, resume, bridge) = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.set_mode_error(error, cx);
                return;
            }
        };
        self.mode_switching.insert(id.clone());
        self.state.update(cx, |s, cx| {
            s.keep_tab(&Tab::Session(id.clone()), cx);
            s.error = None;
        });
        let runtime = self.state.read(cx).backend().runtime;
        let task_id = id.clone();
        let launch = runtime.spawn(async move {
            let record = manager
                .handoff_to_terminal(&task_id)
                .await
                .map_err(|e| e.to_string())?;
            let mut lease = HandoffLease {
                manager,
                id: task_id.clone(),
                bridge,
                _terminal: None,
            };
            lease._terminal = lease
                .bridge
                .register_terminal(SessionInfo {
                    id: task_id,
                    provider: Some(record.provider),
                    mode: SessionMode::Terminal,
                    command: Some(spec.kind.label().into()),
                    cwd: record.cwd,
                    title: record.title,
                    status: "running".into(),
                    reported_state: None,
                })
                .map_err(|e| e.to_string())?;
            let config = lease
                ._terminal
                .as_ref()
                .ok_or("Missing terminal launch configuration")?
                .config();
            let mut args = config.args.clone();
            if spec.provider() == Some(Provider::Codex) {
                args.extend(["-c".into(), "check_for_update_on_startup=false".into()]);
            }
            args.extend(resume);
            let env = config.env.clone();
            Ok::<_, String>((spec, args, env, lease))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = launch.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.mode_switching.remove(&id);
                let result = result.and_then(|(spec, args, env, lease)| {
                    if !this
                        .state
                        .read(cx)
                        .all_open_tabs()
                        .contains(&Tab::Session(id.clone()))
                    {
                        return Ok(());
                    }
                    let provider = spec.provider();
                    let host = TerminalHost::spawn_prepared(spec, args, env, window, cx)
                        .map_err(|e| e.to_string())?;
                    let event_id = id.clone();
                    cx.subscribe_in(&host, window, move |this, _, event, window, cx| {
                        if matches!(event, TerminalHostEvent::ChatRequested) {
                            this.switch_terminal_to_chat(event_id.clone(), window, cx);
                        } else if let TerminalHostEvent::Changed(snapshot) = event
                            && this.mode_terminals.contains_key(&event_id)
                            && let Some(bridge) = &this.mcp
                        {
                            let active = snapshot.status
                                == crate::terminal_host::TerminalHostStatus::Running;
                            bridge.handle().upsert_terminal(
                                SessionInfo {
                                    id: event_id.clone(),
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
                                bridge.handle().revoke(&event_id);
                            }
                        }
                    })
                    .detach();
                    let session = host
                        .read(cx)
                        .terminal_view()
                        .read(cx)
                        .session()
                        .shutdown_handle();
                    this.mode_terminals.insert(
                        id.clone(),
                        ModeTerminal {
                            host: host.clone(),
                            session: Some(session),
                            lease: Some(lease),
                            runtime: this.state.read(cx).backend().runtime,
                        },
                    );
                    this.chats.remove(&id);
                    if this.state.read(cx).selection == Selection::Session(id.clone()) {
                        host.update(cx, |host, cx| host.focus(window, cx));
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    this.set_mode_error(format!("Could not switch to Terminal: {error}"), cx);
                    // A failed launch may have already closed the chat process.
                    if !this.state.read(cx).backend().manager.is_live(&id) {
                        this.chats.remove(&id);
                        if this.state.read(cx).selection == Selection::Session(id.clone()) {
                            this.state.update(cx, |s, cx| s.select(id.clone(), cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn switch_terminal_to_chat(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode_switching.contains(&id) {
            return;
        }
        let Some(mut terminal) = self.mode_terminals.remove(&id) else {
            return;
        };
        self.mode_switching.insert(id.clone());
        let stopped = terminal.stop();
        let backend = self.state.read(cx).backend();
        cx.spawn_in(window, async move |this, cx| {
            let stopped = stopped.await;
            let open = this
                .update_in(cx, |this, _, cx| {
                    let open = this
                        .state
                        .read(cx)
                        .all_open_tabs()
                        .contains(&Tab::Session(id.clone()));
                    if !open {
                        this.mode_switching.remove(&id);
                    }
                    open
                })
                .unwrap_or(false);
            if !open {
                return;
            }
            let result = match stopped {
                Ok(()) => backend
                    .runtime
                    .spawn({
                        let id = id.clone();
                        async move {
                            backend
                                .manager
                                .resume(&id)
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        }
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.mode_switching.remove(&id);
                this.chats.remove(&id);
                if let Err(error) = result {
                    this.set_mode_error(format!("Could not return to Chat: {error}"), cx);
                }
                if this.state.read(cx).selection == Selection::Session(id.clone()) {
                    this.ensure_chat(cx);
                    this.composer
                        .update(cx, |input, cx| input.focus(window, cx));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn import_terminal_chat(
        &mut self,
        terminal_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode_switching.contains(&terminal_id) {
            return;
        }
        let Some(record) = self
            .terminal_store
            .records
            .iter()
            .find(|record| record.id == terminal_id)
            .cloned()
        else {
            return;
        };
        let Some(identity) = record.conversation_id.as_ref() else {
            self.set_mode_error("The terminal's conversation ID is not available yet. Wait for the provider to finish starting and try /chat again.".into(), cx);
            return;
        };
        if let Err(error) = crate::mode_ui::resume_args(record.launch.provider(), identity, true) {
            self.set_mode_error(error, cx);
            return;
        }
        let Some(config) = record.launch.chat_config() else {
            return;
        };
        let Some(host) = self.terminals.get(&terminal_id) else {
            return;
        };
        let session = host
            .read(cx)
            .terminal_view()
            .read(cx)
            .session()
            .shutdown_handle();
        let id = SessionId::new(identity.clone());
        let backend = self.state.read(cx).backend();
        let bridge = self.mcp.as_ref().map(crate::mcp_bridge::McpBridge::handle);
        self.mode_switching.insert(terminal_id.clone());
        let chat_id = id.clone();
        let task = backend.runtime.spawn(async move {
            let now = del_agent::index::now_ms();
            backend
                .manager
                .import_terminal(del_agent::SessionRecord {
                    id: chat_id.clone(),
                    provider: config.provider,
                    account_id: record.launch.account_id,
                    cwd: config.cwd,
                    model: config.model,
                    reasoning_effort: config.reasoning_effort,
                    service_tier: config.service_tier,
                    permission_mode: config.permission_mode,
                    title: Some(record.title),
                    created_at_ms: now,
                    updated_at_ms: now,
                    status: del_agent::SessionStatus::Idle,
                    fork: None,
                    title_turns: 0,
                })
                .map_err(|e| e.to_string())?;
            backend
                .manager
                .handoff_to_terminal(&chat_id)
                .await
                .map_err(|e| e.to_string())?;
            let stopped =
                tokio::task::spawn_blocking(move || session.close(Duration::from_millis(500)))
                    .await;
            backend.manager.release_terminal(&chat_id);
            stopped.map_err(|e| e.to_string())?;
            backend
                .manager
                .resume(&chat_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update_in(cx, |this, window, cx| {
                this.mode_switching.remove(&terminal_id);
                match result {
                    Ok(()) => {
                        this.terminals.remove(&terminal_id);
                        if let Some(bridge) = bridge {
                            bridge.revoke(&terminal_id);
                            bridge.remove_terminal(&terminal_id);
                        }
                        let selected = this.state.read(cx).selection
                            == Selection::Terminal(terminal_id.clone());
                        let old_key = format!("terminal:{}", terminal_id.as_str());
                        let new_key = format!("chat:{}", id.as_str());
                        this.remember_right(cx);
                        if let Some(panel) = this.right_store.remove(&old_key) {
                            this.right_store.set(new_key, panel);
                        }
                        this.terminal_store.remove(&terminal_id);
                        if let Err(error) = this.terminal_store.save(&crate::terminal_store::TerminalStore::path()) {
                            this.set_mode_error(format!("Chat opened, but terminal history could not be updated: {error}"), cx);
                        }
                        this.state.update(cx, |s, cx| s.terminal_became_chat(&terminal_id, &id, cx));
                        this.sync_right(window, cx);
                        this.persist_right(cx);
                        this.chats.remove(&id);
                        if selected {
                            this.ensure_chat(cx);
                            this.focus_composer(window, cx);
                        }
                    }
                    Err(error) => {
                        this.set_mode_error(format!("Could not return to Chat: {error}"), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_mode_terminal(
        &self,
        id: &SessionId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut page = v_flex()
            .size_full()
            .min_h_0()
            .rounded_lg()
            .overflow_hidden()
            .bg(crate::appearance::page_surface());
        if let Some(terminal) = self.mode_terminals.get(id) {
            let id = id.clone();
            page = page
                .child(div().flex_1().min_h_0().child(terminal.host.clone()))
                .child(
                    Button::new("return-to-chat")
                        .ghost()
                        .small()
                        .label("/chat — Return to chat")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.switch_terminal_to_chat(id.clone(), window, cx)
                        })),
                );
        } else {
            page = page.child(div().p_4().child("Switching session mode…"));
        }
        if let Some(error) = &self.state.read(cx).error {
            page = page.child(div().p_2().child(error.clone()));
        }
        page.children(
            self.render_agent_messages(cx)
                .map(|messages| div().p_3().child(messages)),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn only_standalone_mode_commands_are_intercepted() {
        assert_eq!(mode_command(" /terminal "), Some(SessionMode::Terminal));
        assert_eq!(mode_command("/chat"), Some(SessionMode::Chat));
        for text in [
            "explain /terminal",
            "/terminal run this",
            "/chat hello",
            "/terminal\nhello",
            "/terminally",
        ] {
            assert_eq!(mode_command(text), None);
        }
    }
}
