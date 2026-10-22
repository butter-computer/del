# del-agent

Harness-neutral agent layer. UI and provider adapters depend on this crate only
through the types below. Tokio-based; no UI dependencies.

## Contract

- `Provider { Codex, Claude, Acp }`.
- `SessionConfig { cwd, model: Option<String>, reasoning_effort: Option<String>,
  service_tier: Option<String>, provider, permission_mode }`,
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
  `steer_input`, `reconfigure`, `list_models`, `session_title`,
  `session_display_title`, `close`, `shutdown`.
  `start_session`/`resume` return a `SessionHandle`
  (`id`, `provider`, `history()`, `subscribe() -> broadcast::Receiver<AgentEvent>`).
  The manager records `SessionStarted` as the first history event. Backends can
  provide a restored history snapshot through `SessionHandle::with_history`;
  consumers use `subscribe_with_history` to attach without losing replay events.
  `send_prompt` while a turn runs returns `AgentError::Busy`.
  `interrupt` ends the turn with `TurnCompleted{Interrupted}`.
- `MockBackend::new(provider).with_delay(d)`: prompt containing `error`, `approve`
  or `tool` selects a scripted scenario; anything else streams plain text.
- `SessionManager::new(index_path, backends)`: `start_session`, `resume(id)`,
  `send_prompt`, `steer`, `steer_input`, `set_model_settings`, `list_models`,
  `refresh_titles`, `interrupt`, `respond_approval`, `close`, `delete`,
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

## Models, images, and settings

`ModelInfo` carries provider-owned identifiers, display names, descriptions,
effort choices/defaults, and an optional fast service tier. Model discovery does
not submit a prompt. Provider adapters own discovery and capability checks.

Model settings are persisted in the index and applied between turns through
`reconfigure`; the default implementation closes and resumes the same provider
session. A busy session does not change settings mid-turn. The app owns the
durable follow-up queue and retries known busy rejections after completion.

`Attachment` contains a local path and optional MIME type. Adapters encode image
inputs in the provider's format. `steer_input` supports attachments when the
provider overrides it; the default rejects image steering explicitly. Codex
supports active-turn steering; Claude queues input through its streaming CLI;
ACP has no standard steering operation.

Real Codex, Claude, and ACP adapters implement `AgentBackend` and are registered
with the manager. The mock is for isolated tests and has no model catalog.
