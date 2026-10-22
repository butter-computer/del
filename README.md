# Del

Del is a native Rust desktop workspace for coding agents, built with GPUI. It
combines Codex, Claude Code, and installed ACP agents with conversation tabs,
a follow-up queue, a file viewer, and integrated terminals.

The project is a local-development application (version `0.1.0`), with macOS as
the current desktop target. Provider CLIs handle authentication and inference;
Del does not include model weights or replace their account requirements.

## Build and run

Install Rust with support for edition 2024, the macOS Xcode developer tools,
and the provider CLI you want to use. Sign in through that CLI or use Del's
**Settings → Providers** sign-in action, which opens the provider login in Terminal.

```sh
cargo run --locked -p del-app
```

For the signed macOS application, Dock icon, and a stable launch path:

```sh
./scripts/bundle-macos.sh
open target/debug/del.app
```

Release builds are also supported:

```sh
./scripts/bundle-macos.sh release
open target/release/del.app
```

After rebuilding, reopen Del to run the new binary. Building does not replace
an already-running process. Keep using the same bundle path for local upgrades.

### Isolated UI development

The mock backend returns scripted responses without contacting a model. Use a
separate data directory so test conversations cannot mix with normal history:

```sh
DEL_BACKEND=mock DEL_DATA_DIR=/tmp/del-ui-dev cargo run --locked -p del-app
```

Mock replies, tool activity, and approvals are simulations; mock providers have
no model catalog. Do not use mock mode when testing real provider behavior.

### macOS signing and permissions

The bundle script signs with a persistent certificate-backed local identity in
`~/Library/Application Support/del/development-signing`. Preserve this directory:
it contains a private signing key in its own keychain, its password, and the
certificate. Regenerating it changes the identity macOS uses for permission grants.

The script stages and verifies a complete signed bundle before replacing the old
one. Running versions are retained under `target/<profile>/.del-retired` until
their processes exit; later builds clean them up. Deleting a running bundle can
prevent macOS from resolving its identity for permission checks.
The script does not edit TCC, install a trusted root, or grant access automatically.
Moving from an unsigned/ad-hoc build can require one final grant for each permission.
Subsequent builds at the same path reuse the signing identity; macOS still controls
permissions and can require a new grant if access is revoked or the identity changes.
`cargo run` is a separate executable and does not provide this bundle identity.
Documents, Downloads, and Desktop have separate macOS grants: allowing one does
not grant the others.

To use an existing Apple Development or Developer ID certificate, set
`DEL_SIGNING_IDENTITY` and optionally `DEL_SIGNING_KEYCHAIN`. Ad-hoc signing is
rejected. The local development identity is not a notarized distribution release.

```sh
python3 scripts/test-macos-signing.py
```

`python3 scripts/test-macos-install.py` reproduces the failure caused by deleting
a running bundle, then checks that replacements preserve its macOS signing
identity and retired bundles are cleaned up after exit. Neither script grants
permissions or inspects the user's TCC database.

The editable app icon is `crates/app/assets/del.svg`; PNG and ICNS exports are
checked in. `node scripts/generate-icon.cjs` regenerates them on macOS when the
`sharp` Node package is available. Bundle builds use the checked-in exports.

## Conversations and workspace

- Choose a provider, model, and project folder in a new-session composer.
- The project folder and Chat/Terminal controls sit in a darker footer beneath
  the input, keeping workspace selection distinct from the message area.
- **Chat / Terminal** selects how the first prompt will run. Choosing Terminal
  keeps the initial composer visible; the terminal opens after submission.
- A session's project folder is fixed after creation. Its folder label becomes
  informational rather than a clickable folder picker.
- Chat renders streaming answers, reasoning, tool activity, diffs, usage, and
  approval requests. The message rail provides jump-to-prompt navigation.
- Sessions have tabs and sidebar entries, with rename, pin, unread, archive,
  restore, and permanent-delete actions. Terminal entries share the session
  context menu. Provider-owned titles update from conversation metadata; manual
  renames take precedence.
- A blue dot at the far right marks unread conversations, including background
  replies that finish before being opened. Hover actions make room for the dot.
- Sidebars resize from their panel boundaries. Empty left-sidebar space can drag
  the macOS window. Hover highlights slide between session/menu rows, and the
  keep-awake control animates yellow steam while active.

## Models and reasoning

The picker uses provider-advertised choices, not a hardcoded list of model names.
All visible catalog entries remain accessible in a bounded scrolling list, without
a scrollbar; the bottom fades while more entries are below. Refresh retries
catalog discovery. Availability depends on the installed CLI, agent, and account.

| Provider | Model discovery and controls |
| --- | --- |
| Codex | Paginated app-server `model/list`; advertised reasoning levels and priority/fast tier |
| Claude | CLI initialization metadata; advertised models and effort settings passed through `--effort` |
| ACP | Installed registry agents and their advertised model/config options; `thought_level` is exposed for the agent's default entry when available |

Selecting a model opens its reasoning controls. Slider stops correspond to
advertised effort levels, and the accent follows the selected provider (including
Claude orange). Fast mode is enabled only when the model advertises support.
Reset clears explicit effort/fast settings and refreshes the catalog.

Model, effort, and service-tier selections are stored with the conversation.
Existing chat settings apply on subsequent prompts through the backend's
reconfiguration path; they do not rewrite a turn already in flight. Changing a
model clears incompatible effort/tier choices. Provider changes use a new draft.
Catalog discovery submits no prompt, but can start a CLI process; ACP discovery
may create a temporary session.

## Follow-up queue and images

While a reply runs, **Enter** queues the current draft. **Command-Enter** submits
it as steering input; with an empty composer, it submits the top queued item.
Completed turns advance remaining follow-ups in order. A busy provider leaves
an undelivered message waiting instead of displaying a failed-send error or
incorrectly changing the running status to Done.

Queue rows have **Steer**, **Delete**, and **Edit** actions. Edit returns text and
images to the composer; an existing draft is preserved as another queued item.
A drag handle appears on hover for reordering. Rows have subtle separators and a
surface just darker than the composer. Around four rows fit before the list
scrolls, with a bottom fade and no scrollbar.

Attach images using the plus button, paste, or file drop. PNG, JPEG, and WebP
inputs are decoded and saved as local PNG attachments; input files are limited
to 20 MiB each. Images appear as thumbnails in the composer and queue and remain
attached through edit, reorder, and retry. Codex receives local image references;
Claude and ACP receive encoded image content, subject to provider/model support.
The Terminal-mode opening composer currently accepts text prompts only.

Queues persist per conversation. Known busy rejections are recoverable;
uncertain deliveries interrupted before acknowledgement require review rather
than automatic replay. Other delivery failures keep the item and its error.
A stopped or failed turn does not automatically advance the queue.

Steering differs by harness: Codex supports active-turn steering; Claude's
stream-json interface handles in-flight user input as a native queued follow-up.
ACP steering is currently unsupported and leaves the message available to retry.
Steering during streaming keeps the current assistant answer in one block.
Sent steering messages use a small muted yellow queue icon beside the text,
without a separate “steer” heading.

## Files and terminals

The right sidebar has **Files** and **Terminal** panes. The file tree loads
folders on demand, supports optional dotfiles, and excludes generated
`target`/`node_modules` trees. Selection highlights fit the file label. The tree
and preview have a draggable split and subtle edge fades.

The viewer is read-only. Text files show line numbers; Markdown supports
**Markdown / Preview** modes. Text previews are capped at 512 KiB and 3,000 lines.
Expanded folders, selected file, dotfile preference, split height, and Markdown
mode are saved per chat.

Terminals use a real PTY, support shell and agent sessions, and retain colored
terminal cells while letting the normal chat surface show through their default
background. Saved Codex/Claude terminal sessions reopen the exact provider
conversation ID without replaying the initial prompt. Legacy Codex identity
recovery proceeds only when one saved conversation matches its launch time and
workspace. Unresolved identity is reported rather than choosing the latest chat.

Built-in Codex and Claude terminal launches automatically trust the workspace
opened in Del, including resumed sessions. Codex receives a launch-scoped
[`projects.<path>.trust_level`](https://developers.openai.com/codex/config-reference/)
override. Claude's recognized startup trust dialog is selected and confirmed;
Claude then persists its own decision. This does not change tool approval or
sandbox settings. Claude confirmation is limited to startup (two minutes), the
exact workspace, and one acceptance. Unrecognized dialogs stay interactive.
Shell/custom commands and arbitrary ACP agents keep their own trust setup.

Del's MCP bridge provides scoped session coordination, status, notifications,
and user questions for supported terminal agents. Launch-specific credentials
and MCP configuration are kept separate from saved launch history. See
[the MCP documentation](crates/mcp/README.md) for tools, transport, and scope.

## Harness updates

Before launching or reopening an agent process, Del checks its installed harness
and shows an updating screen while an update is applied. Successful checks are
cached for 30 minutes; concurrent launches share an updater.

- Global npm packages update through their owning prefix.
- Homebrew packages update through their owning formula or cask.
- Native Claude Code installations use `claude update`.
- Installed ACP agents use the same npm/Homebrew ownership detection.
- Custom or unmanaged binaries are left alone; registry install hints are not run.

The first prompt waits for the update and is submitted once. Recognized successful
CLI self-update exits reopen the saved conversation automatically. Failed updates
retain the waiting prompt for retry. Running conversations are not interrupted to
install updates. This updates supported harness installations, not Del itself or
arbitrary applications on the computer.

## Settings and shortcuts

Settings has **Preferences** (persisted Light/Dark appearance) and **Providers**
(sign-in status, sign-in, and re-check). The right sidebar is hidden in Settings.
Hold Command to see numeric hints on the top tabs. There are no numeric sidebar
session shortcuts.

| Shortcut | Action |
| --- | --- |
| Command-1–9 | Select a top tab by position |
| Command-N / Command-T | New session |
| Command-W | Close the active tab |
| Control-Tab / Control-Shift-Tab | Next / previous tab |
| Command-Shift-] / Command-Shift-[ | Next / previous tab |
| Command-L | Focus composer |
| Enter | Send when idle; queue while running |
| Shift-Enter | Insert a newline |
| Command-Enter | Steer the draft, or submit the first queued item |
| Command-. | Stop the active chat turn |
| Command-B | Toggle the left sidebar |
| Command-Backslash / Command-Shift-B | Toggle the file sidebar |
| Command-J / Control-Backtick | Open the workspace terminal pane |
| Command-Comma | Settings |
| Command-Q | Quit |

## Local data

Del uses the platform user-data directory (`~/Library/Application Support/del`
on macOS). `DEL_DATA_DIR` overrides application data for tests/development.
Provider credentials and provider-owned transcripts remain managed by their CLIs.

| Path within Del's data directory | Contents |
| --- | --- |
| `sessions.jsonl` | Chat session index and configuration |
| `sidebar.json` | Local names, pins, archive/unread metadata |
| `terminals.json` | Terminal launches and provider conversation identities |
| `followups/` | Per-conversation queued messages and delivery state |
| `attachments/` | Local image copies used by drafts and queued messages |
| `right-panels.json` | Per-chat file/terminal pane state |
| `sidebar-widths.json` | Sidebar sizes |
| `appearance.json` | Light/Dark preference |
| `acp-agents.json` | ACP registry overrides |

The macOS development-signing identity uses the fixed directory described above.
See [ACP configuration](crates/acp/README.md) for custom installed agents.

## Codebase

| Crate | Responsibility |
| --- | --- |
| `app` | GPUI shell, composer, queue, settings, file viewer, terminal host, updates |
| `agent` | Provider-neutral sessions, events, model metadata, persistence |
| `codex` | Codex app-server adapter |
| `claude` | Claude streaming CLI adapter |
| `acp` | ACP registry and protocol adapter |
| `terminal` | PTY lifecycle, input, emulator state, and rendering |
| `mcp` | Session bridge, MCP server, and stdio proxy |
| `core` | Shared UI-independent helpers and shell metrics |

GPUI is pinned through `gpui-kit =0.8.0` and `gpui-pre =0.3.9`; update these pins
together. Development dependencies are optimized to keep the UI responsive.
Crate READMEs describe individual adapters and protocol behavior in more detail.

## Validation

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p del-app
python3 scripts/test-macos-signing.py
python3 scripts/test-macos-install.py
```

Tests cover provider transcript replay, queue ordering and busy recovery,
steering without duplicate output, attachments, terminal resume identities,
PTY lifecycle, updater ownership/failure handling, and persisted UI metadata.
Tests requiring installed/authenticated provider CLIs are ignored by default;
run those explicitly when validating live-provider behavior. Signing continuity
and installation while running have separate macOS regression scripts.
Automated tests do not replace visual checks
of hover, dragging, scrolling, and native permission dialogs.
