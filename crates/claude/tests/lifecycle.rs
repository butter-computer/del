//! Deterministic process recovery through the same manager used by the app.
use del_agent::*;
use del_claude::{ClaudeBackend, ClaudeOptions};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

#[tokio::test]
async fn next_send_restarts_exited_claude_and_preserves_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude-fixture");
    std::fs::write(
        &script,
        r#"#!/usr/bin/python3
import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if message['type'] != 'user':
        continue
    if message['message']['content'] == 'exit':
        sys.exit(1)
    assert '--resume' in sys.argv
    assert sys.argv[sys.argv.index('--permission-mode') + 1] == 'plan'
    print(json.dumps({'type':'assistant', 'message':{'id':'reply', 'role':'assistant',
        'content':[{'type':'text','text':'recovered'}]}}), flush=True)
    print(json.dumps({'type':'result','subtype':'success','is_error':False,
        'result':'recovered','usage':{}}), flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let backend = Arc::new(ClaudeBackend::new(ClaudeOptions {
        binary: script,
        config_dir: Some(dir.path().join("profile")),
        ..Default::default()
    }));
    let manager = SessionManager::new(
        dir.path().join("sessions.jsonl"),
        [backend.clone() as Arc<dyn AgentBackend>],
    )
    .unwrap();
    let session = manager
        .start_session(SessionConfig {
            cwd: dir.path().into(),
            provider: Provider::Claude,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::Plan,
        })
        .await
        .unwrap();
    let original = backend.session_process(&session.id).await.unwrap().0;
    let (_, mut events) = manager.subscribe_with_history(&session.id).unwrap();
    manager
        .send_prompt(&session.id, UserInput::text("exit"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(
            events.recv().await.unwrap(),
            AgentEvent::Error {
                recoverable: false,
                ..
            }
        ) {}
    })
    .await
    .unwrap();
    let result = manager
        .send_prompt(&session.id, UserInput::text("hello"))
        .await;
    if result.is_ok() {
        assert_ne!(
            backend.session_process(&session.id).await.unwrap().0,
            original
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut replied = false;
            loop {
                match events.recv().await.unwrap() {
                    AgentEvent::AssistantMessage { text, .. } => replied |= text == "recovered",
                    AgentEvent::TurnCompleted { status, .. } => {
                        assert_eq!(status, TurnStatus::Completed);
                        assert!(
                            replied,
                            "the original chat must receive the restarted process's reply"
                        );
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    }
    manager.shutdown().await;
    result.expect("next send must restart Claude without closing or reopening the chat");
}
