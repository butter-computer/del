use super::*;
use ::core::prelude::v1::test;
use del_agent::{AgentBackend, CommandKind, ComposerCommand, Result, SessionHandle};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

struct CatalogBackend {
    mock: del_agent::MockBackend,
    folders: Arc<Mutex<Vec<PathBuf>>>,
}

#[async_trait::async_trait]
impl AgentBackend for CatalogBackend {
    fn provider(&self) -> Provider {
        Provider::Codex
    }
    async fn list_commands(&self, cwd: &Path) -> Result<Vec<ComposerCommand>> {
        self.folders.lock().unwrap().push(cwd.to_path_buf());
        Ok([
            ("review", CommandKind::Skill),
            ("goal", CommandKind::Goal),
            ("fast", CommandKind::Fast),
            ("compact", CommandKind::Action),
            ("chat", CommandKind::Action),
            ("terminal", CommandKind::Action),
        ]
        .into_iter()
        .map(|(name, kind)| ComposerCommand {
            name: name.into(),
            description: "Side account command".into(),
            argument_hint: String::new(),
            kind,
            source: "side account".into(),
            target: (kind == CommandKind::Skill).then(|| {
                cwd.join("skills/review/SKILL.md")
                    .to_string_lossy()
                    .into_owned()
            }),
        })
        .collect())
    }
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.mock.start_session(config).await
    }
    async fn start_session_with_context(
        &self,
        config: SessionConfig,
        path: &Path,
    ) -> Result<SessionHandle> {
        self.mock.start_session_with_context(config, path).await
    }
    async fn send_prompt(&self, id: &SessionId, input: UserInput) -> Result<()> {
        self.mock.send_prompt(id, input).await
    }
    async fn steer(&self, id: &SessionId, text: String) -> Result<()> {
        self.mock.steer(id, text).await
    }
    async fn interrupt(&self, id: &SessionId) -> Result<()> {
        self.mock.interrupt(id).await
    }
    async fn resume(&self, config: SessionConfig, id: &SessionId) -> Result<SessionHandle> {
        self.mock.resume(config, id).await
    }
    async fn respond_approval(
        &self,
        id: &SessionId,
        approval: &str,
        response: del_agent::ApprovalResponse,
    ) -> Result<()> {
        self.mock.respond_approval(id, approval, response).await
    }
    async fn close(&self, id: &SessionId) -> Result<()> {
        self.mock.close(id).await
    }
}

#[::gpui::test]
fn side_catalog_keyboard_goals_and_fast_are_scoped_to_the_side_chat(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let system: Arc<dyn AgentBackend> = Arc::new(del_agent::MockBackend::new(Provider::Codex));
    let manager = del_agent::SessionManager::new(dir.path().join("index"), [system]).unwrap();
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::Codex,
        model: None,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: del_agent::PermissionMode::Plan,
    };
    let main = runtime
        .block_on(manager.start_session(config.clone()))
        .unwrap();
    let folders = Arc::new(Mutex::new(Vec::new()));
    manager.register_account(
        "side-account".into(),
        Arc::new(CatalogBackend {
            mock: del_agent::MockBackend::new(Provider::Codex),
            folders: folders.clone(),
        }),
    );
    let context = Context {
        activity_owner: None,
        backend: Backend {
            runtime: runtime.handle().clone(),
            manager: manager.clone(),
        },
        source: ForkSource::New {
            config,
            account_id: Some("side-account".into()),
        },
        accounts: vec![],
    };
    let (side, cx) = cx.add_window_view(|window, cx| SideChat::new(context, window, cx));
    for _ in 0..200 {
        cx.run_until_parked();
        if side.read_with(cx, |s, cx| {
            s.chat
                .as_ref()
                .is_some_and(|chat| !chat.read(cx).is_loading())
        }) {
            break;
        }
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
    }
    cx.update(|window, cx| {
        side.update(cx, |side, cx| {
            side.input.update(cx, |input, cx| {
                input.set_value("please /", window, cx);
                input.set_selected_range(8..8, cx);
                input.focus(window, cx);
                cx.emit(InputEvent::Change);
            });
        })
    });
    for _ in 0..200 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if cx.debug_bounds("composer-command-review").is_some() {
            break;
        }
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
    }
    assert!(
        !folders.lock().unwrap().is_empty(),
        "catalog must use the Side account backend"
    );
    assert!(
        folders
            .lock()
            .unwrap()
            .iter()
            .all(|path| path == dir.path())
    );
    for name in [
        "composer-command-review",
        "composer-command-goal",
        "composer-command-fast",
        "composer-command-compact",
    ] {
        assert!(cx.debug_bounds(name).is_some(), "missing {name}");
    }
    for name in ["composer-command-chat", "composer-command-terminal"] {
        assert!(
            cx.debug_bounds(name).is_none(),
            "Side chats must omit {name}"
        );
    }
    cx.simulate_keystrokes("down up tab");
    let draft = side.read_with(cx, |side, cx| {
        assert_eq!(side.input.read(cx).value().as_ref(), "please /review ");
        let draft = side.draft_input(cx);
        assert_eq!(draft.skills.len(), 1);
        assert_eq!(
            draft.skills[0].target,
            Some(
                dir.path()
                    .join("skills/review/SKILL.md")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(
            side.chat
                .as_ref()
                .unwrap()
                .read(cx)
                .search_messages()
                .is_empty(),
            "accepting a completion must not submit"
        );
        draft
    });
    cx.update(|window, cx| {
        side.update(cx, |side, cx| {
            let mut with_goal = draft.clone();
            with_goal.text.push_str("/goal");
            side.input.update(cx, |input, cx| {
                input.set_value(
                    crate::command_menu::restore_skill_tokens(&with_goal),
                    window,
                    cx,
                );
                let end = with_goal.text.len();
                input.set_selected_range(end..end, cx);
                cx.emit(InputEvent::Change);
            });
        })
    });
    for _ in 0..200 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if cx.debug_bounds("composer-command-goal").is_some() {
            break;
        }
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
    }
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|window, cx| {
        side.update(cx, |side, cx| {
            assert_eq!(side.goal_mode.as_ref().unwrap().kind, CommandKind::Goal);
            let goal = side.draft_input(cx);
            assert_eq!(goal.skills, draft.skills);
            assert_eq!(goal.text, draft.text);
            side.save(cx);
            assert_eq!(side.saved.draft.command, goal.command);
            let chat = side.chat.clone().unwrap();
            let id = chat.update(cx, |chat, cx| chat.enqueue(goal.clone(), cx));
            side.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            side.set_goal_mode(None, window, cx);
            side.edit_queued(&id, window, cx);
            assert_eq!(side.draft_input(cx), goal);
            side.set_goal_mode(None, window, cx);
            side.models = vec![del_agent::ModelInfo {
                id: "test-model".into(),
                name: "Test model".into(),
                description: String::new(),
                efforts: vec![],
                default_effort: None,
                is_default: true,
                fast_tier: Some("priority".into()),
            }];
            side.input
                .update(cx, |input, cx| input.set_value("/fast on", window, cx));
            side.submit(window, cx);
            assert!(side.error.is_none(), "{:?}", side.error);
            assert_eq!(
                side.record().unwrap().service_tier.as_deref(),
                Some("priority")
            );
            assert!(
                manager
                    .session_record(&main.id)
                    .unwrap()
                    .service_tier
                    .is_none()
            );
            assert!(side.input.read(cx).value().is_empty());
            assert!(
                chat.read(cx).search_messages().is_empty(),
                "Fast is a local setting, not a prompt"
            );
            side.models.clear();
            side.input
                .update(cx, |input, cx| input.set_value("/fast on", window, cx));
            side.submit(window, cx);
            assert!(side.error.as_deref().unwrap().contains("unavailable"));
            assert_eq!(side.input.read(cx).value().as_ref(), "/fast on");
        })
    });
    // Changing only this Side chat's account must discard its previous catalog.
    cx.update(|window, cx| {
        side.update(cx, |side, cx| {
            side.input.update(cx, |input, cx| {
                input.set_value("please /", window, cx);
                input.set_selected_range(8..8, cx);
            });
            let (mut config, _) = side.configuration().unwrap();
            config.service_tier = None;
            side.change_settings(config, None, cx);
        })
    });
    for _ in 0..200 {
        cx.run_until_parked();
        if side.read_with(cx, |side, cx| {
            !side.loading
                && side
                    .record()
                    .is_some_and(|record| record.account_id.is_none())
                && side
                    .chat
                    .as_ref()
                    .is_some_and(|chat| !chat.read(cx).is_loading())
        }) {
            break;
        }
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
    side.read_with(cx, |side, _| {
        assert!(side.error.is_none(), "{:?}", side.error);
        assert!(side.record().unwrap().account_id.is_none());
    });
    assert!(
        cx.debug_bounds("composer-command-review").is_none(),
        "old account's skill must not survive a switch"
    );
    assert!(
        manager
            .session_record(&main.id)
            .unwrap()
            .service_tier
            .is_none()
    );
    runtime.block_on(manager.shutdown());
}
