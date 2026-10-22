# del-mcp

del's own MCP server, the one TUI-mode agents (Claude Code, Codex) connect to.
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
  loopback (DNS-rebinding / browser protection). 1 MiB body limit.
- Scoping: `report_status` only for yourself; `send_to_session` only to sessions
  you spawned and the session that spawned you; spawn quotas
  (`max_spawn_per_session` 8, `max_spawn_total` 32); `ask_user` times out after
  `ask_timeout` (30 min). Input lengths are capped.
- No filesystem, shell or network tools. Tool failures return `isError: true`;
  malformed arguments return JSON-RPC `-32602`.
- `Credentials` and `LaunchConfig` redact tokens in `Debug`. Claude config files
  have unique names, are created exclusively, and use mode 0600. Codex gets the
  token via an env var, never argv.

## Tools

All inputs are objects with `additionalProperties: false`; unknown fields are
rejected. Object results carry both `content[0].text` (JSON) and
`structuredContent`; acknowledgments carry text only.

| tool | input | result |
| --- | --- | --- |
| `report_status` | `state` (`working\|idle\|blocked\|done\|error`), `summary` (≤500), `session_id?` (must equal your own) | text `status recorded` |
| `ask_user` | `question` (≤4000), `options?` (≤8 strings, ≤200 each) | `{answered: true, answer, option_index?}` or `{answered: false, dismissed: true}`; blocks until the user answers in del; error on timeout |
| `list_sessions` | none | `{sessions: [{id, provider, cwd, title, status, reported_state, is_self}]}` |
| `get_session_summary` | `session_id` | `{info, reported_summary, last_output}` |
| `spawn_session` | `provider` (`codex\|claude`), `cwd` (absolute, existing dir), `prompt` (≤50000) | the new session's `SessionInfo` |
| `send_to_session` | `session_id` (spawned child or parent), `text` (≤20000) | text `sent` |
| `notify` | `title` (≤120), `body` (≤1000) | text `notified` |

`initialize` returns instructions telling the agent its own session id.

## Host contract

```rust
#[async_trait]
trait McpHost { report_status, ask_user, list_sessions, session_summary,
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
the HTTP endpoint (one thread per request, so `ask_user` does not block `ping`).

`cargo run -p del-mcp --example serve -- <dir>` runs the server over a mock host
and prints ready-to-use commands for both CLIs.

## Verification

`cargo test -p del-mcp` exercises HTTP auth/revocation, caller identity,
parent/child messaging, spawn quotas, blocked questions and timeout, all seven
tools, the stdio proxy, launch-file privacy, malformed requests, and the
`ManagerHost` adapter with a real `SessionManager` and mock backend.

The installed Claude Code and Codex CLIs also discovered all seven tools over
both HTTP and stdio. Claude was checked through SDK control requests
(`initialize`, `mcp_status`); Codex through app-server `mcpServerStatus/list`.
Those checks used isolated temporary configuration directories and launch
flags, without model prompts or changes to global configuration.

App wiring remains with the TUI integration: keep the server alive for the
app lifetime, register each session before launch, apply its `LaunchConfig`,
and revoke credentials and remove launch files when the session ends. Route
`McpUi` callbacks onto the UI thread; resolve `ask_user` when the person answers.
