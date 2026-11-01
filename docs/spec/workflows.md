# Workflows

## Problem

Users repeat the same glue by hand:

- when an agent finishes, run the tests;
- if they fail, send them back;
- have the other model review;
- open a PR;
- ping me.

They want that automated. They don't want long autonomous chains they can't
see. Gas Town-style swarms ("100% vibe coded", agents fixing the same bug twice)
and "spawn 20 agents" marketing are what to avoid. Codex has scheduled
automations and Claude Code has hooks and dynamic workflows, but each works only
inside its own provider. A neutral host can wire them together.

## Goal

Small, visible, event-triggered pipelines built from things Del already does.
Agents can **draft** them from plain language, and only the user can
**enable** them.

## Example

User to a Claude chat: "From now on, whenever any chat in this project
finishes, have Codex review the diff and send its comments back."

Claude calls `create_workflow`. Del writes `.del/workflows/cross-review.toml`:

```toml
name = "Cross-review on finish"
on = "session.finished"          # with changes
[[step]]
review = { provider = "codex", target = "diff" }
[[step]]
send_back = "comments"           # into the session that finished
[[step]]
wait_for = "me"
```

Del shows the draft as a card with a plain-language summary and **Enable**.
Nothing runs until the user enables it.

## Triggers

| Trigger | Fires when |
| --- | --- |
| `session.finished` | A session reaches `done_unreviewed` ([agent state](agent-state.md)), optionally only with changes |
| `session.failed` | A session reaches `failed` |
| `schedule` | Cron-style time, local only, while Del is running (missed runs are reported, not replayed blindly) |
| `pr.opened`, `pr.comment` | GitHub events for the project's remote, polled through the user's `gh` login |
| `manual` | User runs it from the command menu |

## Steps

| Step | Does |
| --- | --- |
| `prompt` | Send a prompt to an existing session or a new one (`provider`, `model`, `worktree = true`) |
| `review` | Review a diff with a provider (Codex `review/start` where available) |
| `run` | Run a command or a project run config ([run](run.md)) and capture the result |
| `send_back` | Deliver output or comments to the triggering session as attributed reference data |
| `if` | Branch on the previous step's result (exit code, review verdict) |
| `retry` | Repeat a sub-sequence up to a hard limit |
| `notify` | Notify the user (counts as a needs-you item, see [attention](attention.md)) |
| `wait_for = "me"` | Pause until the user approves, replies or rejects |
| `pr` | Open or update a PR through `gh` |

Steps reuse existing machinery: `spawn_session`, peer requests with receipts,
`session_work`, terminal sessions, `notify`. Workflows don't add a second agent
runtime.

## Safety rules

- **Agents draft, users enable.** `create_workflow` and `update_workflow` only
  write disabled drafts. Enabling and any change to an enabled workflow's
  triggers, commands or providers need the user's confirmation, with a diff of
  what changed. This blocks a prompt-injected agent from installing persistent
  automation.
- **Workflow files in a repo are untrusted until the user enables them on this
  machine.** Cloning a repo with `.del/workflows/` never runs anything.
- **Hard limits**, which a workflow can lower but never raise:
  - max steps per run,
  - max retries (default 3),
  - max concurrent runs per workflow (default 1),
  - max sessions spawned per run,
  - a trigger deduplication window.
  A workflow can't trigger itself through its own sessions' completion.
- **Visible runs.** Every session a workflow starts appears in the sidebar under
  its project, labelled with the workflow name. A run log shows each step,
  input, output and duration.
- **Kill switch:** pause one workflow, or all workflows, from the command menu.
- Steps run with the same permissions the session would have if the user
  started it. Workflows never grant more.

## UI

- A **Workflows** list per project: name, trigger, enabled state, last run
  result. Opening one shows the TOML with its plain-language summary and run
  history.
- **Three presets** instead of an empty builder:
  1. **Build → test → cross-review** on `session.finished`.
  2. **Checks on finish**: run the project's test command and send failures
     back, up to 3 times.
  3. **Nightly fix**: on a schedule, run tests; if any fail, start a session to
     fix them in a new worktree and notify in the morning.
- No node-graph editor. Plain language plus TOML, edited by the user or an
  agent.

## Non-goals

- Running while Del is closed, or cloud execution.
- Arbitrary scripting languages inside workflows. `run` executes commands, and
  the workflow format stays declarative.
- Marketplace or sharing features.

## Open questions

- Whether `.del/workflows/` lives in the repo (shareable with a team) or in
  Del's data directory by default.
- GitHub trigger polling interval and how to show "watching PRs" calmly.
- How `wait_for = "me"` items appear when several workflows wait at once
  (likely grouped in the Needs-you queue).

## Verification

- `create_workflow` from an agent produces a disabled draft. Nothing triggers
  until the user enables it.
- An agent editing an enabled workflow's command disables it until the user
  confirms the change.
- The retry limit stops a fail → fix → fail loop at 3 and raises a needs-you
  item.
- A workflow-spawned session can't re-trigger the same workflow.
- A cloned repo with workflow files runs nothing until enabled locally.
- Every run's sessions are visible and its log matches its steps.
