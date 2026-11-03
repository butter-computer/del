//! The backend against a scripted `claude` executable: launch arguments,
//! account isolation, catalog discovery, history restore and stdout tolerance.
use del_agent::*;
use del_claude::{ClaudeBackend, ClaudeOptions, account_usage};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

const SCRIPT: &str = r#"#!/usr/bin/python3
import json, os, sys
profile = os.environ.get('CLAUDE_CONFIG_DIR', '')
with open(os.path.join(profile, 'launches.jsonl'), 'a') as f:
    f.write(json.dumps({'argv': sys.argv[1:], 'cwd': os.getcwd(),
        'profile': os.environ.get('DEL_ACCOUNT_PROFILE')}) + '\n')
mode = open(os.path.join(profile, 'mode')).read().strip() if os.path.exists(os.path.join(profile, 'mode')) else ''
def send(value):
    print(json.dumps(value), flush=True)
if mode == 'exit':
    sys.stdin.readline()
    sys.exit(0)
for line in sys.stdin:
    message = json.loads(line)
    if message['type'] == 'control_request':
        request = message['request']
        if mode == 'error':
            send({'type': 'control_response', 'response': {'subtype': 'error',
                'request_id': message['request_id'], 'error': 'fixture discovery failed'}})
            continue
        if request['subtype'] == 'initialize':
            body = {'models': [
                {'value': 'default', 'displayName': 'Default', 'resolvedModel': 'claude-fixture-default', 'supportsEffort': True,
                 'supportedEffortLevels': ['low', 'high'], 'supportsFastMode': True},
                {'value': 'fixture-model', 'description': 'No display name'},
                {'displayName': 'missing value'}],
                'commands': [
                {'name': 'goal', 'description': 'Goal', 'builtin': True},
                {'name': 'fast', 'description': 'Fast', 'builtin': True},
                {'name': 'compact', 'description': 'Compact', 'argumentHint': '[focus]', 'builtin': True},
                {'name': 'review', 'description': 'Review skill'},
                {'name': 'color', 'description': 'Terminal only', 'builtin': True},
                {'name': '_internal', 'description': 'Hidden'},
                {'name': 'old', 'description': '(removed) gone'}]}
        elif request['subtype'] == 'get_settings':
            body = {'effective': {'model': 'fixture-model'} if mode == 'configured' else {'modelSettings': {'claude-fixture-default': {'effortLevel': 'high'}}}}
        else:
            body = {'echo': request, 'five_hour': {'utilization': 12}}
        send({'type': 'control_response', 'response': {'subtype': 'success',
            'request_id': message['request_id'], 'response': body}})
    elif message['type'] == 'user':
        with open(os.path.join(profile, 'inputs.jsonl'), 'a') as f:
            f.write(json.dumps(message) + '\n')
        if message['message']['content'] == 'exit':
            sys.stderr.write('fixture stderr detail\n')
            sys.stderr.flush()
            sys.exit(3)
        print('plain log noise', flush=True)
        print('[1, 2, 3]', flush=True)
        print('{"type":"future_event","payload":{}}', flush=True)
        sys.stdout.write('{"type":"assistant","message":{"id":"m","role":"assistant",')
        sys.stdout.flush()
        sys.stdout.write('"content":[{"type":"text","text":"reply é"}]}}\n')
        sys.stdout.flush()
        send({'type': 'result', 'subtype': 'success', 'is_error': False,
            'usage': {'input_tokens': 3, 'cache_creation_input_tokens': 4,
                      'cache_read_input_tokens': 5, 'output_tokens': 6}})
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    profile: std::path::PathBuf,
    cwd: std::path::PathBuf,
    options: ClaudeOptions,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude-fixture");
    std::fs::write(&script, SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let profile = dir.path().join("profile");
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    Fixture {
        options: ClaudeOptions {
            binary: script,
            config_dir: Some(profile.clone()),
            ..Default::default()
        },
        profile,
        cwd,
        _dir: dir,
    }
}

fn config(cwd: &Path, mode: PermissionMode) -> SessionConfig {
    SessionConfig {
        cwd: cwd.into(),
        model: None,
        reasoning_effort: None,
        service_tier: None,
        provider: Provider::Claude,
        permission_mode: mode,
    }
}

/// Wait until the script has recorded `count` launches.
async fn launches(profile: &Path, count: usize) -> Vec<Value> {
    let path = profile.join("launches.jsonl");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let rows: Vec<Value> = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            if rows.len() >= count {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("claude fixture was not launched")
}

fn argv(launch: &Value) -> Vec<String> {
    launch["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_owned())
        .collect()
}

fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
    args.windows(2).any(|w| w[0] == flag && w[1] == value)
}

async fn until_turn_end(rx: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut seen = vec![];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            let done = matches!(event, AgentEvent::TurnCompleted { .. });
            seen.push(event);
            if done {
                return;
            }
        }
    })
    .await
    .expect("turn did not finish");
    seen
}

#[tokio::test]
async fn temporary_effort_changes_restart_without_resume_and_preserve_context() {
    let f = fixture();
    let backend = std::sync::Arc::new(ClaudeBackend::new(f.options.clone()));
    let manager = SessionManager::new(
        f._dir.path().join("index"),
        [backend as std::sync::Arc<dyn AgentBackend>],
    )
    .unwrap();
    let fork = manager
        .fork(
            ForkSource::New {
                config: config(&f.cwd, PermissionMode::Plan),
                account_id: None,
            },
            ForkLifetime::Temporary,
        )
        .await
        .unwrap();
    let id = fork.handle.id;
    let (_, mut rx) = manager.subscribe_with_history(&id).unwrap();
    manager
        .send_prompt(&id, UserInput::text("remember the blue balloon"))
        .await
        .unwrap();
    until_turn_end(&mut rx).await;
    for effort in ["low", "high"] {
        manager
            .set_model_settings(&id, Some("fixture-model".into()), Some(effort.into()), None)
            .unwrap();
        manager
            .send_prompt(&id, UserInput::text(format!("continue at {effort}")))
            .await
            .unwrap();
        let events = until_turn_end(&mut rx).await;
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::Error { .. })));
    }
    let rows = launches(&f.profile, 3).await;
    for effort in ["low", "high"] {
        let args = rows
            .iter()
            .map(argv)
            .find(|args| has_pair(args, "--effort", effort))
            .unwrap();
        assert!(args.iter().any(|a| a == "--no-session-persistence"));
        assert!(has_pair(&args, "--session-id", id.as_str()));
        assert!(!args.iter().any(|a| a == "--resume"));
    }
    let inputs: Vec<Value> = std::fs::read_to_string(f.profile.join("inputs.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(inputs.len(), 3);
    for input in &inputs[1..] {
        let text = input["message"]["content"].as_str().unwrap();
        assert!(text.contains("remember the blue balloon"));
        assert!(text.contains("reply é"));
        assert!(text.contains("reference material"));
    }
    assert!(
        inputs[2]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("continue at low")
    );
    assert!(manager.is_temporary(&id));
    let history = manager.subscribe_with_history(&id).unwrap().0;
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(event, AgentEvent::UserMessage { .. }))
            .count(),
        3
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn catalog_default_uses_effective_settings_without_changing_the_alias() {
    let f = fixture();
    std::fs::write(
        f.options.config_dir.as_ref().unwrap().join("mode"),
        "configured",
    )
    .unwrap();
    let backend = ClaudeBackend::new(f.options.clone());
    let models = backend.list_models(&f.cwd).await.unwrap();
    let default = models.iter().find(|m| m.is_default).unwrap();
    assert_eq!(default.id, "default");
    assert_eq!(default.name, "fixture-model");
    assert!(default.efforts.is_empty() && default.fast_tier.is_none());
}

#[tokio::test]
async fn catalog_discovery_uses_the_selected_profile_without_persisting_a_session() {
    let f = fixture();
    let backend = ClaudeBackend::new(f.options.clone());
    let models = backend.list_models(&f.cwd).await.unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["default", "fixture-model"],
        "rows without a value are skipped"
    );
    assert!(models[0].is_default);
    assert_eq!(models[0].name, "claude-fixture-default");
    assert_eq!(models[0].default_effort.as_deref(), Some("high"));
    assert!(models[0].supports_effort("high"));
    assert_eq!(models[0].fast_tier.as_deref(), Some("fast"));
    assert_eq!(models[1].name, "fixture-model", "name falls back to the id");
    assert!(models[1].efforts.is_empty() && models[1].fast_tier.is_none());

    let commands = backend.list_commands(&f.cwd).await.unwrap();
    let rows: Vec<_> = commands
        .iter()
        .map(|c| (c.name.as_str(), c.kind, c.source.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("goal", CommandKind::Goal, "Claude Code"),
            ("fast", CommandKind::Fast, "Claude Code"),
            ("compact", CommandKind::Action, "Claude Code"),
            ("review", CommandKind::Skill, "Skill"),
        ]
    );
    assert_eq!(commands[0].argument_hint, "<objective> · clear");
    assert_eq!(commands[2].argument_hint, "[focus]");

    let rows = launches(&f.profile, 2).await;
    for launch in &rows {
        let args = argv(launch);
        for flag in ["-p", "--verbose", "--no-session-persistence"] {
            assert!(args.contains(&flag.to_owned()), "{flag} missing: {args:?}");
        }
        assert!(has_pair(&args, "--input-format", "stream-json"));
        assert!(has_pair(&args, "--output-format", "stream-json"));
        assert!(!args.iter().any(|a| a == "--session-id" || a == "--resume"));
        assert_eq!(launch["profile"], "1");
        assert_eq!(Path::new(launch["cwd"].as_str().unwrap()), f.cwd);
    }
}

#[tokio::test]
async fn usage_is_a_native_control_query_and_discovery_errors_are_reported() {
    let f = fixture();
    let usage = account_usage(&f.options, &f.cwd).await.unwrap();
    assert_eq!(
        usage["echo"],
        json!({"subtype":"get_usage","skip_behaviors":true})
    );
    assert_eq!(usage["five_hour"]["utilization"], 12);

    std::fs::write(f.profile.join("mode"), "error").unwrap();
    let error = ClaudeBackend::new(f.options.clone())
        .list_models(&f.cwd)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("fixture discovery failed"),
        "{error}"
    );

    std::fs::write(f.profile.join("mode"), "exit").unwrap();
    let error = ClaudeBackend::new(f.options.clone())
        .list_commands(&f.cwd)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("exited before returning models"),
        "{error}"
    );
}

#[tokio::test]
async fn session_launches_follow_mode_model_and_persistence() {
    let f = fixture();
    let backend = ClaudeBackend::new(f.options.clone());
    let mut chat = config(&f.cwd, PermissionMode::AutoEdit);
    chat.model = Some("fixture-model".into());
    chat.reasoning_effort = Some("high".into());
    let handle = backend.start_session(chat).await.unwrap();
    let rows = launches(&f.profile, 1).await;
    let args = argv(&rows[0]);
    assert!(has_pair(&args, "--session-id", handle.id.as_str()));
    assert!(uuid_like(handle.id.as_str()));
    assert!(has_pair(&args, "--permission-mode", "acceptEdits"));
    assert!(has_pair(&args, "--model", "fixture-model"));
    assert!(has_pair(&args, "--effort", "high"));
    assert!(has_pair(&args, "--permission-prompt-tool", "stdio"));
    assert!(has_pair(&args, "--settings", "{\"fastMode\":false}"));
    for flag in [
        "--include-partial-messages",
        "--replay-user-messages",
        "--verbose",
    ] {
        assert!(args.contains(&flag.to_owned()), "{flag} missing");
    }
    assert!(!args.contains(&"--dangerously-skip-permissions".to_owned()));
    assert!(!args.contains(&"--no-session-persistence".to_owned()));
    assert_eq!(rows[0]["profile"], "1");
    assert_eq!(Path::new(rows[0]["cwd"].as_str().unwrap()), f.cwd);

    let temporary = backend
        .start_temporary_session(config(&f.cwd, PermissionMode::Plan))
        .await
        .unwrap();
    let rows = launches(&f.profile, 2).await;
    let args = argv(&rows[1]);
    assert!(has_pair(&args, "--session-id", temporary.id.as_str()));
    assert!(has_pair(&args, "--permission-mode", "plan"));
    assert!(args.contains(&"--no-session-persistence".to_owned()));
    assert!(!args.contains(&"--model".to_owned()));
    assert!(!args.contains(&"--effort".to_owned()));
    backend.shutdown().await.unwrap();
}

fn uuid_like(id: &str) -> bool {
    id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4
}

#[tokio::test]
async fn resume_restores_the_profile_transcript_and_resumes_its_id() {
    let f = fixture();
    let id = "6b1f4a52-0d7e-4c55-9a3b-2f0f6f1f9e10";
    let project: String = f
        .cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = f.profile.join("projects").join(project);
    std::fs::create_dir_all(&dir).unwrap();
    let records = [
        json!({"type":"user","uuid":"u1","parentUuid":null,"sessionId":id,"message":{"content":"first question"}}),
        json!({"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":id,"message":{"content":[{"type":"text","text":"first answer"}]}}),
        json!({"type":"user","uuid":"u2","parentUuid":"a1","sessionId":id,"message":{"content":"second question"}}),
        json!({"type":"assistant","uuid":"a2","parentUuid":"u2","sessionId":id,"message":{"content":[{"type":"text","text":"second answer"}]}}),
    ];
    let text: String = records.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(dir.join(format!("{id}.jsonl")), text).unwrap();

    let backend = ClaudeBackend::new(f.options.clone());
    let handle = backend
        .resume(
            config(&f.cwd, PermissionMode::FullAuto),
            &SessionId::new(id),
        )
        .await
        .unwrap();
    let summary: Vec<String> = handle
        .history()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::UserMessage { text } => Some(format!("user {text}")),
            AgentEvent::AssistantMessage { turn_id, text } => {
                Some(format!("assistant {turn_id} {text}"))
            }
            AgentEvent::TurnStarted { turn_id } => Some(format!("start {turn_id}")),
            AgentEvent::TurnCompleted { turn_id, status } => {
                Some(format!("done {turn_id} {status:?}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        summary,
        [
            "user first question",
            "start u1",
            "assistant u1 first answer",
            "done u1 Completed",
            "user second question",
            "start u2",
            "assistant u2 second answer",
            "done u2 Completed",
        ]
    );
    let rows = launches(&f.profile, 1).await;
    let args = argv(&rows[0]);
    assert!(has_pair(&args, "--resume", id));
    assert!(has_pair(&args, "--permission-mode", "bypassPermissions"));
    assert!(args.contains(&"--dangerously-skip-permissions".to_owned()));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn stdout_noise_and_split_writes_do_not_break_a_turn_and_exit_reports_stderr() {
    let f = fixture();
    let backend = ClaudeBackend::new(f.options.clone());
    let handle = backend
        .start_session(config(&f.cwd, PermissionMode::Ask))
        .await
        .unwrap();
    let mut rx = handle.subscribe();
    backend
        .send_prompt(&handle.id, UserInput::text("hello"))
        .await
        .unwrap();
    let events = until_turn_end(&mut rx).await;
    let turn = match &events[0] {
        AgentEvent::TurnStarted { turn_id } => turn_id.clone(),
        other => panic!("expected TurnStarted, got {other:?}"),
    };
    assert_eq!(
        events[1..],
        [
            AgentEvent::AssistantMessage {
                turn_id: turn.clone(),
                text: "reply é".into()
            },
            AgentEvent::TokenUsage {
                input: 7,
                output: 6,
                cached: 5
            },
            AgentEvent::TurnCompleted {
                turn_id: turn,
                status: TurnStatus::Completed
            },
        ]
    );

    backend
        .send_prompt(&handle.id, UserInput::text("exit"))
        .await
        .unwrap();
    let events = until_turn_end(&mut rx).await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnCompleted {
            status: TurnStatus::Failed,
            ..
        })
    ));
    let error = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&error, AgentEvent::Error { message, recoverable: false }
            if message.contains("code 3") && message.contains("fixture stderr detail")),
        "{error:?}"
    );
    backend.shutdown().await.unwrap();
}
