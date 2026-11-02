# Attention: replacing kanban

## Thesis

Kanban is a planning metaphor: a person moves cards through stages they defined.
Agents move themselves, so a board describes work instead of doing it, its
columns lag behind, and the user ends up tending the board. Vibe Kanban retired
its board in April 2026. Armin Ronacher and Peter Steinberger dropped their own
agent managers, and boards in Devin Desktop and Nimbalyst get a lukewarm
reception.

The part of kanban that works is seeing every agent's **state** at a glance
(Agent Navigator: "terminal tabs are sequential; you have to visit each one to
learn anything"). The part that's broken is the **workflow metaphor**. Del keeps
the glance and drops the board.

Del lets users ignore agents until one needs them, then gets them back up to
speed instantly.

## Psychology this rests on

- **Situation awareness (Endsley).** Supervision needs three levels:
  - perception: what it's doing;
  - comprehension: what that means;
  - projection: what happens next.
  Each layer below targets at least one of them.
- **Interruption and resumption cost.** Getting back into an interrupted task
  takes a long time (Mark et al.), and attention residue lingers after a switch
  (Leroy). Switching between agents is an interruption, so the cost to cut is
  getting back up to speed.
- **Batching beats pinging.** Batched notifications improve attention and mood
  (Fitz et al., 2019). Alarm fatigue sets in once most alerts don't matter.
- **Calm technology (Weiser & Brown).** Information sits in the periphery and
  moves to the centre only when it needs the user.
- **Open loops (Zeigarnik).** Unfinished items nag. An empty queue closes them.
  Constant checking is the slot-machine loop users describe with multi-agent
  setups.
- **Automation complacency (Parasuraman & Manzey).** People watching automation
  drift out of the loop. Exceptions must stand out, and review needs evidence,
  not a checkmark.
- **Real-world ceiling.** Most people manage about 2–4 parallel agents. Simon
  Willison is "wiped out by 11am" running four. The design goal is fewer
  interruptions, not more agents.

## Layer 1: periphery (always visible, costs nothing)

- **Sidebar order never changes because of state.** Manual and pinned order
  stays exactly as `neverfuckingchange.md` requires.
- **The trailing status slot gains a `needs you` indicator**, with a fixed
  priority: needs you > working > unread. It remains one slot with one
  indicator, never two side by side. The needs-you mark is visually distinct
  from the blue unread dot and the working ring, and it follows the same hover
  rules (it hides to reveal actions).
- **An optional quiet second line** on running rows comes from
  [agent state](agent-state.md): "running cargo test · 2m", "step 3/5 ·
  migrate schema", "waiting on Codex peer". It's off by default, behind a
  setting, and must keep the existing title fade and spacing rules.
- **Top tabs** show the same state indicator as their sidebar rows.

## Layer 2: exceptions (the Needs-you queue)

- **One shortcut from anywhere** opens the queue as a popover, not a page. It
  lists only sessions in `needs_input`, `needs_approval`, `failed` or
  `done_unreviewed`.
- **Ordering by cost of waiting:**
  1. Sessions that block other peers, so one answer frees several agents.
  2. `needs_approval` and `needs_input`, oldest first.
  3. `failed`.
  4. `done_unreviewed`, oldest first.
- **Each row** shows the session title, project and branch, the state with its
  reason and how long it has waited, and the agent's actual question or
  approval text.
- **Keyboard triage:** next and previous, reply inline, approve or deny inline
  (same semantics as the chat's approval UI), open the session, snooze (15
  minutes or "until I'm back").
- **Empty state:** "Nothing needs you." No counts or badges grow while the queue
  is empty, and the toolbar entry is quiet.
- **Notifications:**
  - Only fire for transitions *into* a `needs_*` or `failed` state.
  - Never fire for progress or for `done_unreviewed`.
  - Bursts within a short window collapse into one ("3 agents need you").
  - Respect focus: nothing fires for the session currently in view.

## Layer 3: resumption (when the user opens a session)

### "Since you last looked"

- A compact recap at the point where the user stopped reading:
  - files touched (count plus top paths),
  - commands run and test results,
  - peers messaged or answered,
  - goal or plan progress,
  - decisions the agent made, taken from its final messages.
- It's built from normalized events. Prose summaries are optional, run on the
  session's own provider, and are clearly labelled.
- It dismisses itself once the user scrolls past it and is never repeated for
  the same span.

### Done card

When a session reaches `done_unreviewed`, its latest turn ends with a card
showing:

- A summary and diff stats (files, +/−).
- **Risk flags** computed from events, not opinions:
  - deleted files,
  - config or lockfile changes,
  - migrations,
  - commands touching paths outside the worktree,
  - force-push or history rewrite,
  - credentials files.
- Evidence: tests run with pass/fail, browser screenshots the agent captured.
- Actions:
  - **Run** (see [run](run.md)),
  - **Review with <other provider>** (see [existing provider controls](../../README.md#models-and-reasoning)),
  - open diff, open in editor, create PR.

## Overview lens (a glance, not a home screen)

- A shortcut toggles a grid of all sessions in the current Space, grouped by
  state: Needs you / Working / Finished, not reviewed / Idle.
- It's **sorted automatically, with no dragging and no user-defined columns.**
  Nothing in it needs maintaining.
- Each tile shows title, provider, branch, state reason, the activity line and
  plan progress.
- The **delegation tree** appears as a toggle: which session spawned or asked
  which, from peer receipts.
- Escape returns to exactly where the user was.

## Non-goals

- Kanban columns, cards to drag, or task-planning boards as the main surface.
- Productivity scores or "X% faster" claims. Show what agents did, not ratings.
- Gamified counters (streaks, agents-running totals).

## Open questions

- Whether `done_unreviewed` belongs in the queue or only in the Overview, given
  how often it occurs. Measure it in practice.
- Whether snooze persists across restarts.
- What the toolbar entry looks like within the toolbar rules.

## Verification

- With 6 sessions across 2 projects, sidebar order is unchanged through every
  state change.
- A needs-you transition in a background window fires exactly one notification.
  A burst of 3 collapses into one.
- Inline approve from the queue resolves the same approval ID as the chat UI and
  clears the state everywhere.
- The recap after 20 background turns lists accurate file, command and test
  counts drawn from events.
- Risk flags fire on fixtures for delete, config change and outside-worktree
  write, and never on a clean change.
