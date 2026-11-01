# Agent state

## Problem

Every attention feature depends on one question: is this agent working, waiting
on me, done, or broken? Tools in this space that scrape terminals get it wrong.
The Agent Navigator author found "permission prompts in Claude Code are UI
elements with no corresponding log event", and the wish heard most often in
multiplexer communities is a reliable "needs input" signal. A false "needs you"
teaches users to ignore the queue, which is worse than having no queue.

## Goal

One normalized, explainable state per session, chat or terminal, derived only
from structured signals. It feeds [attention](attention.md), [run](run.md) and
[workflows](workflows.md).

## States

| State | Meaning | Needs a person |
| --- | --- | --- |
| `working` | A turn or tool is running | No |
| `waiting_peer` | Blocked on an unanswered peer request (`session_work`) | No |
| `needs_input` | Question asked (`ask_user`, provider question tool) or the agent stopped and asked the user something | Yes |
| `needs_approval` | Permission request pending (`ApprovalRequested` without `ApprovalResolved`) | Yes |
| `done_unreviewed` | Work finished with changes the user hasn't opened since | Yes, low urgency |
| `failed` | Turn failed, process exited non-zero, or unrecoverable error | Yes |
| `idle` | Nothing running and nothing waiting | No |

Each state carries **why** (the source signal) and **since** (a timestamp), so
the UI can say "Needs approval · 4m · rm outside worktree" and the user can
check why Del thinks so.

## Sources, in order of trust

1. **Normalized provider events** (`AgentEvent` in `crates/agent/src/types.rs`):
   `TurnStarted` and `TurnCompleted` with their status, `ApprovalRequested` and
   `ApprovalResolved`, `ToolCallStarted` and `ToolCallFinished`, `Error`,
   `PeerWorkChanged`, `FileChange`.
2. **Del MCP calls**: `ask_user` (needs input until answered), `report_status`
   (`ReportedState` in `crates/mcp/src/host.rs`: working, idle, blocked, done,
   error), `session_work wait`.
3. **Provider hooks** for PTY sessions, such as Claude Code's Notification and
   Stop hooks, installed per Del-launched terminal, never into the user's global
   config without consent.
4. **Terminal escape sequences**: OSC 9, OSC 99, OSC 777 notifications, OSC 9;4
   progress, OSC 7501 status (idle, working, done, blocked, failed), and BEL as
   a weak hint only.
5. **Process state**: `TerminalHostStatus::Exited { code }`.

Screen scraping is not a source. If no structured signal exists, the state is
`working` or `idle` from process activity alone, and the UI never claims
`needs_input`.

## New normalized events

- **Plan progress.** Add an `AgentEvent::PlanUpdated { steps, current }`
  variant. Codex emits `turn/plan/updated` and `item/plan/delta`. Claude's
  todo-list tool calls map to the same shape. OpenCode maps its todo events where
  they exist. This drives "step 3/5" status lines.
- **Activity verb.** Derive a short present-tense line from the latest running
  tool, for example "running cargo test", "editing auth.rs", "browsing
  localhost:3000". Use `ToolCallStarted.input_summary`, truncated, without model
  calls.

## Rules

- State changes are computed in the session manager, not in views, and are
  persisted so restart restores `needs_*` and `done_unreviewed`.
- `done_unreviewed` clears when the user opens the session or its Done card, and
  follows the same rules as unread (see "Unread conversations").
- A peer request never sets `needs_input`. Agents talking to agents is not the
  user's problem until someone asks the user.
- Conflicting signals resolve in favor of the most specific, newest structured
  source. A pending approval outranks a `report_status working`.

## Verification

- Replay fixtures per provider: approval requested then resolved elsewhere →
  `needs_approval` then `working`, with no stale badge.
- `ask_user` pending across restart stays `needs_input`.
- A PTY Claude session with hooks enabled reaches `needs_input` on a permission
  prompt. Without hooks it never falsely claims `needs_input`.
- OSC 7501 values map one-to-one onto the state table.
- False-positive budget: zero `needs_*` states without a recorded source signal.
