# del-agent

Harness-neutral agent layer. UI and provider adapters depend on this crate only
through the types below. Tokio-based; no UI dependencies.

## Contract

- `Provider { Codex, Claude, OpenCode, Acp }`.
- `SessionConfig { cwd, model: Option<String>, reasoning_effort: Option<String>,
  service_tier: Option<String>, provider, permission_mode }`,
  `UserInput { text, attachments, command, skills }`, `ApprovalResponse { Approve, Deny }`.
  Optional command metadata preserves an explicitly selected skill through queues
  and session startup. Backends advertise `ComposerCommand` entries through
  `list_commands` and execute selections through `invoke_command`; an unsupported
  harness advertises no commands by default. Native goal loops stay in the harness.
- `AgentEvent` (serde, `{"type": "snake_case", ...}`): `SessionStarted`,
  `TurnStarted`, `AssistantTextDelta`, `AssistantMessage`, `ReasoningDelta`,
  `ToolCallStarted/Updated/Finished`, `FileChange{path,kind,diff?}`,
  `ApprovalRequested{id,kind,detail}`, `ApprovalResolved{id}` (provider settled
  or withdrew it), `TokenUsage{input,output,cached}`,
  `TurnCompleted{turn_id,status}`, `Error{message,recoverable}`,
  `PeerMessage{message}`, `PeerWorkChanged{work}`.
  Ordering per turn: `TurnStarted` … `TurnCompleted`. Deltas of a message are
  followed by its `AssistantMessage` with the full text.
- `#[async_trait] trait AgentBackend: Send + Sync` (object-safe, use
  `Arc<dyn AgentBackend>`): `start_session`, `send_prompt`, `steer`, `interrupt`,
  `resume(config, id)`, `respond_approval(session, approval_id, response)`,
  `steer_input`, `reconfigure`, `list_models`, `session_title`,
  `session_display_title`, `generate_title`, `close`, `shutdown`.
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
  `refresh_titles`, `generate_title`, `interrupt`, `respond_approval`, `close`, `delete`,
  `status`, `list_sessions`, `subscribe(id)`, `subscribe_all()` (yields
  `ManagerEvent::{Event, StatusChanged, PromptSent, TitleLoading}`), `shutdown()`. Status is
  `Idle | Running | WaitingApproval | Error` (`Error` after a non-recoverable
  `Error` event, cleared by the next prompt). The index is JSON lines at
  `default_index_path()` (`<data dir>/del/sessions.jsonl`), written atomically
  (private temp file, fsync, rename) by a background writer; `flush_index()`/
  `shutdown()` wait for it; unreadable lines are preserved. In-flight sessions
  load as `Idle` after a restart. Call `resume(id)` to reattach. `refresh_titles`
  only asks live providers (live sessions, and saved ones whose provider process
  is already running for the same account and folder); it never starts one.
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
and OpenCode support active-turn steering; Claude queues input through its streaming CLI;
ACP has no standard steering operation.

Real Codex, Claude, OpenCode, and ACP adapters implement `AgentBackend` and are registered
with the manager. The mock is for isolated tests and has no model catalog.

## Native goals

`GoalAction::{Set, Pause, Resume, Clear}` is an out-of-band control contract.
`AgentEvent::GoalUpdated { goal: Option<Goal> }` carries the native objective,
status, elapsed usage, and pause capability; `None` means cleared. A turn ending
does not imply a goal is complete. Backends restore state from their own native
store. Unsupported controls return an error; the UI shows only supported actions.

## Project peers and work lifecycle

`SessionManager` owns the provider-neutral mailbox in `sessions.peers.json`.
The app registers project membership through `set_peer_project`; callers cannot
choose their own identity or project through tool arguments. `peer_sessions` and
`authorize_peer` enforce that boundary for discovery, context, and delivery.
Sibling worktrees inherit the configured project identity.

`read_peer_context` pages recorded public events using `ContextCursor`, bounded
by a character budget. It excludes private reasoning and discloses history
retention. Reading does not send a provider prompt or alter the user's read state.

`send_to_peer` and `reply_to_peer` persist receipts before acknowledgment. Sender
idempotency keys reject changed content, and replies route through the original
request. `dispatch_peer` uses the same per-session send lock as user prompts and
never steers or interrupts a running turn. Claims that cannot be reconciled after
a restart become uncertain rather than being silently replayed.

`PeerWork` is separate from a provider turn and from native goals. Peer traffic
runs before Del's queued user input at a safe input boundary. A reply or
`TurnCompleted` does not finish the original task: `control_peer_work` explicitly
completes, waits, pauses, or resumes that work. The app checks
`peer_blocks_user_queue` before delivering another queued user message. Terminal
adapters use `receive_peer` cooperatively; their native CLI queues are not managed.

Restored or reconfigured Side chats rebind historical IDs to their current native
session. Closing a backend preserves its mailbox; deleting a session cancels its
exchanges. Reconciliation accepts only the exact recorded peer envelope with
matching resolved endpoints, avoiding duplicate raw prompts on restoration.
See [project peer sessions](../../docs/peer-sessions.md) for the full contract,
UI behavior, recovery, and verification evidence.
