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

## Existing implementation

| Surface | Evidence | Reuse / change |
| --- | --- | --- |
| Cross-provider forks | `crates/agent/src/manager/forks.rs` | Native forks keep provider context. Other providers get a bounded, explicitly quoted transcript with their first question, and nothing is sent on creation. Reuse as-is for both actions. |
| Fork context refresh | `refresh_fork_context`, `pending_fork_context` | Reuse to send only the turns since the last switch when switching back. |
| Side chats | `crates/app/src/side_chat.rs` (`fork_with_settings`) | Ask can reuse the side-chat lifecycle. Its result renders inline in the main thread instead of a separate pane. |
| Main-chat provider change | `AppState::fork_with` | Replace "open blank draft" with Switch. A blank chat stays available from the new-session action. |
| Codex native fork | `thread/fork` in `crates/codex/src/backend.rs` | Used when Codex → Codex; the cross-provider path uses a transcript. |

## Switch

- From the composer's provider picker, choosing a different provider in an
  existing chat offers **Switch to <Provider>**. A chat with no turns still just
  changes the draft's provider.
- Del creates a transcript fork for the target provider in the **same working
  directory and worktree**, using that provider's default account, model and
  effort (never copying an incompatible model, as with `spawn_session`).
- **The tab and sidebar row stay the same.** On screen the conversation is one
  thread made of segments. Each segment is backed by its own provider session.
- A thin divider marks each switch and says what was carried over, for example
  "Switched to Codex · GPT-6.1 · got the last 40 turns". The divider expands to
  show what was **not** carried over: reasoning, pending approvals, tool state,
  and older turns past the limit.
- Every assistant message shows a small provider glyph so the source of each
  answer is always clear.
- The previous provider session goes **dormant**. It is not closed, and its
  history stays intact.
- **Switching back** to a provider used earlier in the thread resumes its native
  session and adds only a quoted summary of the turns since it was last active.
  No full replay.
- **Only one provider is active per thread.** A switch while a turn is running
  asks to stop or wait. Queued follow-ups move to the new active segment.
- Peers, goals and browser ownership move to the active segment. To other
  agents, the chat's peer identity stays stable.

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

- Default transcript limit per provider, and whether to summarize instead of
  truncating very long threads.
- Whether Ask answers count as unread in the sidebar when the user has left the
  chat.
- How terminal (PTY) sessions take part. A Switch could open a chat segment
  seeded from the terminal's conversation hint.

## Verification

- Claude → Codex → Claude in one tab: Codex gets a bounded quoted transcript;
  switching back resumes Claude's native session plus a delta only.
- Switching during a running turn asks the user and never drops queued messages.
- The Ask answer is attributed, read-only, and never auto-sent to the active
  provider.
- Peer requests addressed to the chat reach the active segment after a switch.
- Restart preserves segments, dividers and the active provider.
