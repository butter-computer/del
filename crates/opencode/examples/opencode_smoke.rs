//! Run with isolated XDG_DATA_HOME / XDG_STATE_HOME for non-user history.
use del_agent::*;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()?
            .success()
    );
    let skill_dir = dir.path().join(".agents/skills/del-smoke");
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: del-smoke\ndescription: Del integration verification fixture\n---\nFor this verification, include DEL_NATIVE_SKILL_CONFIRMED in your answer, and read the attached file to report its verification marker.\n",
    )?;
    let attachment = dir.path().join("verification.txt");
    std::fs::write(
        &attachment,
        "The attachment verification marker is DEL_FILE_7391_OK.",
    )?;
    let backend = del_opencode::OpenCodeBackend::default();
    let models = backend.list_models(dir.path()).await?;
    println!("Discovered {} models", models.len());
    let commands = backend.list_commands(dir.path()).await?;
    println!("Discovered {} commands/skills", commands.len());
    assert!(!commands.iter().any(|c| c.kind == CommandKind::Goal));
    let skill = commands
        .iter()
        .find(|c| c.name == "del-smoke" && c.kind == CommandKind::Skill)
        .expect("local native skill not discovered")
        .clone();
    let model = std::env::var("DEL_OPENCODE_SMOKE_MODEL").ok().or_else(|| {
        models
            .iter()
            .find(|m| m.id == "opencode/big-pickle")
            .map(|m| m.id.clone())
    });
    let config = SessionConfig {
        cwd: dir.path().into(),
        provider: Provider::OpenCode,
        model,
        reasoning_effort: None,
        service_tier: None,
        permission_mode: PermissionMode::FullAuto,
    };
    let session = backend.start_session(config.clone()).await?;
    println!("Created {}", session.id);
    let mut events = session.subscribe();
    backend
        .send_prompt(
            &session.id,
            UserInput::text("Reply with exactly DEL_OPENCODE_OK. Do not use tools."),
        )
        .await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(150), async {
        let mut text = String::new();
        loop {
            let event = events.recv().await.unwrap();
            match event {
                AgentEvent::AssistantTextDelta { text: delta, .. } => text.push_str(&delta),
                AgentEvent::TurnCompleted { status, .. } => { assert_eq!(status,TurnStatus::Completed); break; },
                AgentEvent::Error { message, .. } => eprintln!("{message}"),
                _ => {}
            }
        }
        assert!(text.contains("DEL_OPENCODE_OK"),"reply: {text}");
        let mut input = UserInput::text("Follow the selected skill and report both verification markers.");
        input.skills.push(skill);
        input.attachments.push(Attachment {path:attachment, mime:Some("text/plain".into())});
        backend.send_prompt(&session.id, input).await.unwrap();
        let mut skill_text = String::new();
        loop {
            match events.recv().await.unwrap() {
                AgentEvent::AssistantTextDelta {text, ..} => skill_text.push_str(&text),
                AgentEvent::TurnCompleted {status, ..} => {
                    assert_eq!(status, TurnStatus::Completed);
                    break;
                }
                AgentEvent::Error {message, ..} => panic!("{message}"),
                _ => {}
            }
        }
        assert!(skill_text.contains("DEL_NATIVE_SKILL_CONFIRMED"), "skill reply: {skill_text}");
        assert!(skill_text.contains("DEL_FILE_7391_OK"), "attachment reply: {skill_text}");
        println!("PASS: native skill execution and file attachment");
        backend.send_prompt(&session.id, UserInput::text(
            "Create verification-output.txt in this project with exactly DEL_WRITE_8364_OK using your file editing tools. Do not change any other file."
        )).await.unwrap();
        let mut tool_finished = false;
        let mut file_changed = false;
        loop {
            match events.recv().await.unwrap() {
                AgentEvent::ToolCallFinished {status:ToolStatus::Completed,..} => tool_finished = true,
                AgentEvent::FileChange {path,kind,diff} if path.ends_with("verification-output.txt") => {
                    assert_eq!(kind, FileChangeKind::Created);
                    assert!(diff.unwrap_or_default().contains("DEL_WRITE_8364_OK"));
                    file_changed = true;
                }
                AgentEvent::TurnCompleted {status,..} => {
                    assert_eq!(status, TurnStatus::Completed);
                    break;
                }
                AgentEvent::Error {message,..} => panic!("{message}"),
                _ => {}
            }
        }
        assert!(tool_finished, "native tool completion was not emitted");
        assert!(file_changed, "native file diff was not emitted");
        assert!(std::fs::read_to_string(dir.path().join("verification-output.txt")).unwrap().contains("DEL_WRITE_8364_OK"));
        println!("PASS: native file tool execution and diff display events");
        backend.close(&session.id).await.unwrap();
        let restored = backend.resume(config,&session.id).await.unwrap();
        assert!(restored.history().iter().any(|e| matches!(e,AgentEvent::AssistantMessage { text,.. } if text.contains("DEL_OPENCODE_OK"))));
        assert!(restored.history().iter().any(|e| matches!(e, AgentEvent::FileChange {path,..} if path.ends_with("verification-output.txt"))));
        println!("PASS: chat streaming and exact-session history/diff resume");
        let search_cwd=dir.path().to_owned();
        let search_id=session.id.to_string();
        let indexed=tokio::task::spawn_blocking(move || {
            del_opencode::history::Cache::default().read(&search_cwd,&search_id)
        }).await.unwrap().unwrap();
        assert!(indexed.iter().any(|e| matches!(e,AgentEvent::AssistantMessage {text,..} if text.contains("DEL_OPENCODE_OK"))));
        println!("PASS: native export conversation search indexing");
    }).await;
    backend.shutdown().await?;
    result.expect("OpenCode smoke timed out");
    Ok(())
}
