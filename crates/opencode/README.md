# del-opencode

Native OpenCode 2.x adapter, developed against the 2.1.4 protocol. It runs an
authenticated loopback server per workspace; OpenCode owns inference, its agent
loop, native skills, permissions, models, and saved conversations. This adapter
does not use the incompatible OpenCode 1.x HTTP API.

The app discovers the installed `opencode` binary, including
`~/.opencode/bin/opencode`. Sign in with `opencode auth login` through Settings.
Model discovery uses the native catalog; effort choices are the selected model's
advertised variants. Del uses a white OpenCode accent. There is no synthetic Fast
tier or goal loop: the inspected 2.1.4 protocol exposes no native goals API.

Chat uses structured prompts, file URIs, native skill IDs, streaming events,
permission replies, and native steering. Native forms use Del's question surface.
Model changes apply between turns; resetting the model resolves OpenCode's current
default. Saved chats reload native paginated message history. Conversation search
reads native session exports on its background worker.
Chat titles use OpenCode's native transient generation endpoint with the selected
model. These requests use model quota without changing conversation history;
failed requests retain Del's local first-message fallback.

Terminal launch allocates a native session before opening the TUI and persists
its exact ID. Model and variant are set on that session; they are not passed as
unsupported TUI flags. The terminal uses a standalone server so the launch's
session-scoped Del MCP configuration is applied. Chat receives browser-only MCP
access; OpenCode configures MCP per workspace server, so the chats of one
workspace share a credential that reaches only those chats (and of them, only
the ones running a turn). Terminal sessions receive Del's coordination tools.
Del disables OpenCode's shared inner tab strip for its terminal launches, so a
fresh terminal cannot restore unrelated tabs from earlier OpenCode runs. Saved
Del terminals still reopen their exact native conversation ID.

## Verification

Unit tests cover protocol mapping, structured skills, model variants, and native
form answers. Live checks exercise skill discovery/execution, attachments, file
edits and restored diffs, model resets, local and model-backed commands, native
form answer/cancellation, terminal resume, terminal MCP status/questions, and the
app MCP browser bridge. Stop, busy rejection, and steering have a separate live
example. A local scripted model endpoint verifies that native title generation
returns a title and leaves conversation history unchanged.
The live checks below require an installed OpenCode 2.x CLI. Use
isolated data directories to keep test history separate from personal history:

```sh
cargo test --locked -p del-opencode
XDG_DATA_HOME=/tmp/del-opencode-check/data XDG_STATE_HOME=/tmp/del-opencode-check/state \
  cargo run --locked -p del-opencode --example opencode_smoke
XDG_DATA_HOME=/tmp/del-opencode-check/data XDG_STATE_HOME=/tmp/del-opencode-check/state \
  cargo run --locked -p del-opencode --example opencode_controls
XDG_DATA_HOME=/tmp/del-opencode-check/data XDG_STATE_HOME=/tmp/del-opencode-check/state \
  cargo test --locked -p del-opencode -- --ignored
XDG_DATA_HOME=/tmp/del-opencode-check/data XDG_STATE_HOME=/tmp/del-opencode-check/state \
  cargo test --locked -p del-app real_opencode_terminal -- --ignored
XDG_DATA_HOME=/tmp/del-opencode-check/data XDG_STATE_HOME=/tmp/del-opencode-check/state \
  cargo test --locked -p del-mcp --test app_bridge real_opencode_ -- --ignored
```

The smoke example uses `opencode/big-pickle` when advertised. Set
`DEL_OPENCODE_SMOKE_MODEL` to select another model. Live tests can invoke provider
inference; account access remains owned by OpenCode.
