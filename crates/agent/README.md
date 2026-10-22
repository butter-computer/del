# del-agent

Harness-neutral agent layer. UI and provider adapters depend on this crate only
through the types below. Tokio-based; no UI dependencies.

## Contract

- `Provider { Codex, Claude }`.
- `SessionConfig { cwd, model: Option<String>, provider, permission_mode }`,
  `UserInput { text, attachments }`, `ApprovalResponse { Approve, Deny }`.
- `AgentEvent` (serde, `{"type": "snake_case", ...}`): `SessionStarted`,
  `TurnStarted`, `AssistantTextDelta`, `AssistantMessage`, `ReasoningDelta`,
  `ToolCallStarted/Updated/Finished`, `FileChange{path,kind,diff?}`,
  `ApprovalRequested{id,kind,detail}`, `TokenUsage{input,output,cached}`,
  `TurnCompleted{turn_id,status}`, `Error{message,recoverable}`.
  Ordering per turn: `TurnStarted` … `TurnCompleted`. Deltas of a message are
  followed by its `AssistantMessage` with the full text.
- `#[async_trait] trait AgentBackend: Send + Sync` (object-safe, use
  `Arc<dyn AgentBackend>`): `start_session`, `send_prompt`, `steer`, `interrupt`,
  `resume(config, id)`, `respond_approval(session, approval_id, response)`,
  `close`, `shutdown`. `start_session`/`resume` return a `SessionHandle`
  (`id`, `provider`, `subscribe() -> broadcast::Receiver<AgentEvent>`).
  A backend emits `SessionStarted` itself; late subscribers miss it.
  `send_prompt` while a turn runs returns `AgentError::Busy`.
  `interrupt` ends the turn with `TurnCompleted{Interrupted}`.
- `MockBackend::new(provider).with_delay(d)`: prompt containing `error`, `approve`
  or `tool` selects a scripted scenario; anything else streams plain text.
- `SessionManager::new(index_path, backends)`: `start_session`, `resume(id)`,
  `send_prompt`, `steer`, `interrupt`, `respond_approval`, `close`, `delete`,
  `status`, `list_sessions`, `subscribe(id)`, `subscribe_all()` (yields
  `ManagerEvent::{Event, StatusChanged}`), `shutdown()`. Status is
  `Idle | Running | WaitingApproval | Error` (`Error` after a non-recoverable
  `Error` event, cleared by the next prompt). The index is JSON lines at
  `default_index_path()` (`<data dir>/del/sessions.jsonl`), written atomically;
  in-flight sessions load as `Idle` after a restart. Call `resume(id)` to reattach.
- `detect_all()` / `detect(provider)` → `ProviderStatus { installed, path, version,
  login: LoggedIn | LoggedOut | Unknown }`. Uses `codex login status` and
  `claude auth status`; only the exit code / `loggedIn` flag is read, never tokens.
  Try `cargo run -p del-agent --example detect`.

Real Codex/Claude adapters implement `AgentBackend` and are registered with the manager.
