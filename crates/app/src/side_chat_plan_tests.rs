use super::*;
use ::core::prelude::v1::test;
use std::{sync::Arc, time::Duration};

#[::gpui::test]
fn side_chat_displays_its_own_progress_and_saves_it(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let agent: Arc<dyn del_agent::AgentBackend> =
        Arc::new(del_agent::MockBackend::new(Provider::Codex));
    let manager = del_agent::SessionManager::new(dir.path().join("index"), [agent]).unwrap();
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::Codex,
        model: None,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: del_agent::PermissionMode::Ask,
    };
    let main = runtime
        .block_on(manager.start_session(config.clone()))
        .unwrap();
    runtime
        .block_on(manager.send_prompt(&main.id, UserInput::text("checklist")))
        .unwrap();
    let context = Context {
        activity_owner: None,
        backend: Backend {
            runtime: runtime.handle().clone(),
            manager: manager.clone(),
        },
        source: ForkSource::New {
            config,
            account_id: None,
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
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("plan-checklist").is_none(),
        "main progress must not leak into an independent Side chat"
    );
    cx.update(|window, cx| {
        side.update(cx, |side, cx| {
            side.input
                .update(cx, |input, cx| input.set_value("checklist", window, cx));
            side.submit(window, cx);
        })
    });
    for _ in 0..200 {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(2)).await });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if cx.debug_bounds("plan-checklist").is_some() {
            break;
        }
    }
    assert!(
        cx.debug_bounds("plan-checklist").is_some(),
        "Side transcript must render its provider's plan"
    );
    side.update(cx, |side, cx| {
        side.save(cx);
        let saved = serde_json::to_value(&side.saved).unwrap();
        assert!(
            saved.to_string().contains("plan_updated"),
            "the Side snapshot must retain structured progress"
        );
    });
    runtime.block_on(manager.shutdown());
}
