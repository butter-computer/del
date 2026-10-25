//! Native stop, busy rejection, and steering checks; use isolated XDG directories.
use del_agent::*;
use std::time::Duration;
use tokio::sync::broadcast;

async fn next(events: &mut broadcast::Receiver<AgentEvent>) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(90), events.recv())
        .await
        .expect("native event timed out")
        .expect("native stream closed")
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let backend = del_opencode::OpenCodeBackend::default();
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::OpenCode,
        model: Some("opencode/big-pickle".into()),
        reasoning_effort: None,
        service_tier: None,
        permission_mode: PermissionMode::FullAuto,
    };
    let session = backend.start_session(config).await?;
    let mut events = session.subscribe();
    backend
        .send_prompt(
            &session.id,
            UserInput::text("Run sleep 30 in the shell. Wait for it to finish before replying."),
        )
        .await?;
    loop {
        match next(&mut events).await {
            AgentEvent::ToolCallStarted { .. } => break,
            AgentEvent::TurnCompleted { .. } => panic!("sleep turn ended before tool execution"),
            AgentEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert!(matches!(
        backend
            .send_prompt(&session.id, UserInput::text("Must not be admitted"))
            .await,
        Err(AgentError::Busy(_))
    ));
    backend.interrupt(&session.id).await?;
    loop {
        if let AgentEvent::TurnCompleted { status, .. } = next(&mut events).await {
            assert_eq!(status, TurnStatus::Interrupted);
            break;
        }
    }
    println!("PASS: busy rejection and native stop");
    backend
        .send_prompt(
            &session.id,
            UserInput::text("Run sleep 5 in the shell. Wait for it to finish before replying."),
        )
        .await?;
    loop {
        match next(&mut events).await {
            AgentEvent::ToolCallStarted { .. } => break,
            AgentEvent::TurnCompleted { .. } => panic!("steering turn ended before tool execution"),
            AgentEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    backend
        .steer(
            &session.id,
            "After the current shell call, reply with exactly DEL_STEER_1849_OK.".into(),
        )
        .await?;
    let mut text = String::new();
    loop {
        match next(&mut events).await {
            AgentEvent::AssistantTextDelta { text: delta, .. } => text.push_str(&delta),
            AgentEvent::TurnCompleted { status, .. } => {
                assert_eq!(status, TurnStatus::Completed);
                break;
            }
            AgentEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert!(text.contains("DEL_STEER_1849_OK"), "steered reply: {text}");
    println!("PASS: native active-turn steering after stop/retry");
    backend.shutdown().await?;
    Ok(())
}
