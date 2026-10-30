# del-acp

An additional `AgentBackend` for agents speaking ACP v1 over newline-delimited
JSON-RPC on stdio. Native Codex and Claude backends remain available.

Each session owns one process group. The client negotiates ACP v1, creates a
session, streams text, reasoning, plans, tool progress and diffs into del events,
and ends each turn from the `session/prompt` response. Stop sends `session/cancel`.
An agent that does not settle cancellation within five seconds is terminated.
Close, shutdown, setup failure and backend drop also terminate child processes.

The UI's Allow and Deny decisions select the offered `allow_once` and
`reject_once` option IDs. If there is no matching one-time option, the request
is cancelled. Binary UI decisions never grant persistent permissions. Ask mode
forwards every permission request to the UI; AutoEdit and FullAuto can answer
matching requests automatically. Plan mode requires an advertised `plan` mode.
An agent controls which operations require a permission request.

## Agent configuration

Defaults include `claude-agent-acp`, the older `claude-code-acp`, `codex-acp`,
`gemini --acp`, `pi-acp`, and `dsh acp`. Detection checks executable availability
without downloading adapters or starting login flows. If there is no `dsh` on
PATH, a built `~/dev/harness-eval/deepseek/apps/cli/lib/bin.js` is used with Node
and the `acp` profile. An unbuilt source checkout is not an installed agent.
Older Gemini installations can override the flag with `--experimental-acp`.

Add or override entries in `<del data directory>/acp-agents.json`, alongside the
session index. `DEL_DATA_DIR` overrides that directory. On macOS the default is
`~/Library/Application Support/del`.

```json
{
  "agents": [
    {
      "id": "my-agent",
      "name": "My ACP agent",
      "command": "/absolute/path/to/agent",
      "args": ["--acp"],
      "env": {"MY_AGENT_OPTION": "value"}
    }
  ]
}
```

The `id` must be nonempty and contain no `/`. User entries replace defaults
with the same ID. Commands inherit the process environment plus configured
overrides. Sign in through the agent's own CLI; del does not manage credentials.
The app offers ACP as a provider. Its model picker discovers installed registry
agents and their advertised model/config choices. An empty selection uses the
first installed registry entry. Discovery can create a temporary ACP session,
but does not submit a prompt.

For programmatic callers, `SessionConfig.model` selects `agent-id` or
`agent-id/model-id`. The latter uses an advertised model config option through
`session/set_config_option`, or the agent's optional `session/set_model` method.
Unsupported selections report an error. Advertised `thought_level` options map
to reasoning effort; unsupported effort values are rejected. Opaque del session IDs encode both
the agent ID and its wire session ID, so resume keeps the original agent even
when the default agent changes. Resume sends `session/load` only when advertised.

## Limits

File and terminal client capabilities are disabled; callbacks receive JSON-RPC
method-not-found responses. Agents may use their own tools instead. Image
attachments with an image MIME type are sent as base64 ACP image content;
other attachments become textual path references. Actual image support depends
on the agent and model. ACP v1 has no standard steer method, so
steering returns an explicit error. Stop a turn, then send a new prompt.

The shared `SessionHandle` is delivered after setup. Conversation replay emitted
during `session/load` is not added to del's chat history; resume restores the
agent's context for subsequent prompts. Agent-specific authentication and model
configuration remain the agent's responsibility.

## Verification

`cargo test -p del-acp` runs pure translation tests and a scripted Python stdio
agent. Python 3 is required for the subprocess tests. They cover prompt reuse,
text and reasoning, tool diffs, approval and denial, cancellation, unsupported
callbacks, load capability checks, authentication errors, retries and shutdown
when stdout closes but the agent ignores SIGTERM.

Run `cargo run -p del-acp --example smoke -- gemini` explicitly to exercise an
installed agent using its existing login. The example uses a temporary working
directory, requests a `printf ACP_SMOKE_OK` tool call, approves only a matching
command request, reports observed tools and permission requests, and tests stop.
It has a 90-second deadline and shuts down its process group on every outcome.

## Del peer tools

Each ACP conversation receives an authenticated Del MCP server in `session/new`
or `session/load`. Del uses HTTP when the agent advertises `mcpCapabilities.http`,
and otherwise starts its own `--mcp-stdio` bridge. The provisional credential is
rebound to the native conversation ID and revoked when that process closes.
Peers can read and message sessions in the same Del project; these tools do not
change ACP permission prompts or grant new filesystem capabilities.
