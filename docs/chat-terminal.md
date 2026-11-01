# Chat and Terminal handoff

Del's `/terminal` and `/chat` commands switch the same native conversation between
Chat and the provider's interactive terminal. Claude, Codex, and OpenCode are
supported. These are local commands: Del does not submit them as model prompts.

## User behavior

- Enter `/terminal` in an existing chat after its current response finishes or is
  stopped. Del closes the chat backend and resumes the exact native conversation.
- Enter `/chat` on an empty terminal prompt to close that terminal and reload its
  saved history in Chat. A **Return to chat** button provides the same action and
  remains usable if the terminal exits or returning to Chat fails.
- A conversation originally created in Terminal can also return to Chat once its
  native conversation ID is known. Its sidebar row and open tab become the chat;
  pins, Spaces, tab identity, and tool panes stay with it.
- In a new draft, the commands choose the first message's mode. Ordinary sidebar
  shells do not intercept `/chat`.
- Commands with arguments, multiline input, or ambiguous terminal cursor edits
  are ordinary provider input. Use the button if the terminal editor is not on
  a clear prompt.
- The handoff retains the saved provider, account, folder, model, effort, service
  tier, and permission mode. Native CLI changes are not imported into Del's saved
  settings. Codex's current TUI uses `on-request` with a read-only sandbox for
  Del's Ask mode; the saved Chat permission mode stays Ask.
- Temporary Side chats cannot become main terminals. Save/continue them as a
  regular conversation first. If a saved fork still has context waiting to be
  delivered, send its next message in Chat before switching; Del keeps the fork
  open instead of launching a terminal without that context.

## Side chat and peers

A main conversation in Terminal can still open independent Side chats, including
Side chats on another provider. The initial context is a snapshot of the latest
persisted provider history. **Refresh context** reads a new snapshot; it does not
resume, interrupt, or write to the main terminal. Messages still being generated
may not have been persisted yet.

Claude reads its account-scoped transcript, Codex uses `thread/read`, and OpenCode
exports the native session with the account's environment. A failed history read
surfaces an error and leaves terminal ownership intact. The existing Side chat
also remains intact after a failed refresh.

Peer mailboxes survive Chat → Terminal → Chat. The chat scheduler leaves mail
queued during a handoff; the terminal dispatcher handles delivery only at its
existing agent-reported safe input boundaries. Main-chat model-picker shortcuts
do not target the hidden composer while Terminal is active. Side chat pickers
continue to control their own forks.
Terminal-mode conversations remain listed in Memory usage, with open and unload
actions targeting the same saved conversation.

## Ownership and recovery

`SessionManager` serializes sends, resumes, and handoffs with the same per-session
lock. A terminal-owned session cannot resume or accept a chat prompt. Side chat
creation takes a read-only snapshot under that lock and starts a separate native
identity. Temporary forks never replace their parent.

The app holds a handoff lease for the PTY's lifetime. Return, tab close, and view
destruction wait for terminal termination and PTY draining before releasing
ownership. Failed launches release their lease so Chat can be resumed. Native
terminal imports reject conflicting provider/account/folder records instead of
overwriting them. Terminal ownership is runtime-only; reopening Del returns saved
chat conversations to Chat.

## Verification

Run ordinary tests with debug assertions enabled: GPUI's test selectors are not
available in a release-profile test binary.

```sh
CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --workspace
CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The ignored integration test invokes real inference in temporary workspaces and
requires installed, authenticated providers. Each run also creates Side chats on
the other two providers, so all three CLIs must be available on PATH. OpenCode
uses its `opencode/big-pickle` model for these checks.

```sh
DEL_MODE_TEST_PROVIDER=codex CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked -p del-app \
  real_provider_chat_terminal_chat_preserves_native_history -- --ignored --nocapture
```

Repeat with `DEL_MODE_TEST_PROVIDER=claude` and `opencode`. The test checks two
native handoffs, preserved history/settings, all nine parent/Side chat provider
combinations across the three runs, refresh after a new terminal turn, Side chat
export/restore, and import through a fresh session manager. Unit tests also cover
concurrent sends/handoffs, queued peer mail, account isolation, failed context
reads, permission arguments, and exact local-command interception.

Build and sign the local app with `CARGO_PROFILE_DEV_DEBUG=0
./scripts/bundle-macos.sh debug`, then relaunch and check the visible Terminal +
Side chat layout and `/chat` return path.

### Validation completed for this change

- Workspace suite: **593 passed, 44 ignored**. The changed agent crate was checked
  again after adding the pending-fork-context guard: **73 passed**.
- Clippy with `-D warnings`, formatting, and whitespace checks passed.
- Real CLI matrix: Claude → Claude/Codex/OpenCode, Codex →
  Claude/Codex/OpenCode, and OpenCode → Claude/Codex/OpenCode all passed.
  Each parent also passed a second terminal handoff and fresh-manager import.
- Live Side chat checks passed for new terminal context, refresh, export/restore,
  and keeping the main terminal running through creation and deletion.
- The expanded Codex test initially stopped at its native update prompt. Its
  second launch now uses the same update-suppression flag as the actual app;
  the complete test passed on rerun.
- Signed macOS debug bundle built and relaunched. Native UI checks passed for
  `/terminal`, typed `/chat`, restored history, Side chat availability, Memory
  usage navigation, hidden-composer shortcut handling, and light/dark surfaces.
- Existing dependency notice: Cargo reports a future Rust compatibility issue
  in `block 0.1.6`; it does not fail the current build or lint checks.

This covers the installed provider versions and the tested scenarios, not every
possible future CLI version, network failure, or external native-editor action.
