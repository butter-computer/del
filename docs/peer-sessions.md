# Project peer sessions

## Contract

Every Del session in a project is an equal peer, regardless of provider. Agents
can discover peers, read their recorded context without requesting a model turn,
send requests, and reply to requests. Requests can ask for information or changes;
the receiving session keeps its existing provider/account and permissions.

A model turn ending does not necessarily finish a task. An exchange must preserve
any original work. Peer messages run ahead of queued user messages at a safe input
boundary; handling or replying to a peer request must not release those user
messages. The current work must explicitly complete first. A request to another
agent returns a durable receipt immediately, rather than blocking two agents that
might be waiting on each other. Replies are routed by request ID and do not demand
another reply automatically.

## Existing implementation and references

| Surface | Evidence | Reuse / change |
| --- | --- | --- |
| Del authenticated MCP | `crates/mcp/src/server.rs`, `host.rs`, HTTP tests | Keep per-launch identity and browser isolation; add project peer capabilities instead of parent/child-only access. |
| Del session lifecycle | `crates/agent/src/manager.rs` | Keep provider/account configuration, serialization, and normalized events. Peer traffic must not use steering to interrupt active tools. |
| Del queued input | `crates/app/src/chat/view.rs`, `followups.rs` | Keep edit/reorder/retry UI; gate delivery on work state and pending peer messages, not only TurnCompleted. |
| OpenClaw session tools | https://docs.openclaw.ai/concepts/session-tool ; `src/agents/tools/sessions-send-executor.ts`, `sessions-history-tool.ts` | Carry over authenticated source attribution, request identity, access checks, and disclosed history pagination. Do not copy parent-only resume placement: Del requires equal peers. |
| OpenClaw security model | https://github.com/openclaw/openclaw/blob/main/SECURITY.md | Treat peer text/tool output as attributed data; it never grants new permissions or becomes a human instruction. Del's project boundary is checked on each operation. |

The model selects a peer, decides what context to read, and performs the requested
work. Code handles identity, project membership, persistence, FIFO within peer
traffic, peer-before-user priority, delivery receipts, and UI events. No model
output prose is parsed to guess task completion or intent.

## Creating chat peers

`spawn_session` is available to session-scoped chat credentials. It creates a normal
chat, not a terminal or a special subordinate role. The caller supplies provider,
its own working directory, and a task. Del verifies the canonical directory,
inherits the caller’s registered project and permissions, and preserves account,
model, effort, and tier when the provider matches. Switching provider uses that
provider’s configured default backend/model; it never copies an incompatible model.
A failed provider start does not dispatch work to another provider.

The first task is persisted through the regular peer mailbox. The new chat receives
a request ID and replies with `reply_to_session`; callers must not send the task
again after spawning. The existing background dispatcher wakes unopened chat views.
A caller waiting for several workers uses `session_work wait`; replies resume its
work, and completion remains blocked while requests are unanswered. No four-worker
planner or prose-based completion detector is added.

Existing spawn reservations cap concurrent creation (8 per caller, 32 total by
default). Each successful call creates a new session; creation itself is not
idempotent, so agents must inspect sessions before retrying an unknown outcome.
Terminal callers retain the existing terminal launch behavior.

## Recovery and visibility

Persist an outbound request before acknowledging it. Idempotency keys are scoped
to the authenticated sender; reusing a key with different content is an error.
Save the delivery claim before invoking a provider. If a crash occurs across the
provider acknowledgment boundary, do not blindly replay a potentially executed
change: reconcile against recorded delivery identity or expose uncertain status.
Queued requests and original-work state survive restarts. Deleted or inaccessible
sessions fail explicitly; they are never substituted with another session.

Requests and replies appear in both conversations as a centered
`Current agent ⇄ Other agent` row with provider logos. Both names link to their
conversation; the row has no body text, delivery label, error copy, or card
background. Navigation ticks, working indicators,
and unread completion indicators must include peer work. Reading another chat must
not change its user's read state or active selection.

## Required verification

- Same-project reads and sends across providers; sibling worktrees share their
  configured project while unrelated projects are denied.
- Sender identity cannot be supplied or forged in request arguments.
- Idle wakeup; active-turn protection; FIFO peer messages ahead of user queue;
  peer replies do not finish original work; explicit work completion releases it.
- Bidirectional/cyclic requests cannot deadlock the tool call; reply IDs route
  to the requesting session and cannot be answered by an unrelated session.
- Saved and live context (including tool results), paginated with explicit limits.
- Idempotent retry, simultaneous send/reply, restart recovery, ambiguous delivery,
  provider failure, closed/deleted session, and permission preservation.
- UI attribution, jump-to-chat, navigation ticks, working/unread states, and
  provider tool discovery verified through the actual app bridge.

## Provider and terminal boundaries

Codex and Claude chats keep per-conversation MCP credentials. OpenCode chat uses
one native server per conversation because its MCP configuration is process-wide.
ACP receives HTTP MCP configuration when advertised, otherwise Del's stdio proxy.
No tool accepts a sender identity or project supplied by the model.

Managed chat delivery is serialized with normal provider sends. The agent marks
`session_work complete` only after its original work is finished; Del does not
infer that from prose or `TurnCompleted`. `wait` requires an outstanding reply,
while `pause` keeps both work and queued user inputs intact. The UI has a resume
control. Cancelling a failed exchange or deleting its recipient wakes a waiting
sender so it can handle the failure.

A terminal CLI has no provider-neutral turn/input-queue API. Terminal agents can
pull the next message with `session_work receive` while working and explicitly
report `idle`/`done` to permit a wakeup prompt. Unknown/working status never permits
PTY injection. Native CLI user queues are outside Del's scheduler; strict priority
is enforced for Del-managed chat queues. Terminal context reads disclose that only
the recorded output tail and mailbox exchanges are available.

Mailbox data is saved atomically with private file permissions. A restored Side
chat's native ID is rebound to its prior mailbox, including outstanding replies;
old links resolve to the restored conversation. Delivery interrupted across a
provider acknowledgement becomes `uncertain`, not an automatic retry. The user
can inspect the recipient and retry or dismiss the delivery from the row's
context menu.

## Verification evidence

The 0.1.18 workspace suite passed 550 tests (42 opt-in tests ignored), and strict
Clippy passed. See the [release validation](releases/0.1.18-validation.md). The deterministic manager and HTTP bridge tests cover cross-provider project
scope, sender authentication, idempotence, cyclic requests, context pagination,
queue priority, and original-work retention. ACP fixture tests exercise negotiated
HTTP/stdio MCP transport and credential rebinding/revocation. UI model tests cover
peer receipt updates without duplicate conversation-navigation ticks. An additional
manager lifecycle test verifies a pending Side chat request through restart,
model reconfiguration, and cleanup of the old native session, including stable
project identity, idempotent retries, reply routing, and preserved permissions.
Restored receipts are reconciled with the canonical mailbox after native ID changes.
The Side chat UI test also verifies that disposing a saved view closes its backend
without deleting its mailbox; explicit chat deletion remains a separate operation.
A GPUI navigation regression verifies that opening a peer retains the sidebar tab
strip. After these fixes, all 67 agent tests and 284 app tests passed, as did strict
Clippy for the affected crates.

An opt-in live test (`live_codex_peers_exchange_context_and_finish_original_work`)
passed against the installed Codex CLI: two isolated conversations read context,
exchanged a queued request and reply, and explicitly completed their work. It uses
fresh temporary metadata and does not message existing user conversations.

Peer tools advertise their actual non-destructive, project-local behavior. Codex
confirmation-only MCP forms use Del's ordinary approval UI and grant only one
call; structured forms and URL authentication are not silently accepted. A waiting
chat ends its current turn while retaining its task; the reply resumes it without
busy-polling or releasing queued user messages.

A live main-chat / Side-chat exchange in Del verified context reading, request and
reply cards, delivered receipts, working indicators, and blue unread dots in both
the top tab and left sidebar. Peer links return to the original main chat even
when another inner tool tab is selected; Side chat navigation retains its tab
strip. Navigation ticks include requests and replies.

A fresh exchange was completed and checked across two app restarts: both receipts
remained delivered, completed work stayed complete, and the Side chat's new native
ID was rebound without replaying the request. The restored transcript and peer
links were inspected through the app's accessibility tree. Provider delivery
prompts retain historical IDs; reconciliation now resolves those aliases and
requires an exact matching envelope, preventing duplicate raw prompt rows after
restore. Regression tests also reject altered envelopes and unrelated senders.
The rebuilt local app is running with caffeine enabled. Desktop screenshot capture
was intermittently stale, so the final envelope correction was verified with the
lifecycle regression and restored accessibility state rather than a fresh pixel
capture. Native Claude/OpenCode live exchanges have not been verified;
provider-neutral behavior is covered by the bridge tests.

A separate top-level session, “Isolated Del cross-session demo,” exchanged a
request and reply with READY in the same project. Both receipts were delivered,
and the reply returned `PEER_UI_COBALT_1030`. The rebuilt UI was visually checked
for both provider logos, centered conversation links, and expanded tool output
aligned with the assistant text. Expanded tool groups no longer add a nested
left indent. Tool-call headers use plain labels without leading icons.

### Chat creation regression (2026-10-30)

The original four-worker app test exposed that chat credentials hid `spawn_session`
and the app host could only create terminals. The new bridge regression reproduced
that failure before the fix. The corrected agent/MCP suite passes 102 tests (two
opt-in tests ignored), with strict Clippy passing for both crates and their targets.
Coverage includes four distinct chat peers, inherited account/model/permissions,
canonical directory and symlink rejection before creation, cross-provider defaults,
unavailable-provider failure without an orphan, saved reply routing/project identity,
and standalone host delivery of the first request.

A fresh Codex conversation in the rebuilt local app received one natural-language
request to create four new Codex chats, delegate arithmetic, alphabetization,
letter counting, and prime-number tasks, and report only after all four finished.
No follow-up messages or direct worker prompts were supplied by the tester. The
requesting agent created all four real chats through `spawn_session`; each worker
called `reply_to_session` and then `session_work complete`. The saved mailbox
contains exactly four requests and four delivered replies involving only the five
test chats. All five work records are complete. The requesting agent checked the
worker summaries and reported 391; apple, banana, kiwi, pear; 13 letters; and
2, 3, 5, 7, 11, 13. The final result and worker unread dots were visible in Del.

Replies that arrived while the requesting turn was still running were queued;
its early wait/receive attempts were rejected by the existing safe-boundary
rules, and it ended its turn and received all four continuations without human
intervention. This live run verifies Codex; cross-provider creation/configuration
is covered by deterministic tests, not a new live Claude/OpenCode/ACP run.
The local app was left running with caffeine enabled.
