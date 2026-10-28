# del-mcp

Del's MCP server for terminal agents and Codex/Claude/OpenCode chat browser access.
No GPUI dependency: the server talks to the app through the `McpHost` trait.

MCP is implemented directly (JSON-RPC 2.0: `initialize`, `ping`, `tools/list`,
`tools/call`; protocol versions 2025-06-18, 2025-03-26, 2024-11-05) rather than
through `rmcp`: the surface is tiny, stateless, and we need exact control over
auth, scoping and the blocking `ask_user`.

## Transport and security

- `McpServer::start(host, ServerOptions)` binds `127.0.0.1:0` (ephemeral port);
  endpoint is `POST http://127.0.0.1:<port>/mcp`, JSON in, JSON out. `GET`/`DELETE`
  return 405 (no server-initiated stream). Notifications get `202`.
- Every request needs `Authorization: Bearer <token>`. Tokens are 256-bit random,
  minted per launch by `register_session(&SessionId) -> Credentials`; **a token
  authenticates as exactly one session**, so the server knows the caller and
  agents cannot claim another identity. `revoke_session` invalidates them.
- `Host` must be loopback with our port and `Origin`, if present, must be
  loopback with our port too, so pages on other local ports are rejected
  (DNS-rebinding / browser protection). 1 MiB body limit.
- Scoping: a caller's *family* is its own session, the sessions it spawned, and
  the session that spawned it. `list_sessions`, `get_session_summary`,
  `send_to_session` (family minus self) and `browser` only reach the family;
  `report_status` only yourself. Spawn quotas (`max_spawn_per_session` 8,
  `max_spawn_total` 32) count live children: a child stops counting once its
  credentials are revoked, and a reservation is released even if the client
  disconnects mid-spawn. `forget_session` drops a deleted session's family links.
  `ask_user` times out after `ask_timeout` (30 min). Input lengths are capped.
- No raw filesystem or shell tools. The browser tool can access websites and
  interact with signed-in pages. Tool failures return `isError: true`;
  malformed arguments return JSON-RPC `-32602`.
- `Credentials` and `LaunchConfig` redact tokens in `Debug`. Claude config files
  have unique names, are created exclusively, and use mode 0600. Codex gets the
  terminal token via an env var, never argv. Codex chat receives its browser-only
  configuration through thread/start or thread/resume; Claude chat gets a private
  MCP config file. Browser-only credentials cannot call coordination tools.

## Tools

All inputs are objects with `additionalProperties: false`; unknown fields are
rejected. Object results carry both `content[0].text` (JSON) and
`structuredContent`; acknowledgments carry text only. Browser results, which can
be megabytes, are sent once as `content[0].text` JSON.

| tool | input | result |
| --- | --- | --- |
| `browser` | `action`, optional `session_id`, `tab_id`, `url`, `selector`, `value`, `key`, `script` | page/tab JSON or PNG image content |
| `report_status` | `state` (`working\|idle\|blocked\|done\|error`), `summary` (≤500), `session_id?` (must equal your own) | text `status recorded` |
| `ask_user` | `question` (≤4000), `options?` (≤8 strings, ≤200 each) | `{answered: true, answer, option_index?}` or `{answered: false, dismissed: true}`; blocks until the user answers in del; error on timeout |
| `list_sessions` | none | `{sessions: [{id, provider, cwd, title, status, reported_state, is_self}]}` |
| `get_session_summary` | `session_id` | `{info, reported_summary, last_output}` |
| `spawn_session` | `provider` (`codex\|claude\|opencode`), `cwd` (absolute, existing dir), `prompt` (≤50000) | the new session's `SessionInfo` |
| `send_to_session` | `session_id` (spawned child or parent), `text` (≤20000) | text `sent` |
| `notify` | `title` (≤120), `body` (≤1000) | text `notified` |

`initialize` returns instructions telling the agent its own session id.

### Browser actions

Del is the default for browser requests unless the user explicitly names another
browser or destination. This routing instruction is supplied to Codex/Claude/OpenCode chats
and in MCP initialization/tool metadata.

`list_tabs` without a session lists the sessions this credential may use (its
family), their browser tabs, and `active_session_id` when the visible session is
one of them. Without `session_id` or `tab_id`, actions use the caller's own
session. Actions on a session the user is not viewing run in that session's
hidden browser without changing the user's selection. In the viewed session,
page-changing actions reveal the Browser pane; `list_tabs`, `snapshot` and
`screenshot` do not. A session must be open in a top tab to run in the
background; otherwise Del opens it first. Agents may open at most 20 tabs per
session. Tabs in one session share its profile;
different sessions use separate profiles. Closing tabs and unloading sessions
retain website data; permanent session deletion removes it.

Actions: `new_tab` (optional `url`), `select_tab`, `close_tab`, `navigate` (`url`),
`back`, `forward`, `reload`, `snapshot`, `screenshot`, `click` (`selector`), `fill`
(`selector`, `value`), `press_key` (`key`), and `evaluate` (`script`). Snapshot returns
page text and CSS selectors for interactive elements. JavaScript is a function
body with `return` and optional `await`, executed only in the webpage. Key events
support Enter, Tab, Escape, and arrows. Navigation acknowledges load start; inspect
snapshot readiness before using the new page. Operations time out after 30 seconds;
page scripts are rejected after 25. Results JSON cannot represent (dates, NaN,
Infinity) are converted rather than failing. Screenshots return MCP image
content. Page text and scripts have bounded sizes. Page `alert`/`confirm`/`prompt`
dialogs name the page origin, and a burst of more than three in ten seconds
blocks further dialogs from that site; hidden browsers dismiss them.

Claude chats get a credential bound to their own session. Codex and OpenCode
chats share one browser-only credential that cannot name its session; it can
only reach Codex/OpenCode chats that are currently running a turn. Terminal
agents retain the complete tool set. Page text is untrusted content, not instructions. Website actions remain
subject to the user's authorization, including forms and signed-in accounts.

## Host contract

```rust
#[async_trait]
trait McpHost { browser, report_status, ask_user, list_sessions, session_summary,
                spawn_session, send_to_session, notify }  // each gets `caller: &SessionId`
```

- `MockHost` records calls; `ask_user` parks until a test calls `take_question()`.
- `ManagerHost::new(SessionManager, Arc<dyn McpUi>)` implements everything over
  `del_agent::SessionManager` (spawn = `start_session` + `send_prompt`; send =
  `steer` when running, else `send_prompt`). The app only implements `McpUi`:
  `ask_user` (async, dropping the future cancels it), `status_reported`, `notify`.
  TUI-mode sessions that are not manager-backed need their own `McpHost`.

## Launching agents

```rust
let server = McpServer::start(host, Default::default()).await?;
let creds = server.register_session(&session_id);
let l = launch::claude(&creds, &Transport::Http, &private_dir)?; // args: --mcp-config <file>
let l = launch::codex(&creds, &Transport::Http);                 // args: -c mcp_servers.del.url=... + env DEL_MCP_TOKEN
```

`LaunchConfig { args, env, files }`: append `args` to the CLI argv, add `env` to
the child environment, delete `files` when the session ends. Neither touches
global config (Claude keeps the user's other servers; no `--strict-mcp-config`).
`Transport::Stdio { binary }` points the agent at `del-mcp-stdio` instead, which
reads `DEL_MCP_URL` / `DEL_MCP_TOKEN` and proxies newline-delimited JSON-RPC to
the HTTP endpoint (one thread per request, at most 64 in flight, so `ask_user`
does not block `ping`).

`cargo run -p del-mcp --example serve -- <dir>` runs the server over a mock host
and prints ready-to-use commands for both CLIs.

## Verification

`cargo test -p del-mcp` exercises HTTP auth/revocation, caller identity,
parent/child messaging, spawn quotas, blocked questions and timeout, all eight
tools, the stdio proxy, launch-file privacy, malformed requests, and the
`ManagerHost` adapter with a real `SessionManager` and mock backend.

Codex browser configuration was checked through the installed app-server's
thread/start, using a local MCP fixture without sending a model prompt. Native
WebKit validation is available through `python3 scripts/test-browser.py`.

App wiring remains with the TUI integration: keep the server alive for the
app lifetime, register each session before launch, apply its `LaunchConfig`,
and revoke credentials and remove launch files when the session ends. Route
`McpUi` callbacks onto the UI thread; resolve `ask_user` when the person answers.

### Claude chat goal controls

Claude streaming chats get a separate credential scoped to their own session,
exposing only `browser` and `goal`. The `goal` tool accepts `get`, `set` with an
objective, `resume`, and `clear`; it does not accept a target session ID. Goal
mutations queue native commands behind the calling turn and return `queued: true`.
The model must finish that turn before native confirmation updates the goal card.
Ordinary browser-only credentials cannot control goals. Other session tools and
sibling spawning are not exposed to these chat credentials.

OpenCode 2.x receives launch-local MCP configuration through
`OPENCODE_CONFIG_CONTENT` in standalone terminal mode. Its chat adapter connects
a browser-only credential to its private native server and attaches Del browser
routing through native session instruction entries. Live integration checks in
`app_bridge.rs` exercise cold startup and browser requests through OpenCode.
