#[cfg(test)]
mod tests {
    use crate::mcp_bridge::McpBridge;
    use del_agent::SessionManager;
    use del_agent::{
        AgentBackend, AgentEvent, GoalAction, GoalStatus, PermissionMode, Provider, SessionConfig,
        UserInput,
    };
    use std::sync::Arc;

    /// End-to-end native test: the model must use MCP, and its request must
    /// reactivate a completed native goal (not just change assistant prose).
    #[tokio::test]
    #[ignore]
    async fn real_claude_agent_resumes_completed_goal_through_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(del_claude::ClaudeBackend::new(del_claude::ClaudeOptions {
            setting_sources: Some("project".into()),
            ..Default::default()
        }));
        let manager = SessionManager::new(
            dir.path().join("index.json"),
            [backend.clone() as Arc<dyn AgentBackend>],
        )
        .unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = McpBridge::start(manager.clone(), tx, dir.path().join("mcp"))
            .await
            .unwrap();
        backend.set_browser_tools(bridge.handle().chat_browser_tools().unwrap());
        let result = tokio::time::timeout(std::time::Duration::from_secs(90), async {
            let handle = manager.start_session(SessionConfig {
                cwd: dir.path().to_owned(), provider: Provider::Claude,
                model: None, reasoning_effort: None, service_tier: None,
                permission_mode: PermissionMode::FullAuto,
            }).await.unwrap();
            let mut events = manager.subscribe(&handle.id).unwrap();
            manager.control_goal(&handle.id, GoalAction::Set("Reply with the word READY. The goal is met once you have replied READY. Do not use tools.".into())).await.unwrap();
            loop {
                if matches!(events.recv().await.unwrap(), AgentEvent::GoalUpdated { goal: Some(g) } if g.status == GoalStatus::Complete) { break; }
            }
            // Completion metadata can precede the stream's turn result.
            while manager.status(&handle.id) == Some(del_agent::SessionStatus::Running) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            manager.send_prompt(&handle.id, UserInput::text("Resume the completed goal. You MUST call the Del goal MCP tool with action resume exactly once. Then give a short acknowledgement and finish this turn so the queued goal can run. Do not use shell or browser tools.")).await.unwrap();
            let mut called = false;
            loop {
                match events.recv().await.unwrap() {
                    AgentEvent::ToolCallStarted { name, .. } if name.contains("goal") => { called = true; eprintln!("Goal MCP invoked"); },
                    AgentEvent::GoalUpdated { goal: Some(g) } if called && g.status == GoalStatus::Active => { eprintln!("Native goal resumed: {}s", g.time_used_seconds); break; },
                    _ => {}
                }
            }
            assert!(called);
            manager.control_goal(&handle.id, GoalAction::Clear).await.unwrap();
        }).await;
        manager.shutdown().await;
        bridge.shutdown();
        result.expect("agent-driven native goal resume timed out");
    }
}
