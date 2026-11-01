# Del product specs

Specs for the next set of Del capabilities. Each file stands alone; this page
holds the shared thesis and build order. `docs/neverfuckingchange.md` overrides
anything here. When a spec conflicts with a rule there, the rule wins and the
spec is updated.

## Thesis

Del is a supervisory control surface for coding agents, not a project-management
tool and not an editor. What it manages is the user's attention. Agents are
cheap to start. The human is the bottleneck: noticing what needs them, getting
back up to speed, checking the work, and deciding.

Every spec below serves one loop:

1. **Dispatch.** Start work in any provider.
2. **Watch.** Ignore agents until one needs you ([attention](attention.md), on
   top of [agent state](agent-state.md)).
3. **Test.** Run exactly the branch you mean to check ([run](run.md)).
4. **Second opinion.** Move the thread to another provider, or ask one
   ([provider switch](provider-switch.md)).
5. **Automate the loop.** Turn repeated glue into reviewed workflows
   ([workflows](workflows.md)).

## Specs

| Spec | Summary |
| --- | --- |
| [provider-switch.md](provider-switch.md) | Switch a chat's provider mid-thread with context, or ask another provider a one-off question |
| [agent-state.md](agent-state.md) | One reliable, normalized "what is this agent doing" signal for chats and terminals |
| [attention.md](attention.md) | Replaces kanban: peripheral status, a Needs-you queue, "since you last looked", Done cards, and an Overview lens |
| [run.md](run.md) | Run and test a session's branch with clear branch identity and feedback that goes back to the right chat |
| [workflows.md](workflows.md) | Event-triggered pipelines that agents can draft and only the user can enable |

## Build order

1. **Provider switch.** Reuse the side-chat context foundation, adding stable main-chat
   identity, durable provider segments, separate context attachment and historical
   reads across switches. Every switch starts a fresh native session.
2. **Agent state.** Attention is only as good as this signal. A wrong "needs
   you" is worse than none.
3. **Attention.** Needs-you queue, status lines, recaps.
4. **Run.** Per-session run commands, the live slot, branch identity.
5. **Workflows.** These automate steps 1–4, so they come last.

## Shared principles

- **Structured signals over scraping.** Use provider events, MCP calls, hooks
  and terminal escape sequences, never prose parsing or guesswork (this matches
  `docs/peer-sessions.md`).
- **Visible over magic.** Any work Del starts appears as a normal session the
  user can open.
- **Calm by default.** Interrupt only for state changes that need a person.
  Never notify on progress.
- **Same behavior across providers.** A feature that works for one provider is
  designed so the others can adopt it. Where a provider lacks a capability, the
  UI says so instead of faking it.
- **The user's work is never lost.** Rules from "Projects and worktrees" apply
  to every new flow: never discard Git work, never silently overwrite a
  checkout.

## Evidence base

The research behind these specs, as of Nov 2026:
`reports/AI coding tools landscape for Del.md` and the notes in
`research_notes/AI coding tools landscape for Del/`. Both are local files and
not tracked in the repo.
