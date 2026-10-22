# Auth and terms: what del-claude does and what we could verify

Checked 2026-10-22. This is an engineering summary, not legal advice.

## What the code does
- Spawns the user's installed, unmodified `claude` binary (`claude -p --input-format stream-json ...`).
- Never reads, copies, stores or forwards credentials. Auth is whatever the CLI already has:
  the user's own `claude auth login`, or `ANTHROPIC_API_KEY` etc. in the inherited environment.
- `auth_status()` only runs `claude auth status` and reads `loggedIn` / `authMethod`.
- No `--bare` (which would force API-key-only auth) and no token plumbing, so the sign-in flow stays Anthropic's.

## Verified (primary source, fetched 2026-10-22)
Source: https://code.claude.com/docs/en/legal-and-compliance

- Free/Pro/Max use is under the Consumer Terms; Team/Enterprise/API under the Commercial Terms.
- "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication." Anthropic "does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users", and developers "may not collect, store, or intermediate Claude.ai credentials or session tokens".
- The same page says this does not prevent "an end user from signing in to the unmodified Claude Code binary with their own Claude subscription, including where a platform hosts Claude Code". Platforms running Claude Code must not modify the binary, must not remove or restrict its built-in auth methods, and must not pay for/resell usage; each end user authenticates with their own key or subscription.
- "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK."
- Verified locally: claude 2.1.308 `claude auth status` reports `authMethod: claude.ai`; stream-json mode ran under that login with no token handling by us.

## Unverified / secondary
- Press coverage of the Feb 2026 policy clarification and server-side blocking of subscription tokens outside Claude Code
  (e.g. https://winbuzzer.com/2026/02/19/anthropic-bans-claude-subscription-oauth-in-third-party-apps-xcxwbn/,
  https://thenewstack.io/anthropic-agent-sdk-confusion/). Secondary sources only.
- A third-party README claims that from ~June 2026 `claude -p` / Agent SDK usage draws from a separate monthly credit pool rather than the subscription limits. NOT verified on Anthropic pages; check billing docs before promising users subscription-covered headless usage.
- Whether driving `claude -p` from a desktop app counts as "ordinary, individual usage" for subscription limits is not stated explicitly. Our reading: it is the unmodified binary under the user's own login, which the page allows, but this is an interpretation. Ask Anthropic (contact sales link on the page) before shipping publicly.

## Product guidance
- Show the user which auth mode is active; offer API-key mode as the fallback if subscription use is restricted.
- Do not use the Claude Code name/logo in del's own name or branding (trademark paragraph on the same page).
