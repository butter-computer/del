# Del

Del is a native Rust desktop workspace for coding agents, built with GPUI. It
combines Codex, Claude Code, and installed ACP agents with conversation tabs,
a follow-up queue, a file viewer, integrated terminals, a session browser, and
native Apple voice dictation.

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

Opening a saved chat from the sidebar uses one temporary preview tab. Opening
another replaces that preview in place. Double-click the sidebar session or its
tab, send/queue a prompt, or move the tab to keep it open. Preview titles are
muted and italic. New chats remain open, and replaced previews retain their
conversation history and composer drafts.

**⌘K** opens conversation search. Search message text or titles across saved Codex
and Claude chats, including terminal conversations and archived sessions. Use
**↑/↓** to select a match, **Enter** (or click) to open it, and **Escape** to close.
Add **/chat** or **/terminal** to limit results to that session type; use either
filter by itself to list its sessions. Tab shortcut badges stay hidden while
the palette is open.

Results show the matching passage; opening one keeps its tab, scrolls to the
matched text, and briefly highlights it, including within Markdown replies.

Search reads local provider transcripts on a background worker and caches their
message text in memory, refreshing changed files when the palette opens. It does
not start agents or terminals to index history. Sent messages and streamed
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
  preserved. The tab bar scrolls when tabs overflow and reveals the selected tab.
  Dragged tabs stay inside the tab strip, beyond the left-sidebar toggle.
- The project folder and Chat/Terminal controls sit in a darker footer beneath
  the input, keeping workspace selection distinct from the message area.
- **Chat / Terminal** selects how the first prompt will run. Choosing Terminal
  keeps the initial composer visible; the terminal opens after submission.
- A session's project folder is fixed after creation. Its folder label becomes
  informational rather than a clickable folder picker.
- Chat renders streaming answers, reasoning, tool activity, diffs, usage, and
  approval requests. The message rail appears after six user messages and provides
  jump-to-prompt navigation. Its ticks lengthen only on hover.
- Assistant text uses [FlowToken](https://github.com/Ephibbs/flowtoken)'s default
  streaming fade, tuned to 400 ms with ease-in-out timing. The first live chunk
  also fades in smoothly; saved messages appear immediately when reopened.
  This is implemented with GPUI's native Markdown motion, preserving selection
  and honoring Reduce Motion; the React package is not embedded in Del.
- Opening a saved conversation shows a transcript shimmer while it loads,
  with static placeholders when Reduce Motion is enabled.
- Sessions have tabs and sidebar entries, with rename, pin, unread, archive,
  restore, and permanent-delete actions. Terminal entries share the session
  context menu. Provider-owned titles update from conversation metadata; manual
  renames take precedence.
- A blue dot at the far right marks unread conversations, including background
  replies that finish before being opened. Hover actions make room for the dot.
- Sidebars resize from their panel boundaries. Empty left-sidebar space can drag
  the macOS window. Hover highlights slide between session/menu rows, and the
  keep-awake control animates yellow steam while active.
- Session titles use the available sidebar width, reserving extra action space
  on hover so terminal pin/close buttons do not overlap their titles. New-session,
  new-tab, tab-close, sidebar, archive, and pin icons share the same 15 px size.
  Sidebar rows keep their minimum height and single-line titles in
  short or narrow windows; the list scrolls instead of compressing rows.
- General application, session-start, model-loading, and message-delivery failures
  appear in dismissible toasts. Failed messages remain available to retry or edit.

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
| Claude | CLI initialization metadata; advertised models and effort settings passed through `--effort` |
| ACP | Installed registry agents and their advertised model/config options; `thought_level` is exposed for the agent's default entry when available |

Selecting a model opens its reasoning controls. Slider stops correspond to
advertised effort levels, and the accent follows the selected provider (including
Codex blue and Claude orange). Use **←/→** in the reasoning controls to move one
level at a time, stopping at either endpoint. Fast mode is enabled only when the model
advertises support.
Reset clears explicit effort/fast settings and refreshes the catalog.

Model, effort, and service-tier selections are stored with the conversation.
Existing chat settings apply on subsequent prompts through the backend's
reconfiguration path; they do not rewrite a turn already in flight. Changing a
model clears incompatible effort/tier choices. Provider changes use a new draft.
Catalog discovery submits no prompt, but can start a CLI process; ACP discovery
may create a temporary session.

## Follow-up queue and attachments

While a reply runs, **Enter** queues the current draft. **⌘ + Enter** submits
it as steering input; with an empty composer, it submits the top queued item.
Completed turns advance remaining follow-ups in order. A busy provider leaves
an undelivered message waiting instead of displaying a failed-send error or
incorrectly changing the running status to Done.

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
queue editing, reordering, and retry. Codex receives local image references;
Claude and ACP receive encoded image content. Other files
are supplied as local paths for the harness's tools to read. This does not embed
PDF/spreadsheet contents in the prompt or transcode video; interpretation and
access depend on the harness's tools and permissions. Terminal-mode opening
prompts include paths for all attached files, including images.

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

## Files, terminals, and browser

The right sidebar has **Files**, **Terminal**, and **Browser** panes. The file tree loads
folders on demand, supports optional dotfiles, and excludes generated
`target`/`node_modules` trees. Selection highlights fit the file label. The tree
and preview have a draggable split. The file tree has no opaque edge overlays
over its translucent background.

The viewer is read-only. Text files show line numbers; Markdown supports
**Markdown / Preview** modes. Text previews are capped at 512 KiB and 3,000 lines.
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

Click **TERMINALS** in the left sidebar to collapse or expand the list, with a
short transition and no disclosure-arrow icon. Row highlights fade and clip with
the list. Reduce Motion makes the transition instant.

Terminal content fills its pane without a title/status/Copy output header.

The workspace terminal starts the shell selected by `$SHELL` as a login shell,
falling back to `/bin/zsh`. Its terminal engine is `alacritty_terminal`, rendered
inside Del with GPUI. The cursor is hidden when the terminal or app loses focus.

Terminals use a real PTY, support shell and agent sessions, and retain colored
terminal cells while letting the normal chat surface show through their default
background. Saved Codex/Claude terminal sessions reopen the exact provider
conversation ID without replaying the initial prompt. Legacy Codex identity
recovery proceeds only when one saved conversation matches its launch time and
workspace. Unresolved identity is reported rather than choosing the latest chat.

Built-in Codex and Claude terminal launches automatically trust the workspace
opened in Del, including resumed sessions. Codex receives a launch-scoped
[`projects.<path>.trust_level`](https://developers.openai.com/codex/config-reference/)
override and an explicit `--cd` so login shell startup cannot change its workspace.
Both providers' recognized startup trust dialogs are confirmed as soon as they
appear (100 ms polling), with short retries only while the exact workspace prompt
remains visible. Confirmation is limited to startup (two minutes) and stops once
the prompt disappears. Unrecognized dialogs stay interactive.
Shell/custom commands and arbitrary ACP agents keep their own trust setup.

Del starts and reopens Codex and Claude sessions with full access by default.
Codex chat uses `approvalPolicy: never` and `sandbox: danger-full-access`;
Codex terminals use `--dangerously-bypass-approvals-and-sandbox` (YOLO mode).
Claude chat uses `bypassPermissions` with `--dangerously-skip-permissions`,
and Claude terminals use the same bypass flag. These modes allow file changes
and command execution without tool-approval prompts. Existing saved chats adopt
the setting when reopened; there is currently no in-chat permission toggle.
ACP sessions and custom shell commands keep their existing permission behavior.

Del's MCP bridge provides scoped session coordination, status, notifications,
and user questions for supported terminal agents. Browser tools are also available
to Codex and Claude chat sessions. Launch-specific credentials
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

Codex and Claude agents can use Del's authenticated `browser` MCP tool to discover,
create, select, close, and navigate tabs, inspect page text/elements, click, fill,
send keys, evaluate page JavaScript, and take screenshots. These actions target the
same visible browser and saved profiles. Del is the default browser for agent
requests; external browsers are used only when explicitly requested by the user.
Agents can choose any saved session's browser with its session/tab ID; chat
credentials expose only browser operations.
Page content is treated as untrusted data. ACP browser-tool injection is not yet
implemented.

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

Update failures are logged without blocking chat. Recognized successful CLI
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

Settings has **Preferences** (persisted Light/Dark appearance) and **Providers**
(sign-in status, sign-in, and a refresh icon to re-check status). Light mode uses
more opaque surfaces; dark mode keeps its translucent background. The sidebar
entry includes a gear icon.
The right sidebar is hidden in Settings; **Escape** closes Settings and returns
to the selected conversation, terminal, or draft.
Hold Command to see numeric hints on the top tabs. There are no numeric sidebar
session shortcuts.

| Shortcut | Action |
| --- | --- |
| Command-1–9 | Select a top tab by position |
| Command-N / Command-T | New session |
| Command-W | Close the active tab |
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
| Command-J / Control-Backtick | Open the workspace terminal pane |
| Command-Comma | Settings |
| Escape in Settings | Return to the previous workspace view |
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
UI regression tests also cover preview tabs, unload/resume, hover clipping and
collapse fading, first-chunk fading, reasoning-step boundaries, and zoom shortcuts
with composer or terminal focus. Permission tests cover new and resumed sessions,
terminal bypass flags, and Codex permission changes on subsequent turns.
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
