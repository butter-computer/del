# Provider progress checklists

Del renders the provider’s structured progress in the conversation transcript,
using the same component for main and Side chats. The checklist appears only
when the provider supplies tasks. It is separate from the Plan permission mode.

Pending tasks have an open circle; the active task has a moving ring while its
own turn is running, and a static marker when the turn stops. Completed tasks
have a checkmark, and cancelled tasks have a dash. Task changes fade briefly and
completion morphs the circle into a checkmark. Reduced Motion disables the
spinner and transitions. Text wraps at Side chat widths. Checklists remain
visible when the surrounding completed tool work is collapsed.

A completed, failed, or interrupted response does **not** complete its tasks.
The provider must explicitly report that change. Empty snapshots clear the card.

## Provider contracts

- **Codex:** `turn/plan/updated` supplies an explanation and ordered steps with
  `pending`, `inProgress`, or `completed` status. Each turn owns its plan. Current
  Codex requires `tools.update_plan.enabled = true`; Del sets that override on
  its chat threads without changing the user’s config file.
- **Claude:** successful `TaskCreate` results supply the assigned task ID.
  `TaskUpdate` changes that task’s title, active label, or status, and `deleted`
  removes it. Legacy `TodoWrite` replaces the full list. Failed or declined
  tool calls never change progress; child-agent task lists do not overwrite the
  root chat. Del opts its CLI process into `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`.
  Live `tool_use_result` and native-history `toolUseResult` are both supported.
- **OpenCode v2:** successful `todowrite` calls supply ordered todos, including
  `cancelled`. Del reads native input/result metadata before summary truncation.
  It resolves configuration through the workspace server, preserves the effective
  experimental settings and any existing inline JSONC configuration, and enables
  `experimental.todo` only in the chat server’s process. Global files are untouched.

The shared `AgentEvent::PlanUpdated` carries snapshots or task-ID patches. The
existing conversation journal saves these events; replay reconstructs the same
checklist. Updates on a later turn create a current card from the prior state,
leaving earlier cards unchanged. Provider switches bound task IDs so unrelated task lists cannot merge.
Native Claude and OpenCode history readers also recover checklist events from
successful saved tool calls. Codex checklist updates captured by Del survive in
its journal; the app-server’s prose `plan` item is a different Plan Mode artifact.

## Sources inspected

- [Codex app-server notification contract](https://learn.chatgpt.com/docs/app-server)
- [OpenAI’s Codex tool opt-in](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs)
- [OpenAI’s public Codex checklist renderer](https://github.com/openai/codex/blob/main/codex-rs/tui/src/history_cell/plans.rs)
- [Claude task tracking and structured result correlation](https://code.claude.com/docs/en/agent-sdk/todo-tracking)
- [OpenCode v2.1.4 todo schema and feature gate](https://github.com/anomalyco/opencode/blob/v2.1.4/packages/core/src/tool/plugin/todo.ts)
- [OpenCode v2.1.4 tool-event schema](https://github.com/anomalyco/opencode/blob/v2.1.4/packages/schema/src/session-event.ts)
- [OpenCode v2.1.4 process configuration](https://github.com/anomalyco/opencode/blob/v2.1.4/packages/cli/src/server-process.ts)

The public Codex renderer supplied useful status and wrapping behavior. Del’s
native visual treatment follows the user’s Claude reference and the existing
Del theme; this is not a claim to have ChatGPT’s private application source.

## Verification

Provider parser tests cover typed status updates, failed mutations, replacement,
clearing, and native history. Sanitized Claude fixtures were recorded from the
installed CLI, including a saved conversation with an unfinished task. Reducer
and journal tests cover restart, turn completion, and provider isolation. A GPUI
test checks wrapping at 320-pixel Side width and removal of empty checklists.
The ignored native tests require installed authenticated providers and send a
small progress-only prompt; the OpenCode config test sends no model prompt.

For a scripted visual preview, run a separate data directory with
`DEL_BACKEND=mock`, set `DEL_MOCK_DELAY_MS`, and send `checklist`. It is explicitly
labelled as a preview and does not call a model.
