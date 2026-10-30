# del-mcp

Del's MCP server for project peer sessions, browser access, and terminal agents.
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
- Peer discovery, context, requests, and replies are scoped to the host's Del project
  identity, including sibling worktrees. Every operation checks membership again.
  Browser access retains its separate session/family boundary. `report_status`
  only affects the authenticated caller. Spawn quotas and question timeouts are
  unchanged; plain shells are not agent peers.
- No raw filesystem or shell tools. The browser tool can access websites and
  interact with signed-in pages. Tool failures return `isError: true`;
  malformed arguments return JSON-RPC `-32602`.
- `Credentials` and `LaunchConfig` redact tokens in `Debug`. Claude config files
  have unique names, are created exclusively, and use mode 0600. Codex gets the
  terminal token via an env var, never argv. Each Codex chat receives its own
  chat credential through thread/start or thread/resume (provisional credentials
  are rebound to the native ID). Claude gets a private MCP config. Each OpenCode
  chat owns a native server because OpenCode's MCP settings are process-wide;
  accounts/chats never share peer identity. ACP receives HTTP MCP configuration
  when supported, or Del's `--mcp-stdio` proxy. Credentials are revoked on close.
  Legacy browser-only credentials remain restricted to browser operations.

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
| `read_session` | `session_id`, `cursor?`, `max_characters?` (1000–64000) | recorded public event chunks, next cursor, retention notice |
| `send_to_session` | `session_id`, `text` (≤20000), `idempotency_key` | durable queued receipt; does not wait for an answer |
| `reply_to_session` | `request_id`, `text`, `idempotency_key` | reply receipt, routed to the original sender |
| `session_work` | `action`: `get`, `receive`, `complete`, `wait`, `pause`, `resume` | current work state; `receive` pulls a terminal agent's next peer message |
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

Chat credentials expose browser, their own goal, and peer tools. They cannot
spawn sessions or notify as another agent. Website actions remain subject to user
authorization; peer messages do not expand browser or filesystem permissions.

## Host contract

`McpHost` receives the authenticated caller on every operation. `peer_manager`
provides the durable mailbox and task state; `queue_peer`, `reply_peer`,
`read_context`, `peer_work`, and `authorize_peer` form the collaboration seam.
No peer operation uses provider steering to interrupt an existing turn.

The app scheduler delivers pending peer messages at a safe provider input boundary.
It then continues original work until the agent explicitly completes it. Queued
user messages stay blocked across turns, replies, waits, and restarts. Failed or
ambiguous deliveries need visible recovery rather than automatic replay.

Terminal agents pull messages with `session_work receive` at safe tool boundaries.
An explicit `report_status idle/done` permits waking that CLI with queued input;
unknown, working, blocked, or stopped terminals are never typed into. Terminal
context exposes the recorded output tail plus exchanges, with that limitation in
each response. Native CLI user queues are controlled by the CLI, unlike Del's
managed chat queue. Use chat mode for Del's strict peer-before-user scheduling.

See [the peer contract](../../docs/peer-sessions.md) for task semantics and recovery.

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

`cargo test -p del-mcp` exercises authentication/revocation, project isolation,
forged identity/reply rejection, context paging, durable request/reply routing,
peer work completion, terminal safe-boundary delivery, browser isolation, spawn
quotas, question timeouts, and the stdio proxy. Real provider checks are explicitly
ignored by default because they require local authentication and spend tokens.
