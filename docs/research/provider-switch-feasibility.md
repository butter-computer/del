# Provider switching feasibility

Research date: 2026-11-01. This historical assessment preceded implementation.
Provider switching is now implemented; see the [0.1.24 release notes](../releases/0.1.24.md)
and [validation record](../releases/0.1.24-validation.md). This document is retained
as research background, not an active implementation plan.

## Confirmed user requirements after the initial assessment

The user selected fresh native sessions on every switch, including switching back.
The same displayed Del chat retains all existing messages, reasoning disclosures,
tool details and attachments. A compact switch row follows the peer-exchange visual
style; provider controls update to the new active provider. There is no requirement
to resume a dormant native session.

The user also specified attaching context separately from their next prompt.
The initial recommendation below to concatenate context with the next message is
superseded. Setup must not send a fabricated user message or trigger inference.
Attach supported context at the provider/session layer and include a manifest of
the original conversation and all provider segments. Durable context-read tools
must resolve those references after the original processes stop and after restart.
Full displayed reasoning remains preserved; peer reads retain their existing
public-context/private-reasoning boundary.

Verified attachment mechanisms, distinct from live end-to-end verification:

- Codex: `thread/inject_items` persists model-visible history without starting a
  turn. Present in Del's checked-in 0.162.1 schema as `ThreadInjectItemsParams`.
  [Official API documentation](https://learn.chatgpt.com/docs/app-server#inject-items-into-a-thread).
- Claude Code: `--append-system-prompt-file` attaches additional session context
  while retaining default provider instructions. Resumed system-prompt snapshot
  behavior and installed-version support need integration coverage.
  [Official CLI reference](https://code.claude.com/docs/en/cli-reference#system-prompt-flags).
- OpenCode 2.x: durable session instruction entries are already used by Del at
  `crates/opencode/src/lib.rs:580`; the current API also documents synthetic input
  with `resume: false`. The installed version is 2.1.4; newer docs alone do not
  prove every endpoint or request shape on that version. Validate the chosen
  mechanism and restart behavior. Do not use the V1 `noReply` API by assumption.
  [Instructions](https://dev.opencode.ai/v2/docs/instructions/),
  [API reference](https://dev.opencode.ai/v2/docs/api).

Historical text remains attributed reference data even when a provider's
session-instruction channel carries it. The budget limits loaded model context,
not retained UI history or accessible earlier public records. Unsupported attachment
must leave the source usable rather than silently concatenate the next prompt.

These clarifications are recorded in the [current behavior rules](../neverfuckingchange.md).
The research pass itself preceded application-code implementation.

## Open-source reference: Agent Orchestrator

Verified repository: [OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator), inspected at commit `b824bc8867baf5805820530b5b7154d3b84bb24a`. Sources were fetched read-only through the GitHub connector; no third-party code, install scripts, or repository instructions were executed. The repository uses the [Apache 2.0 license](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/LICENSE).

**It implements actual same-chat provider switching.** This is separate from its Terminal↔Chat interface handoff. `SwitchProvider` keeps the canonical AO conversation and session, starts a new native provider conversation, and publishes a new provider branch. The same frontend route stays open and receives a switch marker. [Backend switch](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L43), [frontend behavior test](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/frontend/e2e/chat-provider-switch.spec.ts#L185).

### State and recovery mechanics worth copying

1. Serialize switches per session. Switching plus its first message holds the same switch lock; a retry checks the message's durable client ID so it cannot switch an already-delivered message back onto an older provider. [Switch/send locking and deduplication](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L55).
2. Admit only a ready, idle source with no pending provider interaction; validate target support before teardown. Then fence source intake. [Admission and fencing](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L153-L235).
3. Retain the workspace, clear the source native conversation ID, and start the target with target model/effort settings. Do not pass a Claude conversation ID to Codex or OpenCode. [Target launch configuration](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L278-L292).
4. Keep the old branch as the durable owner until target startup succeeds. Commit active branch, real native provider handle, controller generation, settings, and switch marker in one database transaction. Re-read session metadata so a concurrent rename/pin/browser credential update is retained. [Commit preparation](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L293-L340), [atomic commit](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/storage/sqlite/store/conversation_store.go#L583-L621).
5. Restore the source controller when target startup or publication fails. A restored branch keeps its original handoff context. [Rollback](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L257-L271), [failure tests](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch_test.go#L139), [publication-failure test](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch_test.go#L310).
6. Persist handoff context with the branch and reapply it when the provider restarts. A newly switched target that has never taken a turn may not yet have a durable native transcript; recovery explicitly allows fresh startup with the saved handoff. [Launch/restart context](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/service.go#L805-L856), [unanswered-branch recovery](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/service.go#L2348-L2368), [restart regression test](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch_test.go#L435-L485).

### What context actually crosses

AO does **not** transplant native hidden state. It creates a deterministic, bounded handoff note from its canonical recorded exchanges, original task, workspace/branch identity, changed-file names, and artifact paths. The note has a 32,000-character cap on non-Windows systems and a separate byte cap. It prioritizes the latest exchange, five previous exchanges, and up to fifteen older short entries; clipping is disclosed and a paginated history command exposes earlier recorded exchanges. No model call is required to summarize. [Note builder and budgets](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/domain/handoff_note.go#L12-L254).

Only completed turns with a user request and final assistant response become chat handoff exchanges; tool calls are not carried by this projection. Changed-file names supplement the exchange context. Consequently, AO demonstrates useful continuity, not perfect transfer of every fact, attachment, tool result, or provider-private state. [Projection](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/handoff_note.go#L49-L123), [workspace evidence](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/handoff_note.go#L197-L229).

Every switch clears the target native ID and creates a fresh branch, including switching back. It does **not** demonstrate Del's proposed dormant-native-session resume plus incremental context delta. AO marks the branch strategy `approximate_context` and records a replay cutoff/truncation flag. [Fresh native target](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch.go#L278-L287), [branch metadata](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/service.go#L999-L1014).

### What the tests do and do not prove

Inspected source includes tests for canonical context/identity retention, failure rollback, publication failure, serial switching through Claude/OpenCode, restart before the target's first turn, target settings, and retrying a delivered message after another switch. The frontend test switches Codex→Claude→Codex and checks a refused switch preserves the draft and prior provider. These use fake drivers/mocked HTTP; they are useful test designs, not evidence of a live six-direction Del integration matrix. None of AO's tests were run during this review. [Backend test suite](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/service/chat/provider_switch_test.go), [frontend test suite](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/frontend/e2e/chat-provider-switch.spec.ts).

AO's adapters differ from Del: it uses Codex app-server, Claude ACP, and OpenCode ACP adapters (including a separate OpenCode 2 adapter). Its provider-independent switch is relevant; its exact provider injection/acknowledgement behavior cannot establish that Del's native stream-JSON Claude and HTTP OpenCode paths work. [Driver registry](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/adapters/chatdriver/registry/registry.go#L97-L101), [OpenCode 2 driver](https://github.com/OrchestratorInc/agent-orchestrator/blob/b824bc8867baf5805820530b5b7154d3b84bb24a/backend/internal/adapters/chatdriver/opencodev2acp/driver.go).

A historical AO report describes a Claude→OpenCode switch that ran successfully until restart, when the reserved branch handle differed from the actual native session handle and resume failed permanently. This is a reported failure in an earlier implementation, not a claim that inspected main still has it. It explains why publishing the real provider handle and crash/restart tests matter. [Issue 6606](https://github.com/OrchestratorInc/agent-orchestrator/issues/6606).

### Comparison for a bounded Del implementation

| Reference mechanism | Recommendation for Del | Reason |
| --- | --- | --- |
| Stable conversation/session with provider branches | Copy the identity separation | Existing session-owned drafts, tabs, peer routes and browser state must survive native provider ID changes. |
| Idle-only, interaction-free admission | Copy for first release | Avoid interrupting tools or silently abandoning approvals; offer Stop and wait through existing controls. |
| Fresh native conversation per switch | Prefer as the first supported design | AO implements it; dormant resume plus exact deltas is an additional state/recovery feature. |
| Atomic publish and source restoration | Copy the contract using Del's persistence model | Avoid partial ownership/settings/history updates and broken restart recovery. |
| Bounded handoff with readable earlier history | Copy, strengthen event/attachment coverage where needed | The new provider cannot inherit private native state; visible history and delivered context are different guarantees. |
| AO's provider-driver plumbing | Do not copy directly | Del uses different native Claude and OpenCode transports. |
| Large orchestration/runtime machinery | Do not add for this feature | The evidence supports extending existing Del sessions and fork/context code, not replacing the harness. |

## Del assessment at 4cf5090

This is a feasibility assessment, not an implemented feature. Same-chat switching is feasible for Codex, Claude Code and OpenCode through their existing adapters. The missing work is reliable conversation identity, history and ownership across a switch; no cross-provider native transcript import is required.

### Existing implementation and limits

| Area | Evidence | Implication |
| --- | --- | --- |
| Main provider picker | [state.rs:2220](/Users/nihal/code/del/crates/app/src/state.rs:2220), [fork_with:1652](/Users/nihal/code/del/crates/app/src/state.rs:1652) | Same-provider account changes call switch_account; another provider opens an empty draft. |
| Same-provider transfer | [manager.rs:1056](/Users/nihal/code/del/crates/agent/src/manager.rs:1056) | Serializes against sends, checks idle, preserves the native ID, and reconnects the original account on failure. Preserve this behavior. Native IDs cannot be passed to a different provider. |
| Cross-provider foundation | [forks.rs:259](/Users/nihal/code/del/crates/agent/src/manager/forks.rs:259), [forks.rs:294](/Users/nihal/code/del/crates/agent/src/manager/forks.rs:294) | A target adapter can start with quoted historical context; reconfigure_fork retains displayed history but currently only allows independent forks. |
| Side-chat lifecycle | [side_chat.rs:324](/Users/nihal/code/del/crates/app/src/side_chat.rs:324), [side_chat.rs:472](/Users/nihal/code/del/crates/app/src/side_chat.rs:472) | Replacement is adopted before the old fork is deleted. It is not a dormant-provider session model suitable for blindly copying into main chats. Side chats have their own stable storage ID. |
| Main identity | [index.rs:13](/Users/nihal/code/del/crates/agent/src/index.rs:13), [manager.rs:687](/Users/nihal/code/del/crates/agent/src/manager.rs:687) | A main SessionRecord currently identifies one native provider session. It has no active-segment list or cross-provider canonical transcript. |
| UI persistence | [followups.rs:21](/Users/nihal/code/del/crates/app/src/followups.rs:21), [main.rs:1510](/Users/nihal/code/del/crates/app/src/main.rs:1510), [reply_times.rs:61](/Users/nihal/code/del/crates/app/src/chat/reply_times.rs:61), [conversation_search.rs:36](/Users/nihal/code/del/crates/app/src/conversation_search.rs:36) | Queue storage, workspace storage, reply timestamps and search assume the existing session/provider identity. A naive replacement would strand or misattribute state. |
| Handoff payload | [forks.rs:60](/Users/nihal/code/del/crates/agent/src/manager/forks.rs:60) | Transfers user/assistant text, peer messages and completed tool-output summaries. It excludes reasoning and pending approvals, but does not explicitly retain attachment payloads, tool-call inputs or FileChange events. Restored provider text may contain attachment references; that is not a durable attachment transfer contract. |
| Bounds and refresh | [forks.rs:104](/Users/nihal/code/del/crates/agent/src/manager/forks.rs:104), [forks.rs:485](/Users/nihal/code/del/crates/agent/src/manager/forks.rs:485) | Current bound keeps the last 100,000 bytes at a Unicode boundary, potentially cutting a serialized record. Refresh sends a complete bounded parent snapshot, not a delta. The switch-back delta promised by the spec does not exist yet. |
| Delivery acknowledgement | [manager.rs:1005](/Users/nihal/code/del/crates/agent/src/manager.rs:1005) | Pending fork context is cleared after an ordinary prompt is accepted; provider command paths bypass that wrapper. First-message commands/skills need explicit switching coverage. |
| Peer/browser scope | [peers.rs:1](/Users/nihal/code/del/crates/agent/src/peers.rs:1), [mcp_bridge.rs:251](/Users/nihal/code/del/crates/app/src/mcp_bridge.rs:251) | Peer IDs and credentials must resolve to the stable chat and active provider. An old process must not act after replacement or revoke the new process's access on late cleanup. |

### All three provider paths

- **Codex:** native thread/start and thread/resume; same-provider thread/fork is optional optimization. Cross-provider history goes through ordinary input. [backend.rs:733](/Users/nihal/code/del/crates/codex/src/backend.rs:733).
- **Claude Code:** persistent session creation, streamed user input and native --resume are present. Temporary side chats explicitly disable persistence, so they are not the right default for durable main segments. [backend.rs:657](/Users/nihal/code/del/crates/claude/src/backend.rs:657).
- **OpenCode:** native session creation, normal structured prompts and paginated history/resume are present; each chat owns its server and MCP identity. [lib.rs:679](/Users/nihal/code/del/crates/opencode/src/lib.rs:679).
- **Capability differences:** account/model/effort/fast values must be resolved for the target. OpenCode has no native goal support in this adapter; active native goals cannot be transparently transferred. [OpenCode adapter notes](/Users/nihal/code/del/crates/opencode/README.md).

### Recommended scope and transaction

Keep the user experience small: pick another provider in the existing composer; keep the same tab, title, project/worktree and transcript; insert a small attributed switch marker. Same-provider account switching continues to work. Do not bundle Ask, terminal-mode switching or the other roadmap features into this change.

Use one stable Del chat identity with separately recorded provider sessions/segments. Existing chats can keep their current identifier as that stable identity, avoiding unnecessary rewrites of unrelated UI storage; the original native ID becomes explicit in the first segment. The exact schema and migration require implementation review.

1. Under the same conversation lock used for sends, check the target account/provider and require a settled source. Pending approvals/questions, in-flight deliveries and native goal continuations need explicit handling. For the first version, block the switch until those settle; do not silently discard them.
2. Fence automatic follow-ups and peer continuations during preparation. Retain the draft, attachment references and queue under the stable chat identity. Revalidate provider-specific skills/commands before dispatching them to a new provider.
3. Persist an intent describing old/new segment, target configuration and bounded context through a known conversation sequence. Build context deterministically from whole public records with explicit coverage/omission metadata; do not invoke an extra summarizing model by default.
4. Stop/fence the outgoing execution and start a persistent target session in the same working directory. Del's permission contract remains authoritative; fail explicitly for unsupported settings rather than broadening permissions.
5. Commit the active segment and provider configuration durably only after target startup succeeds. Del uses atomic JSON files rather than AO's SQLite transaction: it needs a single authoritative commit record plus recoverable derived updates, not a sequence of unrelated best-effort file writes.
6. Route UI events, MCP credentials, peer replies, browser ownership, search and queues through the stable chat. Fence stale events by execution generation. Source cleanup must not delete the stable chat or its resources.
7. Attach context to the next accepted input once. A crash or uncertain acknowledgement must not silently replay a tool-running prompt. Failed target setup retains/restores the original provider; a failed restore remains visible with original history intact.
8. Restart reconstructs one chat with all segments and its active provider, including a switch made before the first target prompt.

These are deterministic lifecycle/storage responsibilities. The model performs the requested coding task using supplied context. No additional autonomous routing or inference-based completion detector is needed.

### Comparison and component decisions

| Component | Decision | Reason |
| --- | --- | --- |
| Stable Del chat with provider segments | KEEP | AO's canonical conversation shows the mechanism; Del's ID-keyed queue/workspace/search makes an explicit stable identity necessary. |
| Existing provider adapters and bounded context path | KEEP, extend | All three expose start/send/resume. Reuse those contracts and improve transfer metadata rather than translating native databases. |
| Atomic activation and recovery intent | KEEP | Target failure, disk failure and restart must leave a reconstructable owner; AO commits branch/settings/activity together. |
| New model-generated handoff summary service | KILL for this scope | Neither provider selection nor deterministic transfer requires another inference call. Explicit bounded public context is sufficient for the initial feature. |
| Reusing old native provider session on switch-back | KILL for this scope | The user selected a fresh native session every time, including switching back. |

**Confirmed direction:** fresh native segment per switch, while keeping the entire visible Del chat and original native histories. The user explicitly selected this behavior after the initial assessment. The current behavior rules preserve it.

The user-visible promise is conversation continuity. Provider-private reasoning, hidden compaction state, live approvals and internal tool execution state are not portable. Bounded model context may omit older content while the full saved chat remains visible. Attachment availability and any omissions must be represented honestly.

The user's explicit request for same-chat switching superseded the old behavior rule requiring a new draft. The implemented behavior is documented in the [README](../../README.md#models-and-reasoning) and [behavior rules](../neverfuckingchange.md).

### Required verification before shipping

- All six directed transitions: Codex→Claude, Claude→Codex, Codex→OpenCode, OpenCode→Codex, Claude→OpenCode, OpenCode→Claude; plus three-provider chains and switching back.
- Each receives a known fact from before the switch, the current working directory and file changes; replies remain attributed correctly. Test old and new attachments, long Unicode histories and omitted context.
- Same-provider account switch before and after cross-provider switching; target defaults and explicit account/model selections; no fallback to another account on failure.
- Streaming/approval/question races, queued follow-ups and steering, peer messages arriving during handoff, double clicks and stale events.
- Missing CLI/account, expired login, target startup timeout, storage failure, lost acknowledgement, crash before/after commit and restart before the first target message.
- Browser cookies/tabs, file drafts, side chats, pins, Space membership, title, unread state, reply times and search survive. No Git checkout/worktree mutation is introduced.
- Existing chats migrate without loss; archive/unload/delete remain correct across all segments.
- Tests at the provider-neutral layer, scripted adapter integration, then isolated real-provider smoke tests for all six directions, and native light/dark UI validation.

### Executed evidence

On 2026-11-01, in /Users/nihal/code/del at 4cf5090:

```text
cargo test --locked -p del-agent manager::forks::tests -- --nocapture
8 passed; 0 failed; 0 ignored; 61 filtered out
```

The passing tests cover existing fork context, snapshot restoration, pending context persistence, side-account failure preservation and peer rebind behavior. They use test backends and do not establish that the proposed main-chat switching feature works. No live inference, app rebuild, full-suite run or AO test execution was performed during this assessment. Application source was not changed.
