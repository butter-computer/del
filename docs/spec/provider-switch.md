# Provider switch and second opinions

## Problem

Users hold more than one agent subscription and move between models
constantly: "Claude to build, Codex to review" is the most common pattern. They
also jump to whichever model just shipped or has the better answer today. Today,
changing the provider in a main chat opens an empty draft in the same folder
(`AppState::fork_with`, `crates/app/src/state.rs`). All context is lost, so
users re-explain or copy and paste.

## Goal

Two actions inside any chat:

- **Switch.** The thread continues on another provider in the same tab, with
  context carried over.
- **Ask.** Get a one-off answer from another provider inline, without handing
  over the thread.

The current requested implementation is Switch across Codex, Claude Code, and
OpenCode. Ask remains a separate future feature. The behavior below incorporates
the user's 2026-11-01 clarification: one intact displayed chat, a compact switch
row, a fresh native session on every switch, and context attached separately from
the user's next message.

## Existing implementation

| Surface | Evidence | Reuse / change |
| --- | --- | --- |
| Cross-provider forks | `crates/agent/src/manager/forks.rs` | Reuse context projection concepts, not the current first-user-prompt wrapper or temporary fork lifecycle. Switching requires durable main-chat identity and separate provider context attachment. |
| Fork context refresh | `refresh_fork_context`, `pending_fork_context` | Currently refreshes a complete bounded snapshot, not a delta. Switching back starts a fresh native session and needs no dormant-session delta mechanism. |
| Side chats | `crates/app/src/side_chat.rs` (`fork_with_settings`) | Ask can reuse the side-chat lifecycle. Its result renders inline in the main thread instead of a separate pane. |
| Main-chat provider change | `AppState::select_account` → `SessionManager::switch_provider` | Switch in place. A blank chat stays available from the new-session action. |
| Stable identity and historical segments | `SessionRecord::segments`, `manager/switching.rs`, `conversation.rs` | Del retains one chat ID, commits the new native identity durably, and records full displayed history independently of provider storage. |
| Codex native fork | `thread/fork` in `crates/codex/src/backend.rs` | Used when Codex → Codex; the cross-provider path uses a transcript. |

## Switch

- From the composer's provider picker, choosing a different provider in an
  existing chat offers **Switch to <Provider>**. A chat with no turns still just
  changes the draft's provider.
- Del creates a fresh persistent native session for the target provider in the **same working
  directory and worktree**, using that provider's default account, model and
  effort (never copying an incompatible model, as with `spawn_session`).
- **The tab and sidebar row stay the same.** On screen the conversation is one
  thread made of segments. Each segment is backed by its own provider session.
- Preserve every existing displayed message, reasoning disclosure, tool detail,
  attachment and reply timestamp. New replies append below the switch. Context
  transfer limits must never remove or rewrite the displayed conversation.
- Mark the boundary with a compact centered exchange row matching peer exchanges:
  source and target provider logos/names and a switch label, such as
  "Switched from Claude to Codex". No large card, transcript dump, or new
  per-message glyph requirement. Keep transfer details available on inspection.
- Provider/account/model/effort/usage controls describe the new active provider.
- Retire the previous execution while preserving its native history and Del's
  recorded segment. Never delete the old chat records as switch cleanup.
- **Switching back also creates a fresh native session**, receiving context from
  the whole Del conversation across all prior provider segments. Do not resume
  an old provider session or require incremental catch-up semantics.
- **Only one provider is active per thread.** A switch while a turn is running
  asks to stop or wait. Queued follow-ups move to the new active segment.
- Peers and browser ownership retain the stable Del chat identity and route to
  the active execution. Handle native goals explicitly before switching; do not
  pretend a provider-specific goal can migrate to an unsupported provider.

## Context attachment and historical reads

- Attach context through the target provider's supported history or session-context
  mechanism during setup. Do not prepend a transcript to the user's next prompt,
  fabricate a user message, or start an inference turn merely to switch providers.
- Codex's pinned protocol supports `thread/inject_items`, which appends model-visible
  history without starting a turn. Claude supports appended session context through
  `--append-system-prompt-file`. OpenCode 2.x exposes durable session instruction
  entries; current API documentation also describes synthetic input with
  `resume: false`. Verify the installed version and acknowledgement/restart behavior
  before choosing the precise OpenCode mechanism. Do not use V1 API assumptions.
- Keep transcript data explicitly attributed and quoted as reference material,
  even when carried in a session instruction channel. Preserve normal provider
  instructions; never elevate historical tool output into new instructions.
- Attach a snapshot of transferable conversation content within the target's
  supported budget. Full displayed history remains in Del independently. Retain
  whole records, source identities and attachment references; disclose omitted
  coverage instead of silently slicing serialized text.
- Include the stable Del chat ID and an ordered manifest of the original session
  and every switch segment, with provider/session IDs and readable history references.
  Extend the existing authenticated context-read tools to page through saved public
  history across those segments, including when their original processes are stopped
  or Del has restarted. A bare dead native-session ID is not a usable reference.
- The agent can retrieve earlier public context on demand without waking the old
  agent, changing user selection, or marking another chat read. Keep existing project
  scope checks. Displayed reasoning remains in the UI; historical reads continue to
  exclude private reasoning according to the peer contract.
- Save context attachment state before activation. Restore it without duplication
  after restart; an uncertain attachment acknowledgement needs reconciliation.
  If the installed provider cannot attach the context, preserve the source session
  and report the failure instead of silently reverting to prompt concatenation.

## Chat and Terminal

`/terminal` grants exclusive ownership of the active native segment, using its
current provider/account/settings rather than the stable Del chat ID as a native
resume ID. Claude's separately attached context is included in the terminal launch;
a session without a first user turn uses `--session-id` instead of `--resume`.

For switched chats, a private durable checkpoint saves the native transcript and
the boundary in Del's journal before launching the TUI. Side chats and
`read_session` combine earlier provider history with new persisted terminal events
without taking ownership. `/chat` appends those new events, preserving the existing
UI history exactly. Return is retry-safe after a partial journal commit or restart.
If the native transcript rewrites the saved prefix, return fails explicitly and
keeps the original journal rather than guessing a merge. Provider/account switches
remain blocked while the terminal owns the conversation.

## Ask

- A composer action such as `Ask Codex` (or a slash command) sends the user's
  question, plus a quoted bounded transcript and the current diff when relevant,
  to a temporary session on another provider.
- The answer renders **inline** as a distinct, attributed block in the main
  thread ("Codex says…"), collapsed by default after it's read.
- The asked provider gets **read-only intent**: its prompt says it is giving an
  opinion, not editing. If the provider supports it, the session runs with a
  read-only or plan permission mode.
- From the answer block:
  - **Send to <current provider>** quotes the answer into the active thread as
    reference data, never as a user instruction.
  - **Switch to <Provider>** promotes the Ask into a full Switch.
- A preset, **Review this diff with <other provider>**, uses Codex `review/start`
  when the target is Codex. For other targets it uses a review prompt over the
  diff. Results appear as review comments the active agent can act on.

## Rules

- Carried context is quoted reference material. It never grants permissions or
  counts as a user instruction (same as fork snapshots and peer messages).
- Never send credentials or live approvals across providers (already a
  `ForkSnapshot` invariant).
- The transcript limit and what it includes are shown, not hidden.
- A failed start on the target provider leaves the thread on the current
  provider with an error. It never silently falls back to a third provider.

## Non-goals

- Running two providers on the same worktree at the same time.
- Automatic routing ("pick the best model for me"). The user picks.
- Merging two providers' transcripts into one native session.

## Open questions

- Exact initial-context budgets and provider version requirements. Earlier public
  history remains retrievable through the segment manifest and context-read tools.
- Whether Ask answers count as unread in the sidebar when the user has left the
  chat.

## Verification

The native acceptance test is `crates/app/tests/provider_switch.rs`. It runs all
six directed transitions using installed providers and the production MCP bridge,
closes and resumes each target before its first user turn, then makes the target
retrieve the original fact through `read_session`. It verifies that switching
itself starts no inference. Run explicitly with:

```sh
cargo test --locked -p del-app --test provider_switch -- --ignored --nocapture
```

Manager regression tests also cover 24 repeated switches, 60,000-event history,
restart before first target input, account transfer after a provider switch,
queued input during setup, renaming during setup, busy/approval rejection,
context failure, and rollback after a disk commit failure. Journal tests cover
whole-record Unicode bounds, private-reasoning exclusion and partial-write recovery.
Native UI checks cover the compact rows, preserved work disclosures, provider
controls, light/dark appearance, and restart.

- All six directed transitions among Codex, Claude and OpenCode, including
  Claude → Codex → OpenCode → Claude: each switch gets a distinct native session,
  while one Del chat retains its complete displayed history and tool state.
- Context attachment starts no model turn and leaves the next user input unchanged.
- Retrieve an older public fact omitted from initial context through the original
  session/segment references after multiple switches and after restart.
- Reasoning disclosures, tool details, attachment references and timestamps from
  earlier providers remain visible; the active controls show the target provider.
- Switching during a running turn asks the user and never drops queued messages.
- The Ask answer is attributed, read-only, and never auto-sent to the active
  provider.
- Peer requests addressed to the chat reach the active segment after a switch.
- Restart preserves segments, switch rows, attached context and the active provider,
  including a switch made before sending any message to the target.

The combined real-provider regression runs a switch before the first target turn,
then two Terminal handoffs, cross-provider Side chats, and a fresh-manager restore:

```sh
DEL_MODE_TEST_SWITCH=1 DEL_MODE_TEST_PROVIDER=claude cargo test --locked -p del-app \
  real_provider_chat_terminal_chat_preserves_native_history -- --ignored --nocapture
```

Repeat for `codex` and `opencode`. The provider-switch acceptance test also asserts
that every target returns a nonempty real model catalog.

Validation after merging the Chat/Terminal feature: 605 workspace tests passed,
45 opt-in tests skipped, and Clippy with `-D warnings` passed. Live checks passed
for all six provider switches with nonempty model catalogs, plus two terminal
handoffs per provider with a sibling session kept live, all cross-provider Side
chats, refresh, and fresh-manager recovery. Native UI checks verified populated
Codex/Claude/OpenCode model menus, model selection, light/dark appearance, typed
`/terminal` and `/chat`, context recall, retained switch rows, and restart. The
macOS debug app was rebuilt, signed, and relaunched with real providers.
