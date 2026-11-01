# Run: testing a session's branch

## Problem

With several agents on the same project, the user has several branches and
worktrees in play at once. The hard part isn't creating worktrees. It's
answering "which branch am I actually looking at right now?" Users start a dev
server in the wrong folder, ports collide, a browser tab shows the old branch,
and feedback ends up in the wrong chat. Conductor ships per-workspace Run
actions with `$CONDUCTOR_PORT` and an experimental "Spotlight" mode that syncs
one workspace into the repo root. Its docs say nothing about telling branches
apart, and users report Spotlight as flaky.

## Goal

From any session, one action runs that session's branch. What is running, and
from which branch, is unmistakable on every surface, and feedback from the
running app goes back to the chat that built it.

## Run configuration

- Per project: `.del/run.toml` in the repository, or a Del-local override when
  the user doesn't want to commit it.

  ```toml
  [[run]]
  name = "web"
  command = "pnpm dev --port $DEL_PORT"
  url = "http://localhost:$DEL_PORT"
  ready = "Local:"          # optional output match, else wait for port open

  [setup]                   # runs once per new worktree
  copy = [".env", ".env.local"]
  command = "pnpm install"
  ```

- **Detection** when no config exists:
  - `package.json` scripts (`dev`, `start`),
  - `Cargo.toml` (`cargo run`),
  - `Makefile` targets,
  - `Procfile`,
  - common framework defaults.
  The proposal is shown for one-click confirmation, never run silently.
- Agents can write or update the config with a Del MCP tool such as
  `propose_run_config`. The user confirms the first version and any changed
  command.
- `setup` runs when Del creates a worktree, before the agent's first turn. This
  covers the friction that made users give up on worktrees.

## Modes

### Isolated (default)

- **Run** executes the command in the session's own working directory or
  worktree, in a terminal tab owned by that session. `DEL_PORT` is allocated per
  session and kept across restarts when free. `DEL_SESSION`, `DEL_BRANCH` and
  `DEL_WORKTREE` are exported.
- **One live slot by default.** Starting Run for a session stops the run of the
  previously live session in the same project, after confirmation if it has
  unsaved state. One app running and clearly labelled means nothing to mix up.
- **Parallel runs** are opt-in, per project. Each session keeps its own port,
  and every surface stays labelled.

### Spotlight (into my checkout)

- For projects that must run from the main checkout (fixed ports, native app
  bundles, heavy builds, or the user's own IDE and tooling).
- Del brings the session's tracked changes into the main checkout. It applies
  only if the main checkout is clean. Otherwise it refuses with the dirty
  paths, never stashing silently.
- One session at a time. Turning Spotlight off restores the main checkout's
  previous commit and leaves nothing behind. Edits the agent makes keep syncing
  one way while Spotlight is on.
- Follows "never discard Git work": refuse in the middle of a rebase or merge,
  and never touch untracked or ignored files the session didn't create.

## Branch identity

- Each session gets a **stable session colour** (from a small fixed palette,
  theme-aware), used consistently on:
  - its sidebar row and top tab while running,
  - its run terminal tab,
  - its browser tab chip ("feature/login · Fix auth").
- The browser pane shows a small chip in its own chrome with branch, session and
  port. It never draws over the page. The floating browser preview keeps its
  no-banner rule.
- When the browser's URL matches a session's `DEL_PORT`, the tab is bound to
  that session automatically. A page from a stopped run shows "stale: run
  stopped" in the chip instead of looking live.
- The live slot is visible from anywhere: a quiet title-bar indicator showing
  "Live: feature/login". Clicking it focuses that session's run.

## Feedback goes back to the right chat

- From a bound browser tab:
  - **Send to agent** with a note and screenshot,
  - **Send element**: HTML, computed CSS and screenshot of a clicked element.
  These go to the bound session as an attachment, never to whichever chat is
  in view.
- Run terminal output: select text, then **Send to agent**. Crashes during a run
  (non-zero exit, a matched error) offer a one-click "Send this error to <session>".
- **"How to test this" checklist:** at the end of a turn with changes, the agent
  may provide steps through a Del MCP tool. The Done card shows them as a
  checklist. Unchecked items with comments go back to the agent as one message.

## Non-goals

- Becoming a process manager or a Docker or compose orchestrator. Run starts the
  user's own commands.
- Hosted previews or cloud environments.
- Device and simulator management for mobile apps, beyond running the user's
  command.

## Open questions

- Whether `.del/run.toml` lives in the repo by default or in Del's data
  directory.
- How to handle services shared across branches (one database, many app
  servers). Possibly a `[[service]] shared = true` that the live slot never
  stops.
- Port range and conflicts with ports the user has fixed.

## Verification

- Two worktree sessions on one project: Run A then Run B stops A, keeps ports
  distinct, and labels every surface with the correct branch.
- Spotlight refuses on a dirty main checkout and restores the exact previous
  state when turned off, untracked files included.
- Send to agent from a browser tab bound to session A while viewing session B
  delivers to A.
- `setup` copies env files and installs dependencies before the first turn of a
  new worktree session.
- A run that crashes offers to send the error to its own session only.
