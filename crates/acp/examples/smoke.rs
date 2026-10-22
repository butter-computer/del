//! Run explicitly: cargo run -p del-acp --example smoke -- gemini
//! Uses the agent's existing login. Works in a temporary working directory.
use del_acp::{AcpBackend, registry};
use del_agent::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let (agents, warning) = registry::load();
    if let Some(warning) = warning {
        eprintln!("Registry warning: {warning}");
    }
    for status in registry::detect(&agents) {
        eprintln!(
            "{}: {}",
            status.agent.id,
            status
                .path
                .map_or("not installed".into(), |p| p.display().to_string())
        );
    }
    let selected = std::env::args().nth(1);
    let cwd = tempfile::tempdir()?;
    let backend = AcpBackend::new(agents);
    let run = async {
        let handle = backend
            .start_session(SessionConfig {
                provider: Provider::Acp,
                model: selected,
                reasoning_effort: None,
                service_tier: None,
                cwd: cwd.path().into(),
                permission_mode: PermissionMode::Ask,
            })
            .await?;
        let mut events = handle.subscribe();
        let mut tools = 0;
        let mut approvals = 0;
        let mut assistant_output = false;
        backend.send_prompt(&handle.id, UserInput::text(
            "Run the shell command `printf ACP_SMOKE_OK` using your shell tool, requesting permission if needed. Then reply with ACP_SMOKE_OK. Do not read or modify files."
        )).await?;
        loop {
            let event = events
                .recv()
                .await
                .map_err(|e| AgentError::Backend(e.to_string()))?;
            eprintln!("{event:?}");
            match event {
                AgentEvent::AssistantMessage { text, .. } if !text.is_empty() => {
                    assistant_output = true
                }
                AgentEvent::ToolCallStarted { .. } => tools += 1,
                AgentEvent::ApprovalRequested { id, kind, detail } => {
                    approvals += 1;
                    let answer = if kind == ApprovalKind::Command
                        && detail.contains("printf")
                        && detail.contains("ACP_SMOKE_OK")
                    {
                        ApprovalResponse::Approve
                    } else {
                        ApprovalResponse::Deny
                    };
                    backend.respond_approval(&handle.id, &id, answer).await?;
                }
                AgentEvent::TurnCompleted { status, .. } => {
                    if status != TurnStatus::Completed {
                        return Err(AgentError::Backend(format!("smoke turn ended {status:?}")));
                    }
                    break;
                }
                AgentEvent::Error {
                    message,
                    recoverable: false,
                } => return Err(AgentError::Backend(message)),
                _ => {}
            }
        }
        eprintln!("Prompt completed; observed {tools} tools and {approvals} permission requests.");
        if !assistant_output {
            return Err(AgentError::Backend(
                "Agent returned no assistant output; prompt/tool/permission smoke is unverified"
                    .into(),
            ));
        }
        backend
            .send_prompt(
                &handle.id,
                UserInput::text("Explain ACP cancellation in detail."),
            )
            .await?;
        backend.interrupt(&handle.id).await?;
        loop {
            let event = events
                .recv()
                .await
                .map_err(|e| AgentError::Backend(e.to_string()))?;
            if let AgentEvent::TurnCompleted { status, .. } = event {
                eprintln!("Cancellation settled: {status:?}");
                if status != TurnStatus::Interrupted {
                    return Err(AgentError::Backend("cancel did not interrupt".into()));
                }
                break;
            }
        }
        Ok(())
    };
    let outcome = tokio::time::timeout(Duration::from_secs(90), run).await;
    backend.shutdown().await?;
    outcome.map_err(|_| AgentError::Backend("smoke timed out after 90 seconds".into()))?
}
