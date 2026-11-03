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

Restored history is presentation data: the native transcript is never rewritten.
Claude's metadata-linked local commands and native task notifications become
expandable activity with readable output. Literal XML/code in genuine messages
is preserved. Complete history records from Claude, Codex, and OpenCode share
ANSI-control cleanup, including color sequences and terminal hyperlink wrappers.
Live deltas bypass this cleanup because an escape sequence may span chunks.

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

## Provider-switch integration

Terminal handoff uses the active provider segment's native ID after any provider
switch. The full Del conversation stays in its journal. A durable checkpoint lets
Chat append new terminal events exactly once, and lets Side chats and peer reads
combine earlier provider segments with the live native transcript. Claude's launch
reattaches the public context file and handles switching before its first turn.

Del passes `thread_unload_delay_secs=0` both when launching its Codex app-server
and when opening each thread. The server-wide launch option is required for older
CLIs (verified with 0.159.1) that ignore per-thread unload overrides; newer Codex
versions honor [per-thread settings](https://github.com/openai/codex/pull/55270).
This changes only Del's child process, not the user's global Codex configuration.
Del waits for `thread/closed` after unsubscribing before handing over ownership. An unsubscribe acknowledgement alone
does not release the native lock when another chat keeps the app-server alive.
This follows the [Codex app-server lifecycle](https://learn.chatgpt.com/docs/app-server).
Terminal handoff probes working CLI installations like Chat startup, skipping a
broken executable launcher. The user’s installed CLI files are never modified.

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

### Terminal-history cleanup validation — 2026-11-02

- Workspace suite: **599 passed**; Clippy, formatting, and whitespace checks passed.
- Provider history fixtures verify ANSI cleanup, preserved user/assistant/tool
  roles, Unicode, literal XML and escaped-code examples, native command output,
  and background-notification provenance.
- The reported native Claude transcript passed the opt-in read-only regression
  check. Native files were not edited.
- Live Claude, Codex, and OpenCode handoff suites passed again, including all nine
  parent/Side chat provider combinations and repeated history restoration.
- The signed app was rebuilt and relaunched. The reported conversation now shows
  `Ran /terminal-setup` with expandable plain-text output. Its real messages and
  navigation survive another typed `/terminal` → `/chat` round trip. Light and
  dark appearance were checked; the app was left in its original dark appearance.

For subsequent provider-switch compatibility and Side chat queue validation, see
the [0.1.24 validation record](releases/0.1.24-validation.md).


### Codex 0.159.1 release-delay regression (2026-11-03)

A native two-thread test reproduced the failure to release an idle thread within
five seconds while another thread kept the server alive. With the server-wide
override, release took about 110 ms. A separate real Del MCP-bridge test completed
three Claude → Codex → Claude round trips, both with and without another Codex
chat open. Codex → Claude took 0.25–0.27 seconds; Claude → Codex took 1.75–2.97
seconds, including native setup and context attachment. These are local observations,
not performance guarantees. Neither test sent a model prompt.

Reproduce with the opt-in tests `idle_thread_releases_promptly_while_another_chat_stays_open`
in `del-codex`'s `e2e` target and
`native_claude_codex_round_trips_with_and_without_another_codex_chat` in `del-app`'s
`provider_switch` target. The ordinary replay suite checks the launch option and
still requires the native close notification before handing off a thread.

The rebuilt app also switched the affected existing conversation back from Codex
to Claude successfully, with its earlier messages and switch markers still visible.
The locked workspace suite passed 789 tests (48 opt-in tests ignored), and workspace
Clippy passed with warnings denied. Both new opt-in native tests passed separately.
