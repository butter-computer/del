# Del

Del is a native Rust desktop workspace for coding agents, built with GPUI. It
combines Codex, Claude Code, and OpenCode 2.x with conversation tabs,
a follow-up queue, a file viewer, integrated terminals, a session browser, and
native Apple voice dictation. Sessions in a project can read each other’s recorded
context and exchange requests and replies as equal peers.

Del `0.1.19` builds from the current source for Apple Silicon Macs running macOS 11 or later. Provider CLIs handle authentication and inference;
Del does not include model weights or replace their account requirements.

The agreed UI and behavior rules are documented in
[docs/neverfuckingchange.md](docs/neverfuckingchange.md). Preserve these decisions
when making future changes.

## Download and install

The latest downloadable installer is **0.1.13**. The source version is **0.1.19**;
see its [release notes](docs/releases/0.1.19.md) and
[validation record](docs/releases/0.1.19-validation.md).
Download **Del-0.1.13-arm64.dmg** from the [0.1.13 release](https://github.com/butter-computer/del/releases/tag/v0.1.13),
open it, and drag **Del.app** to **Applications**. The release includes a `.sha256`
file for verifying the download. Provider CLIs and their accounts are installed
and authenticated separately; they are not included in the DMG.

This release is ad-hoc signed and **not notarized**. A browser-downloaded copy
can be blocked by macOS even though a locally built copy opens normally.
If macOS says “Apple could not verify Del,” dismiss the alert with **Done**,
then open **System Settings → Privacy & Security → Open Anyway** for Del.
Confirm **Open** (and authenticate if prompted), then launch `/Applications/Del.app`.
Only approve a copy you downloaded from the release linked above and trust.
This creates an exception for Del; it does not disable Gatekeeper globally.
See [Apple’s instructions](https://support.apple.com/en-gb/102445).
The release uses `computer.butter.del`; local development builds keep their
separate signing identity. macOS may ask for permissions again when switching.

To build the public release artifacts locally:

```sh
./scripts/make-dmg.sh
# target/release/Del-0.1.19-arm64.dmg, its .sha256 sidecar, and latest.json
```

This builds the optimized executable, packages the icon and permission descriptions,
and verifies the app signature and DMG before producing the checksum. It does not
publish a GitHub release or modify the local development signing keychain. Upload
the DMG, `.sha256` sidecar, and `target/release/latest.json` together to a draft
GitHub release, then publish it only after all three assets are uploaded.

## Multiple provider accounts

In **Settings → Providers**, use **+** beside Claude, Codex, or OpenCode to add
an account. Del opens the provider's own sign-in flow in Terminal and saves a
separate local profile. Sign-in skips broken CLI launchers and uses a working
installation when available; failures are reported in Settings. No account name is required: Codex and Claude
usernames are shown without the email domain, with green text identifying the
default account in Settings, without a checkmark. OpenCode’s built-in login is
labeled **System**. When identity is unavailable, Codex and Claude fall back to
**Account 1** for the system login; added profiles start at **Account 2**. Add account is disabled while that provider has
a sign-in in progress; complete it or cancel in Terminal before adding another.
Use **Make default** for the account new chats should use, or the red trash icon
to remove an unused saved account. Accounts referenced by saved chats or terminals
cannot be deleted; switch chats away and remove terminal references first. Sign in appears only when reconnecting is needed. Removed profiles are
kept locally under `removed-accounts/` for 30 days for recovery; after that Del
deletes them, including their saved credentials, at the next launch.
Successful sign-in closes only its own idle Terminal window. Duplicate email
identities appear once. Codex and Claude accounts show a slim usage bar and percentage
remaining, using the most constrained reported allowance. The account picker shows
a small ring without a number. Claude usage comes from its native `get_usage`
control request, without sending a prompt or reading credentials. Unavailable quota
(including older Claude CLIs without this query) stays hidden.

In the composer's provider picker, click a provider to select its default account, or hover its arrow to choose the default account or a saved account. Only the account selected for the current chat has green text; the picker has no account checkmarks. Model lists and commands load for that selection.
A new chat remembers its account across restarts. Choosing a different account of the same provider
resumes the existing chat with its native history under that account. Switching
is blocked during a running response; a failed transfer reconnects the original
account. Changing providers still opens a new draft. New chats use the provider’s chosen default
account. Saved accounts work in Chat and Terminal modes. Terminal launches retain their
selected account when reopened. OpenCode’s built-in account is labeled **System**.

Profiles live under Del's local `accounts/<UUID>/` directory; `accounts.json`
stores profile metadata, not tokens. Provider CLIs manage credentials and refresh:
Claude uses `CLAUDE_CONFIG_DIR`, Codex uses a separate `CODEX_HOME` with file
credential storage, and OpenCode 2.x uses isolated config/data/database paths
and a private server for sign-in. Each OpenCode profile can contain connections
to several upstream providers. Profiles are local to this Mac, not synced through Git.
Claude Console accounts should use API keys for isolation; separate config
directories alone do not isolate two keyless Console logins.

## Build and run

Install Rust with support for edition 2024, the macOS Xcode developer tools,
and the provider CLI you want to use. Sign in through that CLI or use Del's
**Settings → Providers** sign-in action, which opens the provider login in Terminal.

```sh
cargo run --locked -p del-app
```

For the signed macOS application, voice input, Dock icon, and a stable launch path:

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

The background shaders are precompiled with Xcode's Metal compiler (`xcrun metal`).
On a Mac with only the Command Line Tools, debug builds still succeed with a
warning and compile the shaders from source at first use. Release builds require
the Metal compiler (install Xcode or point `DEVELOPER_DIR` at it); set
`DEL_ALLOW_RUNTIME_SHADERS=1` to build a release without it anyway.

### Running on another Mac

Each Mac needs its own provider CLI installation and authenticated account. Pulling
this repository does not install providers or transfer logins, model access, or
local settings. Build the app on the destination Mac using the commands above;
do not copy another Mac's `target` directory or credentials.

Del prepares a child-process PATH for provider probes, model discovery, chat,
and history loading. It includes the selected executable's directory and common
Homebrew (Apple Silicon and Intel), native installer, npm, Bun, Volta, and nvm
locations, so Finder launches can resolve CLI runtimes such as Node. Existing
launch environment settings remain inherited; Del does not source shell startup
scripts or copy API keys from another machine. Custom installation locations
must be included in the environment used to launch Del.

If models or chat fail on a new Mac:

1. Open **Settings → Providers** and check installation and sign-in status.
2. In Terminal, run the chosen CLI's `--version` command and sign in using
   `codex login`, `claude auth login`, or `opencode auth login`. Verify a normal
   request works in that CLI with the same account and workspace.
3. Reopen Del after installing or updating a provider, refresh the model list,
   and try **Default** first. Model availability depends on that account/provider.
4. If the CLI works but Del fails, report the provider/version, macOS version,
   processor architecture, and the exact Del error. Never share API keys or
   credential files. A successful local build alone does not verify live access
   on every Mac or account.

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

The glass app icon uses three tapered strokes forming an angular D. Its editable
source is `crates/app/assets/del.svg`; PNG and ICNS exports are checked in. `node scripts/generate-icon.cjs` regenerates them on macOS when the
`sharp` Node package is available. Bundle builds use the checked-in exports.
Direct development launches also set the Dock icon from the embedded ICNS asset.

## Conversations and workspace

The sidebar has **Pinned** (hidden when empty) and **Projects**. Click either
heading to smoothly collapse or expand its entire section; individual projects
also expand independently. Section choices persist across restarts, with a separate
collapse state for Archived. The Projects plus button opens the directory picker
without toggling the section. Chats and
terminals are sessions in the same project list; there is no separate terminal
section. Projects are created automatically from session folders. Git subfolders
and linked worktrees group under the parent repository, while non-Git folders use
their canonical full paths. Existing histories, pins, titles, and unread state are
preserved. Project membership is derived from the stored working directory, so no
provider transcript migration is needed. The archive button beside Settings switches to **Archived**, showing only archived
sessions grouped by project, with a selected background on the toggle. The heading
and project-row plus buttons are hidden in this view; the global New Session
action returns to active Projects. Project identities and collapsed state
persist in `projects.json`. Hover a project for **+** and its three-dot menu;
choose Chat or Terminal in the composer. Terminal rows show a terminal icon with
a small provider badge. Each new-session action creates an independent draft in
the selected session's directory, or the last directory when no tab is selected.
The Projects heading's **+** opens a directory picker. Changing a draft's folder
moves it to that project and expands the destination.

Projects are directory groups, not separate containers: only sessions and explicit
drafts make them visible. Rename changes the display name only. Archive hides the
project's sessions from the active list. Restoring one session restores only that
session; archived siblings stay in Archived, so a project can appear in both views.
Restoring a project restores all its archived sessions. Deleting from Projects
removes all its histories; deleting from Archived removes only archived histories.
Deletion requires confirmation and never deletes directory contents or Git work.
Rename edits the project name in place in the sidebar, like session renaming.
Enter saves; Escape or leaving the field cancels. Project deletion uses an opaque
confirmation dialog with Cancel and a red Permanently delete button.
Archiving or deleting the last project leaves an empty, usable new-session page,
without inventing a replacement `/` project. App startup also shows an empty
composer without creating a draft tab or project. You can submit directly from
that page; starting a conversation creates its session and directory group.
Explicit New Session actions still create separate draft tabs immediately.

Pinned sessions appear once, above projects, in pin order, without a persistent
pin icon. Unpin remains available on hover and in the context menu. New pins append. Drag
sessions within their project or within the shared pinned group, including across
chat and terminal modes. Dragging never moves a session to a different project or
changes its pin status. Manual order persists across restarts.

Reasoning headers show a shimmering **Thinking** label while active and **Thought**
after completion. Click the label to expand or collapse the details; there is no
spinner or separate show/hide button. Reduce Motion keeps the label static.
Stop requests remain pending while a provider is preparing a turn, so an early
click still interrupts it when it starts. “Stopping…” acknowledges the request;
automatic follow-ups pause until another prompt is explicitly submitted.
The composer omits the token-usage line. For existing sessions, the folder label
shows the project name; hover it to see the branch.

### Branches and managed worktrees

Below the new-session composer, the directory, **Branch**, **New worktree** checkbox,
and Chat/Terminal controls share one row. Choose a starting branch and optionally
enable New worktree. The branch picker opens below its button in a compact
320-point panel, with a focused, icon-free search field that filters local and
remote names case-insensitively. Its list is capped at eight rows and
shrinks to fit the window, with wheel/trackpad scrolling, no visible scrollbar,
and a bottom fade while more results remain. Long branch names fade at the trailing
edge like tab titles, without ellipses. Use ↑/↓ or Page Up/Page Down to
navigate, Return to choose, and Escape to dismiss. Open the branch picker with ⌃⇧B in a new-session draft. There is no shortcut-hint footer.
Git discovery and checkout preparation run in the background.
Both Chat and Terminal use the prepared working directory. Non-Git folders show a gray Git icon with a red cross and a “No Git repository”
tooltip. This indicator is not clickable, and there is no branch picker or worktree checkbox. Without a new worktree, selecting a different branch switches
the existing checkout only when it is clean; Git also refuses branches checked out
elsewhere. With New worktree, Del creates a unique `del/session-<UUID>` branch from
the selected commit without changing the source checkout. Uncommitted source changes
are not copied. An unborn repository needs its first commit before worktree creation.

Del-owned worktrees live in `<Del data>/worktrees/<UUID>/checkout`, with an ownership
manifest beside the checkout. Failed provider-start retries reuse that draft's
worktree. Saved sessions reopen their exact directory and still appear under the
original project. Closing, unloading, archiving, or deleting a session does not
remove Git work. Right-click a project and choose **Remove unused clean worktrees**
for explicit cleanup. Cleanup removes only Del-owned checkouts with no saved-session
or pending-draft references, no changed/untracked/ignored files, and no commits since
creation. Branches are always preserved. Worktrees containing work remain on disk
for normal Git review and cleanup.

The sidebar starts with four equal-sized buttons: **Search**, **New session (+)**,
**Keep Mac awake**, and **Memory usage**. Keep-awake uses a neutral hover-colored
background and animated yellow steam while enabled. A small right inset keeps
the last toolbar button clear of the sidebar divider.

Memory usage opens a compact popover below its button, clear of the top tabs, with a pointer
on the top. It lists loaded chats and terminals by process-tree RAM usage,
refreshing every three seconds while open. Each row shows a fading session title
and its RAM usage at the far right; click the title to open the session. Delete and
unload icons appear to its right only on hover, taking no space while hidden.
Hover updates run layout before drawing the controls. Unload releases the live session
while retaining its history; delete uses the existing permanent-delete action.
Up to four rows are visible, with a bottom fade and scrolling without a scrollbar.

Memory figures measure process RSS, not per-chat allocations inside Del. Providers
that share a server can show the same process usage for several chats, and
unloading one chat may not release that server's RAM. Unavailable measurements
appear as a dash rather than zero.

Opening a saved chat or terminal from the sidebar uses one temporary preview tab. Opening
another replaces that preview in place. Double-click the sidebar session or its
tab, send/queue a prompt, type or paste into a terminal, or move the tab to keep it open. Preview titles are
muted and italic. New chats remain open, and replaced previews retain their
conversation history and composer drafts. Double-clicking a chat or terminal
entry in the sidebar keeps its preview tab open.

**⌘K** opens conversation search. Search message text or titles across saved Codex,
Claude, and OpenCode chats, including terminal conversations and archived sessions. Use
**↑/↓** to select a match, **Enter** (or click) to open it, and **Escape** to close.
Add **/c** for chats or **/t** for terminals to limit results; use either
filter by itself to list its sessions. Tab shortcut badges stay hidden while
the palette is open. The palette is centered over the conversation pane and
stays aligned as sidebars resize. A dimmed backdrop, a short fade-and-slide
entrance, and a 140 ms fade-out keep opening and closing smooth. Reduce Motion
shows and dismisses it immediately. Clicking outside also dismisses the palette.
Inserted skill tokens use blue text and icons. Command suggestions show
navigation hints without argument-placeholder text.
The search footer places navigation, open, and close hints on the left and **/c /t Filter**
on the right. Result type labels (Chat or Terminal) align with the Enter icon.

Assistant previews render Markdown and highlight the matching words. Opening a
result keeps its tab, scrolls to the matched text, and briefly highlights it,
including within Markdown replies.

Search reads local provider transcripts on a background worker and caches their
message text in memory, refreshing changed files when the palette opens. It does
not submit agent prompts or open terminals to index history. OpenCode exports
use a private standalone server rather than the shared background service. Sent messages and streamed
replies are included from their loaded views. Results show the agent icon in a
compact palette that grows with results up to a maximum height. Terminal matches
open the actual terminal and resume its original conversation when needed. Del
waits for restored output to settle, then scrolls to and highlights the passage
when it is available in the terminal scrollback. Codex uses inline terminal mode
to preserve that scrollback. If the CLI has not restored the passage to its
buffer, Del reports that limitation while keeping the terminal open. Browser
profiles and conversation history are retained. Plain shell terminals have
searchable titles; provider-backed terminals also have searchable messages.

Claude chats reload their saved local transcript when reopened, including messages,
reasoning, and tool results. Use **Unload** in a session’s context menu to close
its tab and release its live chat process or terminal. The session stays in the
sidebar with its history, title, and pin intact; selecting it resumes the same
conversation. Unloading a running session also stops its current work.

- Choose a provider, model, and project folder in a new-session composer.
- Each New Session action opens an independent draft tab; existing drafts are
  preserved. The top tab bar scrolls when tabs overflow, fades at clipped edges,
  and reveals the selected tab. Empty “New session” rows expose a hover × in the
  left sidebar so drafts can be closed there as well as from the top tab bar.
  Dragged tabs stay inside the tab strip, beyond the left-sidebar toggle.
- The project folder and Chat/Terminal controls sit in a matching footer beneath
  the input, keeping workspace selection distinct from the message area.
- **Chat / Terminal** selects how the first prompt will run. Choosing Terminal
  keeps the initial composer visible; the terminal opens after submission.
- A session's project folder is fixed after creation. Its folder label becomes
  informational rather than a clickable folder picker.
- Chat renders streaming answers, reasoning, tool activity, diffs, and
  approval requests. The message rail appears after six user messages and provides
  jump-to-prompt navigation. Hover magnifies nearby ticks, but only the hovered
  tick is white; neighbors stay gray. Away from the rail, only the current-message
  tick is highlighted. Moving onto a different tick emits native haptic feedback
  on supported macOS hardware.
- The transcript uses left-aligned user message cards, unboxed assistant prose, and
  compact expandable tool rows. Consecutive tools share a group immediately, even
  while running. Its header shows the current action and any failure/decline count;
  summaries show only the first input line and truncate to the available width,
  including while shimmering. Tool headers have no leading icons. Expanding
  smoothly reveals calls and their full input/output aligned with the chat text,
  without a nested left indent; diffs remain available through disclosure.
  Matching edit/file events share one row, and empty diff counts are omitted. See
  [Codex chat reference](docs/codex-chat-reference.md) for source research and scope.
- A shimmering “Working for…” label sits beneath the latest user message and
  scrolls with it. There is no duplicate Working/Done indicator above the composer.
  Reduce Motion keeps the label static.
- While a task is working, keep its intermediate updates, tools, and peer exchanges
  visible. Once the final answer has finished, fold the earlier activity under an
  expandable **Worked for…** row; the final answer stays outside it. The reveal
  animates smoothly in both directions and respects Reduce Motion. Elapsed time
  spans peer continuations, is saved with replies, and is recovered from older
  Codex/Claude transcript timestamps when available; missing timing is never invented.
  Search and navigation reveal hidden activity before jumping to it. Peer continuations
  belong to the original request, so waiting for workers does not collapse early.
- Hover the final reply of a completed response to reveal Copy and its local reply time
  in 24-hour `HH:MM` format. Intermediate updates and streaming replies have no copy footer.
  Reply times are saved across reopening; older Codex and Claude replies recover their
  timestamps from local transcripts. The reply-time journal stores a short prefix and
  fingerprint of each reply rather than its text; older journals stay readable. If no timestamp exists, only Copy is shown.
  The 13 px overlapping-squares Copy icon and 11 px time text fade in and out over
  160 ms on hover, without shifting the transcript. The square-proportioned glyph
  aligns with the reply text’s left edge and keeps a 24 px click target. Copy saves
  the full reply to the clipboard and shows a checkmark for 1.5 seconds; clicking
  again restarts that confirmation.
- Codex file citations render as clickable filenames in live and saved replies,
  including Side chats. Code examples remain literal.
- Local file links in replies open in Del’s file viewer; relative links resolve
  against the conversation’s project folder. Only http, https, and mailto links
  open in your browser or mail app; other link types (custom app links, network
  shares, remote `file://` hosts) are not opened, and Del shows a notice instead.
- Assistant text appears as it arrives, without streaming fades or animation
  delays. Deltas append to a persistent Markdown view; already-received adjacent
  chunks coalesce without waiting or crossing tool/turn boundaries. Native Markdown
  rendering preserves text selection.
- Opening a saved conversation shows a transcript shimmer while it loads,
  without a loading text line, with static placeholders when Reduce Motion is enabled.
- Sessions have tabs and sidebar entries, with rename, pin, unread, archive,
  restore, and permanent-delete actions. Terminal entries share the session
  context menu. Chat titles are generated in the background by the session’s
  provider as soon as the first message is sent, without waiting for the reply,
  then refreshed every six user turns. The title prompt uses OpenCode’s published instructions: a natural
  three-to-five-word phrase with specific identifying details. Codex uses
  GPT-5.6 Luna with reasoning disabled; Claude uses Haiku 5.5 with thinking
  disabled. OpenCode uses its native transient generation API with the session's
  selected model, preserving conversation history. These requests use the existing provider login and do not
  change the conversation’s model. The tab and sidebar title shimmer while a
  name is being generated; sidebar titles use a muted base and bright sweep that
  remains visible on selected rows. Titles persist across restarts, manual
  renames take precedence, and failed requests retain the first-message fallback.
  The shared title-generation prompt lives in
  [`crates/agent/src/titles.rs`](crates/agent/src/titles.rs) as `INSTRUCTIONS`, loading `title_prompt.txt` (OpenCode, MIT).
  Title generation consumes provider usage, including OpenCode free-model quotas.
  Empty drafts remain “New session.” Terminal titles follow provider metadata.
  Archived chat and terminal rows show Restore and Permanently delete icons on hover.
  The delete icon stays red, including when hovered.
- Running chats show a thin rotating ring with a fading tail in their sidebar row and top tab. Terminal
  sessions show it when their agent reports Working. Hover hides the spinner and
  reveals the usual actions in the same slot.
- While working, the spinner replaces the unread dot in the same trailing slot;
  unread state is preserved and the dot returns after work finishes if still unread.
- A blue dot at the far right marks unread conversations, including background
  replies that finish before being opened. A selected chat also becomes unread
  when its reply finishes while Del’s window is inactive. Opening the chat, returning focus to its window, or sending another prompt clears
  the dot. Completion in the actively viewed chat stays read. Hover temporarily hides it so the action icons keep their usual right alignment.
- Both sidebars resize from their panel boundaries. The right tab row has a thin,
  floating divider; the content area stays borderless. The left sidebar only
  resizes at its boundary; its empty space does not drag
  the macOS window. Hover highlights slide between session/menu rows, and the
  keep-awake control animates yellow steam while active.
- Session titles use the available sidebar width and fade at the edge instead
  of ending in an ellipsis. Hovering reserves extra action space so terminal
  pin/close buttons do not overlap their titles.
  Sidebar rows keep their minimum height and single-line titles in
  short or narrow windows; the list scrolls instead of compressing rows. Broad
  top/bottom fades mark overflow without covering the fixed Settings controls.
- Agent questions appear inside the active conversation, above its input, with
  answer choices and a free-text field. Each question keeps its own draft while
  switching sessions; terminal questions stay with their terminal session.
- General application, session-start, model-loading, and message-delivery failures
  appear in dismissible toasts. Failed messages remain available to retry or edit.
  OpenCode quota errors show a readable message and the provider's retry hint
  when available, rather than raw JSON. Provider usage limits still apply.

## Models and reasoning

The picker uses provider-advertised choices, not a hardcoded list of model names.
All visible catalog entries remain accessible in a bounded scrolling list, without
a scrollbar; the bottom fades while more entries are below. Refresh retries
catalog discovery. Availability depends on the installed CLI, agent, and account.
The picker opens with a **Search models…** field. It fuzzy-filters names and
provider IDs, ignoring case, spaces, and punctuation; clear it to restore all
choices. Opening the picker again starts with an empty search. Use **↑/↓** to
navigate results, **Enter** to select, and **Escape** to close. The provider
default is labeled **Default**.

| Provider | Model discovery and controls |
| --- | --- |
| Codex | Paginated app-server `model/list`; advertised reasoning levels and priority/fast tier |
| Claude | CLI initialization metadata; advertised effort levels and Fast support, applied with `--effort` and session-scoped `--settings` |
| OpenCode 2.x | Native model catalog and per-model effort variants; white provider accent |

Selecting a model opens its reasoning controls. Slider stops correspond to
advertised effort levels, and the accent follows the selected provider (including
Codex blue, Claude orange, and OpenCode white). Use **←/→** in the reasoning controls to move one
level at a time, stopping at either endpoint. Fast mode is enabled only when the model
advertises support.
The model pill immediately shows the effective effort, including the catalog
default before any adjustment. Reset clears explicit effort/fast settings and refreshes the catalog.
Changing effort by dragging, clicking a stop, or using arrow keys emits one native
haptic tick per level transition on supported macOS hardware. Movement within the
same level stays silent; devices without haptic support receive no feedback.

Model, effort, and service-tier selections are stored with the conversation.
Existing chat settings apply on subsequent prompts through the backend's
reconfiguration path; they do not rewrite a turn already in flight. Changing a
model clears incompatible effort/tier choices. Provider changes use a new draft.
Catalog discovery submits no prompt, but can start a CLI process. ACP is no longer
offered in the provider picker or Settings; its backend remains for legacy records.

### Skills and slash commands

Type **/** at the start of the composer to browse the selected harness's skills
and commands. Type to filter, use **↑/↓** to navigate, **Enter** or **Tab** to
select a command, and **Escape** to dismiss. Selected skills become blue inline
tokens; their full provider identity survives editing, undo, and switching drafts.
Type **/** again after a selected skill to add another; each selection stays a
separate blue token. Select **/goal** before or after adding skills to combine them
with goal mode; the selected skills remain requirements of the native goal.
Add your instructions after the skills, then send them together.
Selections survive queued-message editing too. Claude Code supports up to six
skills per message; Codex sends every selected skill as a native input item.
The bounded menu shows descriptions and argument hints, with skills first.
`/skills` narrows the menu to skills. Reopening it refreshes discovery.

- **Codex:** uses app-server `skills/list`, including enabled global, project,
  system, and plugin skills returned by the CLI. Selected skills are sent as
  native `skill` input items with their exact paths, alongside text and attachments.
- **Claude Code:** uses the CLI's initialization command catalog and native
  stream-json slash-command execution. User, project, and local settings load
  normally, so personal and installed plugin skills are available too.
- **OpenCode 2.x:** uses native skill IDs and the native command catalog. Selected skills
  travel as structured prompt items alongside text and file attachments. OpenCode
  owns execution and persistence. Its inspected 2.1.4 API exposes no native goals
  or Fast tier, so Del does not advertise those capabilities for OpenCode. See the
  [adapter notes](crates/opencode/README.md) for installation and verification.
- **Fast:** `/fast`, `/fast on`, and `/fast off` use the same saved session
  setting as the model picker. Codex uses its advertised service tier; Claude
  uses `fastMode` in session-scoped settings. Only supported models can enable
  it; account access and usage charges remain controlled by the provider.
- **Goals:** select `/goal` to enter goal mode: the composer shows a Goal control
  and asks for a measurable objective. Click Goal to cancel that mode without
  losing the draft. Sending shows **Sent as goal** on the message and a compact
  status bar above the composer with the objective, elapsed time, and expandable
  details. The bar follows the selected session and restores from provider state.
  Codex offers native pause/resume and clear controls; Claude offers native clear
  (there is no Claude pause/resume command). Clear removes only the goal.
  `/goal <objective>` and provider-supported lifecycle commands still work.
  Completion is read from native goal state, independently of individual turns.
  Codex uses goal RPCs and notifications; Claude follows native transcript status
  records without sending status prompts. The harness owns persistence and
  automatic continuation; Del keeps streaming, queues, and Stop connected.
  Claude chats also receive a session-scoped `goal` MCP tool: the agent can inspect,
  set, resume (including completed goals), or clear its native goal when asked.
  Changes requested during a tool call queue until the current turn ends; the card
  and timer update from native confirmation, never from assistant prose.

The shared `AgentBackend::list_commands`, `invoke_command`, and `control_goal` contracts own
discovery and execution. The composer consumes provider-neutral command and goal state;
new harnesses can implement that contract without adding provider branches to
the menu. Backends without command support return an empty catalog. ACP adapters
currently use that default rather than advertising commands they cannot execute.

## Follow-up queue and attachments

While a reply runs, **Enter** queues the current draft. **⌘ + Enter** submits
it as steering input; with an empty composer, it submits the top queued item.
Completed turns advance remaining follow-ups in order. A busy provider leaves
an undelivered message waiting instead of displaying a failed-send error or
incorrectly changing the running status to Done. If completion arrives before
the provider releases its send slot, Del retries the undelivered message
automatically. Stop pauses pending retries too; steering preserves the remaining
queue and its order.

Queue rows have **Steer**, **Delete**, and **Edit** actions. Edit returns text and
attachments to the composer; an existing draft is preserved as another queued item.
Drag from the left edge to reorder, without an extra hover icon. Rows have subtle
separators and a surface just darker than the composer. Around four rows fit
before the list scrolls, with a bottom fade and no scrollbar.

Attach any regular file using the plus button, paste, or file drop: PDFs,
spreadsheets, Markdown, documents, images, video, audio, archives, and unknown
formats are accepted. Del copies each file into its own attachment directory,
preserving the original name and bytes without collisions. Imports no longer
apply the old 20 MiB image-only limit. Unreadable files report individual errors
while other files in the selection are retained.

**⌘V** pastes copied images or Finder files into the composer; ordinary text
paste still works normally on both new-session and conversation pages. Clipboard
bitmap formats such as TIFF and BMP are converted to PNG in the background.
PNG, JPEG, and WebP show thumbnails of the actual image. Other formats show one
file-type icon, a compact filename, and extension.
Sent messages show a muted attachment count (for example, “4 attachments”)
instead of listing filenames, combining images and files into one total.
Cards form a single horizontal scrolling row with no scrollbar. Wide, subtle
edge fades appear only on sides with more offscreen content. Each card reveals
a bright **×** on hover that remains visible while hovered or pressed. Click
a card to preview its original file in the Files sidebar. Attachments survive
queue editing, reordering, and retry. Permanently deleting a session also removes
its queued follow-ups and the imported copies they still reference. Codex receives local image references;
Claude and ACP receive encoded image content. Other files
are supplied as local paths for the harness's tools to read. This does not embed
PDF/spreadsheet contents in the prompt or transcode video; interpretation and
access depend on the harness's tools and permissions. Terminal-mode opening
prompts include paths for all attached files, including images.

Queues persist per conversation. Known busy rejections are recoverable;
uncertain deliveries interrupted before acknowledgement require review rather
than automatic replay. Other delivery failures keep the item and its error.
A stopped or failed turn does not automatically advance the queue.

Steering differs by harness: Codex and OpenCode support active-turn steering; Claude's
stream-json interface may absorb input into the current response or queue another
response. Del follows Claude's input acknowledgements so either case releases
the remaining follow-up queue when the response finishes.
ACP steering is currently unsupported and leaves the message available to retry.
Steering during streaming keeps the current assistant answer in one block.
Sent steering messages use a small muted yellow queue icon beside the text,
without a separate “steer” heading.

### Voice input

Click the microphone beside Send to dictate into the composer using Apple's
[Speech framework](https://developer.apple.com/documentation/speech). Click the
red stop button to finish, then review or edit the text and send it normally.
Completed phrases stay in the draft across pauses; only the current phrase is
revised. Incoming text reveals over 80 ms (instant with Reduce Motion). Send and
keyboard submission are disabled during dictation, and the caret stays hidden.
Stop dictation before sending; clicking into the composer stops capture and
restores editing, including any words still appearing.

There is no Del recording-duration limit: Apple's bounded recognition requests
renew in the background while capture stays active. Editing, switching chats,
opening Settings, or quitting stops capture. Del does not save audio. Apple
service availability and quotas still apply.

Native bridges target macOS 11 or later even when built with a newer SDK, retaining
runtime availability checks. Microphone capture uses the long-supported audio tap
API; optional punctuation is enabled only on macOS 13 and later.

Use the signed `del.app` build for voice input. On first use, macOS asks for
Microphone and Speech Recognition access. Denied access can be changed in
**System Settings → Privacy & Security**. Raw `cargo run` builds show a helpful
error instead of requesting permissions without bundle metadata.

Recognition uses the current macOS locale and runs on-device when Apple supports
it for that locale; otherwise Apple's speech service may require a connection.
No Codex, Claude, or third-party transcription API is used.

## Project peer conversations

Agents in the same Del project can discover each other, read recorded chat context,
and exchange requests and replies across providers. Sibling worktrees use their
parent project. A peer can ask for information or edits without changing the
recipient's account, model, or permissions. Incoming requests are actionable tasks,
including new assignments after a previous task completes. Agents carry them out
and report back without requiring the user to resend them; standing user constraints
and existing permissions still apply. Each exchange appears as a centered
`Current agent ⇄ Other agent` row with provider logos and clickable names, without
message text, status labels, or a card background. Exchanges also appear in conversation
navigation and activity indicators. Failed or uncertain deliveries retain retry
and dismiss actions in the row's context menu.

Peer messages are durable and run before queued user messages at a safe input
boundary. **Finishing a turn or replying to a peer does not finish the original
task.** The agent continues that work and explicitly completes it before Del
releases queued user messages. Waiting and paused work remain visible. Uncertain
delivery after a crash requires review before retrying.

Chat agents can create new chat peers with `spawn_session`. New peers inherit the
caller’s working directory, project, and permissions. When using the same provider,
they also inherit its account, model, effort, and service tier; a different provider
uses its default account and model. The first task is a durable peer request, so
its reply returns to the requesting chat without a second dispatch. An agent can
create several workers, wait for their replies, and then finish its original task.
Creating a worker does not make either chat an orchestrator or change your selected
conversation. Each spawn call creates a session; inspect the session list before
retrying a call whose outcome is unknown.

Tools: `spawn_session`, `list_sessions`, `get_session_summary`, `read_session`, `send_to_session`,
`reply_to_session`, and `session_work`. Context reads are paginated and exclude
private reasoning. Side chats retain mailbox routing after restoration or provider
changes. Plain shells are excluded. Terminal agents use a cooperative inbox and
explicit idle reports; their native CLI input queue remains owned by the CLI.
For example, ask “Read the context of the API chat in this project” or “Ask the
Tests chat to check your changes and report back.” Agents use the tools to find
and contact that session; there is no required orchestrator or parent/child role.
See [peer sessions](docs/peer-sessions.md) for details and recovery behavior.

## Files, terminals, and browser

The right sidebar starts with **Files**, **Changes**, **Terminal**, and **+**.
Use **+** for a new tool or browser page. Each terminal occupies its own tab in
that row; choosing Terminal from a new tab starts an independent shell. Closing
one terminal leaves the others running. Drag tabs to reorder them; each session
remembers its sidebar order. Drag a tool between the session row and right-sidebar
row to move it without closing its shell, browser, editor, or side chat. The original
conversation stays fixed. Both rows use the same compact 128-point tab width, including during transfers.
Command-T opens in the last-used zone, and newly selected tabs scroll into view.
Command-W closes only that zone’s inner tab; Command-Shift-W closes the top-level session.
Side chats use the same provider button and searchable model picker as main chat,
including effort, Fast mode, and reset controls. Account names appear in the
provider submenu; changes apply only to that side chat.
The launcher centers its four options in a compact vertical stack below the URL bar,
with shortcuts beside neutral icons. Card and URL-bar backgrounds are slightly
lighter to remain visible against the page, with no divider below the URL bar.
Entering either row opens a live
insertion gap, and neighboring tabs move aside
as you change slots. Dragging tracks the pointer horizontally, locked to the tab
row with no vertical movement or magnetic effects. Top-level and inner tabs emit
one native haptic tick when their reorder target changes, matching sidebar session
reordering; movement within the same slot stays silent. The darker, full-width preview stays attached to the pointer at its original grab offset,
including after crossing into the other zone. Keep dragging to rearrange there; the
tab settles into its destination slot only when you release.
The source slot collapses as soon as the tab leaves, so its plus button follows
the remaining tabs. Both rows use the same background and small hover-close
controls, and fade gradually at clipped edges. Both
rows remember their tabs and placement across restarts. Blank tabs stay open until closed, and closing the
last tab leaves a fresh **New tab** with the shared launcher. Tool icons match
between the session tabs, sidebar tabs, and launcher. The file tree loads
folders on demand, supports optional dotfiles, and excludes generated
`target`/`node_modules` trees. Selection highlights fit the file label. The tree
and preview have a draggable split. The file tree has no edge overlays hiding its content.

Text files are editable with line numbers and syntax highlighting. Use **Save** or
**Cmd-S** to save, **Revert** to discard edits, and **Reload** to read from disk.
Unsaved drafts survive file switches and app restarts. Saving detects external
changes and keeps your draft instead of overwriting them. Markdown supports
**Markdown / Preview** modes. Text files are capped at 512 KiB.

**Changes** automatically refreshes staged, unstaged, and untracked Git changes
every two seconds while visible, with selectable diffs and no action toolbar. Unchanged diffs retain
their selection and scroll position during refreshes.

On macOS, PDFs render with PDFKit in a continuous vertical scroll without floating
page-arrow controls. Word documents (`.doc`/`.docx`) render inside the sidebar using
[Quick Look](https://developer.apple.com/documentation/quicklookui/qlpreviewview),
with native document scrolling. Supported spreadsheets, presentations, rich text,
and image formats use the same viewer. Document previews bypass the plain-text
size limit and stay local. Rendering capabilities depend on the installed macOS
Quick Look providers; encrypted or damaged files may not be previewable.
Expanded folders, selected file, dotfile preference, split height, and Markdown
mode are saved per chat. Switching panes, opening Settings, or closing the sidebar
hides native previews; closing the preview releases its document.

Workspace terminals have independent tabs with plus and close controls, like the
browser. Switching tabs keeps their shells running; closing a tab stops only its
shell. Tab layout is saved per chat, with fresh shells created after restarting.

The workspace terminal starts the shell selected by `$SHELL` as a login shell,
falling back to `/bin/zsh`. Its terminal engine is `alacritty_terminal`, rendered
inside Del with GPUI. The cursor is hidden when the terminal or app loses focus.

Terminals use a real PTY, support shell and agent sessions, and retain colored
terminal cells while letting the normal chat surface show through their default
background. Saved Codex/Claude/OpenCode terminal sessions reopen the exact provider
conversation ID without replaying the initial prompt. Legacy Codex identity
recovery proceeds only when one saved conversation matches its launch time and
workspace. Unresolved identity is reported rather than choosing the latest chat.
New OpenCode terminals create fresh native sessions. Del disables OpenCode's
shared inner tabs for its launches, preventing previous sessions from appearing
inside a new terminal; the user's on-disk OpenCode configuration is unchanged.

Built-in Codex and Claude terminal launches automatically trust the workspace
opened in Del, including resumed sessions. Codex receives a launch-scoped
[`projects.<path>.trust_level`](https://developers.openai.com/codex/config-reference/)
override and an explicit `--cd` so login shell startup cannot change its workspace.
Both providers' recognized startup trust dialogs are confirmed as soon as they
appear (100 ms polling), with short retries only while the exact workspace prompt
remains visible. Confirmation is limited to startup (two minutes) and stops once
the prompt disappears. Unrecognized dialogs stay interactive.
Shell/custom commands and arbitrary ACP agents keep their own trust setup.

Del starts and reopens Codex, Claude, and OpenCode sessions with full access by default.
Codex chat uses `approvalPolicy: never` and `sandbox: danger-full-access`;
Codex terminals use `--dangerously-bypass-approvals-and-sandbox` (YOLO mode).
Claude chat uses `bypassPermissions` with `--dangerously-skip-permissions`,
and Claude terminals use the same bypass flag. OpenCode chat uses native allow
rules and its terminal uses `--auto`. These modes allow file changes
and command execution without tool-approval prompts. Existing saved chats adopt
the setting when reopened; there is currently no in-chat permission toggle.
ACP sessions and custom shell commands keep their existing permission behavior.

Del's MCP bridge provides scoped session coordination, status, notifications,
and user questions for supported terminal agents. Browser tools are also available
to Codex, Claude, and OpenCode chat sessions. Launch-specific credentials
and MCP configuration are kept separate from saved launch history. See
[the MCP documentation](crates/mcp/README.md) for tools, transport, and scope.

### Browser

On macOS 14 or later, the Browser pane embeds WebKit with an address field,
back/forward navigation, reload/stop, and multiple tabs. HTTP(S) and localhost
pages open inside Del. Browser zoom follows the app's zoom setting. The sidebar
remains resizable while a browser tab is open. Pop-up links currently open in the
same tab; downloads and browser extensions are not supported.

Each saved chat or terminal session has its own persistent WebKit profile.
Its tabs, cookies, site storage, and sign-ins remain across tab closing, session
unload, archiving, and app restarts. Different sessions do not share sign-ins.
Only **Permanently delete** removes that session's browser profile; normal site
expiry and server-side sign-outs still apply. If WebKit is still releasing a
profile, its website data is cleared and remaining cleanup retries at next launch.
Startup cleanup initializes WebKit first, so pending deletions cannot crash the app
before its first browser view opens.

Codex, Claude, and OpenCode agents can use Del's authenticated `browser` MCP tool to discover,
create, select, close, and navigate tabs, inspect page text/elements, click, fill,
send keys, evaluate page JavaScript, and take screenshots. These actions target the
same visible browser and saved profiles. Del is the default browser for agent
requests; external browsers are used only when explicitly requested by the user.
Agents can use only their own session's browser and the browsers of sessions they
spawned or that spawned them. Claude and Codex chats each get a browser credential
bound to their own chat. OpenCode chats each own a server and credential, keeping
browser access and peer identity separate even in the same workspace. Actions on a
session you aren't viewing run in its hidden browser without changing your
selection. When an agent uses a browser tab that isn't visible, a small draggable
page preview appears on the right. It refreshes from the existing page without
opening the sidebar or changing the browser's viewport. Click the preview to
reveal that same tab in the sidebar; drag it to reposition it within the window.
While the preview is updating, its page keeps rendering invisibly so sites such
as YouTube can finish loading instead of remaining on a skeleton screen. The
hidden native view cannot intercept clicks, expose accessibility controls, or
show page dialogs. Rendering pauses again when preview sampling stops.
Closing the sidebar retains the page's last viewport for the preview instead of
shrinking it with the panel animation.
The preview image clips to rounded corners and has no URL header. A translucent
× with a locally blurred page backdrop floats over its top-right corner.
Each session keeps its own preview and position: switching away hides it, and
returning restores it unless dismissed. Background sessions never show their
preview over the current session.
The × dismisses only the preview and stays dismissed for subsequent actions on
that tab. Another browser tab can show a new preview. List/close-tab requests do
not create previews. An open Browser pane in either zone hides floating previews,
even when it displays a different browser tab, so native browser content cannot
cover or clip the thumbnail. Hiding the Browser pane restores the preview unless
it was dismissed. The preview fits the page’s aspect ratio without an outer frame
or black padding.
Page dialogs name their origin, and repeated dialogs are blocked. Chat
credentials expose only browser operations (plus goal controls for Claude chats).
Page content is treated as untrusted data. ACP browser-tool injection is not yet
implemented.

## App updates

Public Apple Silicon builds check GitHub's latest stable release at startup,
when the selected session/tab changes, and when the window becomes active.
Automatic checks are limited to once every 30 minutes and never block sessions;
manual checks run immediately.
Discovery reads the release-hosted `latest.json` manifest, avoiding GitHub API quotas.
Releases whose version Del cannot compare (such as pre-release tags) are ignored.
The control above **Settings** also lets you check manually and retry failures.
Failed restarts keep the staged download so you can retry without downloading again.

Click **Update to …** to download the existing release DMG. The control shows
percentage progress, then verifies its SHA-256 sidecar, bundle identity,
version, and code signature before staging the app beside the current installation.
**Restart to update** offers **Restart now** and **Later**. **Restart now** verifies
and installs in the background (the control shows **Installing update…**). Later keeps the staged
update ready for this run; quitting without applying it discards the staged copy.
Restart stops running agents/terminals through the normal quit flow and preserves
saved sessions. A failed replacement restores the previous app; its bundle is
retained in `.del-retired` so a running process never loses its signed resources;
retired bundles and abandoned staging folders more than a day old are removed at a
later launch.

Updates require a writable installation directory. Move a read-only or
translocated app to Applications first. Local development bundles and raw Cargo
runs do not self-update or replace their development signing identity. Releases
continue to use the existing DMG/checksum publishing mechanism and its documented
ad-hoc signing status. This feature becomes available after installing a build
that includes it; older builds need that first upgrade manually.

## Harness updates

Del checks the selected provider's installed harness in the background at startup
and every 30 minutes. Terminal launches also request a background check. Sending,
resuming, and model discovery never wait for updates. Successful checks are cached
for 30 minutes; concurrent checks share an updater.

- Global npm packages update through their owning prefix.
- Homebrew packages update through their owning formula or cask.
- Native Claude Code installations use `claude update`.
- Installed ACP agents use the same npm/Homebrew ownership detection.
- Custom or unmanaged binaries are left alone; registry install hints are not run.

Update failures are logged without blocking chat. Failed checks back off
(5 minutes, doubling up to 6 hours) instead of retrying on every change. Del does
not upgrade a harness while a chat session or terminal using it is open; it tries
again every 5 minutes. Recognized successful CLI
self-update exits reopen the saved conversation automatically. This updates
supported harness installations, not Del itself or arbitrary applications.

Del also keeps one unused chat session ready after provider discovery at startup.
It matches the selected provider, model, folder, reasoning, service tier, and
permission settings, replenishes after use, and refreshes after 30 minutes. No
prompt is sent and no sidebar chat is created until the session is used. Changing
chat settings replaces the standby; active conversations are never recycled.
Preparation failures retry in the background and normal sends can still start a
session immediately. Startup logs record standby preparation, reuse, and first
prompt acceptance times; model-side first-token latency can still vary.

## Settings and shortcuts

Sidebar and context-menu hover highlights move between rows in 90 ms with smooth
easing. Reduce Motion keeps these transitions instant.

Del opens as a centered window at roughly 70% of the available display width and
76% of its height, with a minimum size of **960 × 640 logical pixels**.

Zoom scales the shell, chat, controls, file panels, and embedded terminal together
in 10% steps, bounded to 70–200%. The setting is saved in `zoom.json` next to the
session index and restored on launch. Keyboard shortcuts work while the composer
or terminal has focus.

Settings has **Appearance**, **Providers**, and **Keybinds** navigation,
with icons aligned to the left of their labels. Appearance includes persisted
Light/Dark appearance; Providers shows sign-in status and account controls.
**Keybinds** lists app shortcuts and contextual controls, grouped by area; it is read-only.
Light mode uses a white workspace, off-white chrome, and slightly darker `#F9F9F9`
composer cards. Menus, model and effort pickers, command suggestions, search, and
Settings share that light surface. The shell opacity setting is fixed at 85%;
light surfaces remain opaque to preserve their colors. Settings uses the same
fade-and-slide entrance as search, with a strongly blurred and dimmed backdrop.
**Escape**, Done, or clicking outside closes Settings and returns to the workspace.
Top-tab titles use an edge fade instead of ellipses and fill the available space
up to the action slot, including the close-button space while idle and not hovered.
The text line preserves descenders; hidden unread markers do not reserve extra spacing.

Hold Command to see numeric hints on the top tabs. There are no numeric sidebar
session shortcuts.

| Shortcut | Action |
| --- | --- |
| Command-1–9 | Select a top tab by position |
| Command-N | New session |
| Command-T / Control-T | New inner tab in the last-used zone |
| Command-K | Open or close conversation search |
| Command-Shift-W | Close the top-level session tab |
| Command-W | Close the active inner tab in the focused zone |
| Control-Shift-G | Open Changes in the focused zone |
| Command-P | Open Files in the focused zone |
| Option-Command-S | Open Side chat in the focused zone |
| Command-+ (Command-Shift-=) | Zoom the entire interface in |
| Command-- | Zoom the entire interface out |
| Command-0 | Reset zoom to 100% |
| Control-Tab / Control-Shift-Tab | Next / previous tab |
| Command-Shift-] / Command-Shift-[ | Next / previous tab |
| Command-L | Focus composer |
| Enter | Send when idle; queue while running |
| Shift-Enter | Insert a newline |
| Command-Enter | Steer the draft, or submit the first queued item |
| Command-. | Stop the active chat turn |
| Command-B | Toggle the left sidebar |
| Command-Backslash / Command-Shift-B | Toggle the file sidebar |
| Command-J / Control-Backtick | Open a new terminal in the last-used zone |
| Command-Comma | Settings |
| Escape in Settings | Return to the previous workspace view |
| Command-Q | Quit |

The Settings → Keybinds page lists these shortcuts plus file-editor, terminal,
search, and model-picker controls. Keybindings are not editable yet.

## Local data

Del uses the platform user-data directory (`~/Library/Application Support/del`
on macOS). `DEL_DATA_DIR` overrides application data for tests/development.
Provider credentials and provider-owned transcripts remain managed by their CLIs.

| Path within Del's data directory | Contents |
| --- | --- |
| `sessions.jsonl` | Chat session index and configuration |
| `sessions.peers.json` | Project membership, peer receipts, work state, restored Side chat routing |
| `sidebar.json` | Local names, shared session order, pins, archive/unread metadata |
| `projects.json` | Directory identities, display names, collapse state, and last directory |
| `worktrees/` | Managed Git checkouts and ownership manifests |
| `terminals.json` | Terminal launches and provider conversation identities |
| `followups/` | Per-conversation queued messages and delivery state |
| `attachments/` | Local file copies used by drafts and queued messages |
| `right-panels.json` | Per-session file/terminal/browser state, profile IDs, and tabs |
| `browser-profile-deletions.json` | Pending WebKit profile cleanup |
| `sidebar-widths.json` | Sidebar sizes |
| `appearance.json` | Light/Dark preference |
| `zoom.json` | Interface zoom percentage |
| `acp-agents.json` | ACP registry overrides |

WebKit owns website data under its app-specific Library directories;
`right-panels.json` stores the UUID connecting each session to its profile.
Browser data is not stored in the conversation transcript.

The macOS development-signing identity uses the fixed directory described above.
See [ACP configuration](crates/acp/README.md) for custom installed agents.

## Codebase

| Crate | Responsibility |
| --- | --- |
| `app` | GPUI shell, composer, queue, settings, file viewer, terminal host, updates |
| `agent` | Provider-neutral sessions, events, project peer mailbox, work lifecycle, persistence |
| `codex` | Codex app-server adapter |
| `claude` | Claude streaming CLI adapter |
| `acp` | ACP registry and protocol adapter |
| `opencode` | OpenCode 2.x native server adapter |
| `terminal` | PTY lifecycle, input, emulator state, and rendering |
| `mcp` | Authenticated peer/browser tools, session bridge, and stdio proxy |
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
Browser preview regressions cover session isolation, dismissal, dragging,
page-aspect sizing, localized blur, and suppression over open native browser panes.
UI regression tests also cover preview tabs, unload/resume, hover clipping and
collapse fading, incremental streamed Markdown, reasoning-step boundaries, and zoom shortcuts
with composer or terminal focus. Permission tests cover new and resumed sessions,
terminal bypass flags, and Codex permission changes on subsequent turns.
Project regressions cover directory inheritance, archive/restore grouping, worktree
ownership and cleanup, full-width animated rows, and memory-row hover rendering.
Tests requiring installed/authenticated provider CLIs are ignored by default;
run those explicitly when validating live-provider behavior. Signing continuity
and installation while running have separate macOS regression scripts.
Automated tests do not replace visual checks
of hover, dragging, scrolling, and native permission dialogs.

The native PDF/Word integration check can be run on macOS with:

```sh
clang -fobjc-arc -Wno-incompatible-pointer-types scripts/test-document-preview.m \
  crates/app/src/document_preview.m -framework AppKit -framework Quartz \
  -o /tmp/del-document-preview-test
/tmp/del-document-preview-test
```

The native browser check uses a local HTTP fixture, verifies navigation, page
JavaScript, screenshots, isolated profiles, cookie/local-storage persistence
across process restarts, profile deletion, and pointer routing during sidebar resizing:

```sh
python3 scripts/test-browser.py
```

Dictation lifecycle checks do not record audio or prompt for permissions:

```sh
clang -fobjc-arc -mmacosx-version-min=11.0 scripts/test-dictation.m -framework AppKit -framework Speech \
  -framework AVFoundation -o /tmp/del-dictation-test
/tmp/del-dictation-test
```

### Composer picker shortcuts

Use ⌃M to select a model, ⌃⇧M to open effort controls, and ⌃⇧P to select a
provider or account in the active chat pane. These commands also work in Side
chats, including tabs moved between zones. ⌃⇧B opens the starting-branch picker
in a new-session draft. Hover the controls for shortcut badges; Settings →
Keybinds lists all four. Locked controls and non-chat tabs do not open a picker.

Model selection focuses search as soon as its popup opens; type to filter and
use ↑/↓ and Return without clicking the field. Side chat account menus use the
same live account identities as the main chat.
After selecting a model, press Return in the effort popup to confirm and return
to the composer. This does not send the draft message.
