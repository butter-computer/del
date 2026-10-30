# Del: agreed UI and behavior rules

These are the product decisions agreed with the user. Preserve them during refactors,
theme changes, dependency upgrades, and UI work. Change them only when the user
explicitly requests a different behavior. Later explicit requests take precedence.
This records the final decisions, including corrections to earlier experiments.

## Colors and surfaces

- Light mode follows the supplied reference: white workspace (`#FFFFFF`),
  off-white chrome (`#FCFCFC`), and slightly darker input cards (`#F9F9F9`).
- Keep the light-mode composer darker than the main workspace. Do not brighten
  the composer past the workspace or reverse the surface hierarchy.
- The folder/mode footer matches the composer surface.
- All light-mode menus and modals match the input surface: model list, effort
  picker, slash-command suggestions, context menus, memory popup, search, and Settings.
- Preserve readable selections, hover states, separators, and muted text.
- Do not change unrelated colors when asked to adjust one surface. Keep dark-mode
  styling independent of light-mode adjustments.
- Dark mode follows the supplied ChatGPT desktop reference: opaque near-black
  chrome, black workspaces, a slightly darker left sidebar, and charcoal inputs.
  Keep surfaces stable across wallpapers; avoid washed-out translucent greys.
- Use blue for actions and selections, green for save/success, amber for revert or
  caution, and red for destructive/error states. Ordinary tabs and text stay neutral.
- Keep the existing light-mode palette. Do not restore an opacity slider or load
  old saved opacity values.

## Sidebar toolbar and keep-awake

- Four equally sized buttons, in order: Search, New session, Keep Mac awake,
  Memory usage. Search must not expand wider than the others.
  Leave a small gap between the toolbar’s final button and the sidebar divider.
- Cap control growth; empty space is fine. Do not stretch buttons to fill a wide sidebar.
- Keep-awake uses a neutral hover-style background, never a yellow filled background.
- While enabled, use the gold cup with subtle rising steam above it. Respect Reduce Motion.
- Archive and pin controls use the shared SF Symbols (`archivebox`, `pin`, and
  `pin.fill`) like the search and activity icons, with SVG only as a platform fallback.
- Tooltips must remain visible and clear of the title bar; toolbar hints open below.

## Memory usage

- The visible heading, tooltip, and accessibility label are **Memory usage**.
- Anchor the memory popup below its toolbar button so it never overlaps the top tabs.
- Keep it compact: about 240 logical pixels wide, at most four visible rows.
- Sort loaded chat and terminal sessions by highest process RAM usage first.
- Each single-line row contains the correct provider logo, a short fading title,
  and RAM on the right. Terminal providers must resolve correctly, including Codex.
- Clicking the title opens that session.
- At rest, RAM is at the far right; delete and unload reserve no empty space.
  On row hover, reveal those controls to its right with usable padded targets.
  Change their layout through render state, never paint-time group-hover display
  changes: GPUI must lay out/prepaint the buttons before painting them.
- Use one row hover background, not a second nested highlight on the title.
- Fade long titles like the sidebar; do not allot most of a wide popup to the title.
- Allow scrolling, hide the scrollbar, and fade content at the lower edge.
- Do not show “Process RAM · highest first,” “shared” suffixes, or shared-RAM
  explanatory text in the popup. Keep measurement caveats in documentation.
- Unload preserves history and releases the live session; delete follows the
  existing permanent-delete behavior. Unavailable measurements are a dash, not zero.

## Sidebar ordering

- Top-tab titles fade at their trailing edge instead of using ellipses. Use all
  space up to the action slot; an absent unread dot must not reserve a gap.
  Reclaim the close-button space when idle and not hovered. Preserve descenders
  with enough line height, and use a smooth trailing fade.

- Keep a visible theme-aware vertical divider between the workspace and each sidebar,
  including the right Files/Terminal panel. Preserve its resize target.

- Pinned sessions (chats and terminals together) keep their pin order, oldest first. A new pin goes after existing
  pins; activity must not reorder them. Unpinning and pinning again appends it.
- Sidebar rows support drag-reordering like top tabs, with live row movement.
  Lock the preview horizontally inside the sidebar; use vertical movement for reordering.
  Hide the source row and its highlight while dragging; show only the floating preview.
  Neighbor rows use the top tabs’ 90 ms easing, with a dead zone to prevent jitter.
- Top-level and inner tab drags use the same native level-change haptic as sidebar session reordering. Tick once when the reorder target changes, including entering a destination zone or changing its insertion slot; movement within the same slot stays silent.
- Persist manual order across restarts. Reordering must preserve selection and history.
- Restrict dragging to the same project or the shared pinned section. Chats and
  terminals can reorder together. Cross-project or pinned/unpinned drops must not
  change membership, pin status, history, or the selected session.
- Preserve the visible order of old pins when migrating saved data without order metadata.
- Pinned sidebar rows have no persistent pin glyph or reserved pin space. Keep
  the hover Unpin action and working/unread indicators.

## Unread conversations

- Show a blue dot for replies completed in chats the user is not viewing.
- Completion in the currently viewed, active chat stays read and clears any stale dot.
- The selected chat must also become unread when it finishes in an inactive Del window.
- Clear the blue dot when the user opens the chat, returns focus to its window,
  or submits another prompt in it. Do not clear unread markers in other chats.
  Hide the sidebar dot on hover so the action icons occupy its space; keep actions
  at the same right inset regardless of unread state. Hovering alone does not mark it read.

## Model and effort controls

- Effort slider changes emit one native haptic tick per level transition on supported
  macOS hardware, for dragging, clicking stops, and arrow keys.
- Repeated pointer movement within the same level stays silent. Opening the picker
  or synchronizing saved settings must not trigger feedback.

## Working indicators

- Working replaces unread in one trailing slot; never show a spinner and blue dot
  side by side. Preserve the unread state so the dot returns after work finishes
  unless the reply was actually read. Apply this to sidebar rows, drafts, top tabs,
  and both inner tab zones. Sidebar dots and spinners share one centered status
  container aligned with the rightmost hover action, so their centers never shift.

- Show a thin rotating ring with a fading tail in sidebar rows and top tabs while a chat is running,
  matching the supplied ChatGPT reference. Use smooth constant rotation, not a segmented spoke spinner.
  Terminal sessions use their reported Working status. Hide the spinner on hover
  to reveal the existing close, pin, or archive actions; keep the trailing slot stable.

## Tabs and session previews

- Opening an existing chat or terminal without interacting creates a temporary,
  muted italic preview tab. Opening another preview replaces that slot.
- Double-clicking either a sidebar entry or its top tab makes it permanent.
  This must work for terminals exactly as it does for chats.
- Sending/queueing a chat prompt, typing/pasting into a terminal, or moving the
  tab keeps it open. Merely receiving terminal output must not keep a preview.
- Explicitly created sessions remain open. Replacing a preview preserves history
  and drafts; it is not deletion.
- Top-level session tabs and both tool-tab zones share 11-point close glyphs
  and 16-point hover buttons, including Browser pages.
- Each Browser tab owns one page, with no nested tab strip. Browser pages use the
  same outer row, ordering, close controls, and cross-zone drag as other tools.
  Moving a page preserves its live WebKit view, profile, and agent tab ID.
  Restore legacy nested pages as separate outer tabs; never drop blank pages
  that were explicitly opened. Agent new-tab requests create outer sidebar tabs.

## Search and command suggestions

- Cmd-K toggles search. Escape and clicking outside dismiss it through the same
  close path, including a smooth disappearance animation.
- Use a short fade-and-slide entrance and a 140 ms fade-out. Respect Reduce Motion.
- Strongly blur and dim the background; keep the dialog dark in dark mode.
- Do not show a match-count line such as “0 matches.”
- Put **/c /t Filter** on the right of the navigation-hints footer. Navigation,
  Enter/Select, and Escape/Close hints remain on the left.
- `/c` filters chats and `/t` filters terminals. Preserve filtering and keyboard navigation.
- Search matches must be visibly highlighted, including in Markdown and after opening a result.
- Result types say **Chat** or **Terminal**, without “You” or “Assistant.”
  Vertically center the type label with the Enter icon.
- Keep rounded clipping correct, including the footer in light mode.
- Inserted skill tokens retain their original blue (`#579FFF`) text and icon.
- Remove argument-placeholder hints such as `AREA=<value>` from all command
  suggestion footers. Preserve command descriptions and navigation hints.

## Transcript layout

- Follow the supplied Codex desktop reference with left-aligned user message cards with their original background, unboxed assistant prose, and compact muted tool rows without leading icons.
- Assistant replies show only an overlapping-rounded-squares Copy icon and
  local reply time in 24-hour **HH:MM** format beside it in a hover footer, with a 160 ms eased fade in and out.
  Do not add audio, feedback, or overflow icons to this footer.
  Keep space reserved so hovering does not shift the transcript. Never show response duration here or invent timestamps
  for historical messages without a recorded reply time; hide the time when unavailable.
  Save new reply timestamps across reopening and recover old Codex/Claude timestamps
  from their transcripts. Keep the copy glyph compact (13 px) and time text 11 px.
  Use two equal rounded squares, preserve the icon’s aspect ratio, and align its
  visible left edge with the reply text while retaining the 24 px click target.
  Copy the full reply and replace the glyph with a same-sized checkmark for
  1.5 seconds. Repeated clicks restart the confirmation for that reply only.
- Tool rows use muted text and inline disclosure arrows; omit shell launch wrappers from summaries.
- Tool summaries, including live shimmering group headers, show only the first
  input line and truncate to available width. Never wrap or overlap neighboring
  transcript rows. Keep full multiline input/output available when expanded.
  Merge matching successful edit/file events into one row, keep output/diffs in disclosure, and omit zero-count diff badges.
- Keep tool input/output available through disclosure. Group consecutive tools immediately, including while running.
  Show the latest running action and failure/decline counts in the header; keep each tool’s
  output expandable. Messages and approval requests remain separate boundaries.
- Tool group contents align with the chat text without a nested left indent,
  and retain a reversible animated expand/collapse.
  Respect Reduce Motion and keep virtual-list heights synchronized throughout the reveal.
- Local file links in chat open in Del’s file viewer, resolved against the conversation
  folder. Do not send bare paths to the operating system as application URLs.
- Render Codex file-citation directives as local filename links in assistant replies,
  including streamed, restored, and Side chat messages. Preserve raw saved text,
  literal code examples, and malformed directives; remap search-highlight offsets.
- Keep search highlights, message navigation, and immediate streaming intact.
- Keep intermediate responses visible while work is active, waiting, paused, or
  still streaming. Only after a finished final answer may earlier activity collapse
  under an expandable Worked row. Keep the final answer visible with the only Copy
  footer; intermediate provider-turn completions are not final task answers.
  Search/navigation must expand a hidden target. Never discard collapsed events.
  Use a reversible reveal with reduced-motion support. Save elapsed time at reply
  completion, including peer waits; restore it without counting time spent away.
  Keep tool-summary disclosure arrows directly after their text, not at the far edge.
- Message navigation highlights exactly one tick: the hovered tick while pointing,
  otherwise the current message. Nearby ticks may lengthen but stay gray. Emit a
  native haptic tick on entering a different marker, not on every pointer move.
- Show the live elapsed-work label directly beneath the latest user message,
  scrolling with that message; never fix it at the top of the transcript. Do not invent historical timing.
- Shimmer the inline “Working for…” text (static with Reduce Motion). Do not show
  a duplicate Working spinner, queue/steer hint, or Done label above the composer.
- See [codex-chat-reference.md](codex-chat-reference.md) for the public-source
  reference and the distinction between the open terminal UI and the desktop screenshot.

## Loading and streaming

- Show conversation-loading shimmer only; no visible “Loading conversation…” line.
  Keep an accessible status label and static placeholders for Reduce Motion.
- Display assistant chunks as they arrive. No FlowTokens, word-reveal animation,
  first-chunk fade, easing, or artificial streaming delay.
- Update the Markdown view on incoming chunks and notify/remeasure the list as
  parsing completes. Do not wait for turn completion or a later unrelated repaint.
- Append incoming deltas directly to the existing Markdown view. Coalesce only
  already-received adjacent chunks from the same turn, preserving tool/status boundaries.
  Replies containing file citations refresh their display text as chunks complete directives.
- Bound event draining so a busy stream cannot monopolize the UI update loop.
- Preserve selection, Markdown rendering, reasoning/tool events, and saved transcript loading.

## Settings

- Settings is a modal over the existing workspace, not a replacement page.
- Left navigation has icons before labels, consistently aligned. Use Appearance and Accounts.
- The main heading is the selected category, not “Settings” followed by a duplicate category heading.
- Use an opaque modal surface so the underlying composer cannot bleed through.
- Use the same fade-and-slide entrance as Cmd-K, with a heavily blurred, dim backdrop.
- Done, Escape, and clicking outside close Settings and return to the workspace.
- No opacity controls; use the shared appearance surfaces.

## Titles

- Keep automatic conversation-title generation enabled.
- Sidebar titles visibly shimmer during generation, including the selected row:
  use a muted base and contrasting text-width sweep, static with Reduce Motion.
- Use the checked-in OpenCode title prompt in `crates/agent/src/title_prompt.txt`,
  with its source attribution and MIT license. Do not silently substitute a new prompt.
- Generate concise, natural, specific titles in the background; retain existing
  refresh timing, persistence, fallback, and manual-rename precedence.
- Terminal titles continue to follow provider metadata.

## Delivery and regression checks

- Keep the README accurate when changing these behaviors.
- Check the affected light and dark surfaces; avoid broad palette rewrites for a local tweak.
- Verify streaming before completion and terminal/chat preview parity when touching those paths.
- Build the native macOS app and relaunch when delivering UI changes.
- When asked to commit and push, include the requested work and documentation,
  verify the push succeeded, and report the resulting commit and working-tree status.

## Provider portability

- Provider detection, model loading, chat, and history subprocesses must share
  runtime PATH preparation. Finder's minimal PATH must not break an installed
  CLI whose runtime is in its installation directory or a supported common location.
- Preserve user authentication and explicit launch settings without inheriting
  parent coding-agent session markers. Never embed developer-specific home paths,
  credentials, or account-specific model assumptions as setup requirements.
- A fresh Mac requires its own installed and authenticated provider. Document
  this setup in README; pulling source does not transfer working provider access.

## Multiple provider accounts

- Settings → Providers has an Add account (+) action for Claude, Codex, and OpenCode.
- Saved account rows show a red trash icon (with Delete account tooltip and
  accessibility label) and Make default action, without “Saved account”
  or “Connected” text. Show Sign in only when authentication needs attention.
  Persist the chosen default for new chats; keep existing chat selections intact.
  Prevent deleting accounts still used by chats; move removed profiles to local
  recovery storage instead of destroying credentials.
- Do not require a user-created account label. Use the sign-in identity where
  available and automatic Account N names otherwise.
- Provider rows in the composer picker have arrows that open account submenus
  on hover. Include the default account and each saved profile, with one selection.
- Keep model catalogs, slash commands, title generation, history, and chat
  execution scoped to the selected profile. Never fall back silently to system
  credentials when a profile is missing.
- Persist the account on each chat. Switching accounts within the same provider
  keeps the same chat, native history, and workspace, including across restarts.
  Block switching during running turns and restore the original account on failure.
  Changing providers still creates a new draft.
- Deduplicate full account identities while showing only the username. Close only
  the successful login attempt's own idle Terminal window. Show per-account Codex and Claude
  remaining usage as a slim bar and percentage only: no window labels, reset
  details, or unavailable text. Use the most constrained reported allowance.
  Query Claude through its native get_usage control with skip_behaviors; never
  send an inference prompt or read OAuth credentials to populate the meter.
  Keep usage caches keyed by both provider and account, including system accounts.
  In the composer account picker, only the current chat’s selected account name is green;
  account checkmarks are hidden. Keep usage rings unchanged. Show each account’s
  remaining usage as a compact circular ring beside its name when available,
  without a number inside. Keep the percentage available to accessibility.
- Use separate provider storage and child-process environments; never mutate
  the app-wide environment or overwrite the system CLI's saved login.
- Account sign-in uses the provider's own flow. Do not report success merely
  because a credentials-list command exited successfully.
- Saved accounts apply to Chat and Terminal modes. Persist the terminal account
  and isolate its CLI environment on launch and reopen. Never fall back to the
  system account when a saved profile is unavailable.
- Clicking a provider selects its configured default account; hovering the arrow
  opens per-chat account choices. Only the selected account name is green, with no picker tick.
  Label OpenCode’s built-in account System.

- Account sign-in must validate CLI executables, skipping broken launchers just
  like chat startup. Prepare the child runtime PATH consistently and report
  a failed or cancelled login promptly instead of waiting for a success timeout.

- Show the reply copy/time footer only on the final assistant message of a
  successfully completed turn. Intermediate updates and streaming messages have
  no copy footer or reserved footer space; earlier finished replies keep theirs.

- Account 1 is the existing default provider login; extra profiles start at
  Account 2. Prefer the signed-in email in Settings and the account submenu.
- Disable Add account while that provider has a login in progress, including
  duplicate actions. Re-enable after success, failure, cancellation, or timeout.

- Display account email usernames without the domain. Replace the default-account
  text badge with green account-name text: the default in Settings and the selected
  account in the picker. Do not show account checkmarks.
  Preserve the full identity internally.

- Launch provider sign-in through a short, quoted script path in Terminal. Keep
  long environments inside a private temporary script to avoid input truncation;
  clean it up and report success, failure, or Ctrl+C cancellation.

- Show each provider account once in Settings and the account picker. Compare
  full email identities, not the shortened visible username; the default row
  takes precedence. Preserve stored profiles used by existing chats.

- Offer only Codex, Claude, and OpenCode in the provider picker and Settings.
  Keep legacy ACP records readable, but do not offer ACP for new selections.

## Projects and worktrees

- The starting-branch picker opens below its button with a focused, icon-free search
  field and case-insensitive filtering,
  a viewport-aware maximum of eight visible rows, wheel/trackpad scrolling without
  a visible scrollbar, and a bottom fade only while more results remain. Keep long
  names faded at the trailing edge using the shared tab-title treatment, without
  ellipses, and the keyboard highlight in view. Support ↑/↓, Page Up/Down,
  Return to choose, and Escape to dismiss. Do not add a hint/footer row.

- The left sidebar groups chats and terminals together as sessions under **Pinned**
  and **Projects**. Do not restore a separate Terminals section.
- Automatically identify projects from directories; Git worktrees and subfolders
  belong to their parent repository. Preserve existing histories and pins.
- Keep project expansion state and identities across restarts. Each project offers
  a new-session action, with Chat/Terminal chosen in the composer.
- Below the new-session composer, keep the directory, starting branch, New worktree
  toggle, and Chat/Terminal controls together on one row. Do not add a second row.
  Hide Git controls for non-repositories; disable them while launch is in progress.
- Create unique owned worktrees and branches asynchronously, retain their directory
  on resume, and reuse the same worktree on a failed-start retry.
- Never discard Git work when a session closes, unloads, archives, or is deleted.
  Explicit unused-worktree cleanup must preserve referenced checkouts, dirty files,
  ignored/untracked files, new commits, and all branches.
- Every new-session action creates an independent draft. A project's plus uses
  that project's directory; the general plus inherits the currently selected
  session or draft directory. Use the remembered directory only without a selected
  tab. The Projects heading offers a directory picker, and project
  row plus buttons appear only on hover.
- Terminal sessions use a terminal glyph with a small provider badge at its lower
  right. Chat sessions retain their provider glyph.
- New worktree is a compact checkbox, not a switch. Keep keyboard activation
  and disabled state, without an extra outer outline.
- Keep Projects close to the toolbar. Hide Pinned when empty. Project rows reveal a three-dot menu and plus on hover.
  Project rename changes its display name only. Archive is reversible through the
  archived list. Permanent deletion confirms removal of the project and all its
  session histories; directory contents and Git worktrees remain untouched.
- For directories without Git, show a gray Git icon with a red cross on the right of the
  composer footer, with “No Git repository” in its tooltip; hide the branch picker and New worktree checkbox.
- Dropdown menus ease in over 180ms and fade out over 120ms, respecting reduced
  motion. Dismissed menu items must not activate twice during the exit.

- Project actions use the same icons and moving hover highlight as session context
  menus. Permanently delete uses red text, a red trash icon, and a red hover tint.
- Show the selected model's effective effort on its pill immediately, using its
  catalog default until the user explicitly changes it. Keep the panel and pill
  labels consistent, including provider-supplied effort names.

- Archive view shows only archived projects and sessions, never active sessions or
  drafts. Keep the archive toggle available when this view becomes empty.

- Label the archive sidebar “Archived” and hide its heading and project-row plus
  buttons. The global New Session action remains available and returns to the
  active Projects view.

- Restoring one session makes its project visible in Projects without restoring
  archived siblings. The project can appear in both views with disjoint sessions;
  restoring the last archived session removes its archive group. A project with
  only archived sessions stays out of Projects unless it has an active draft.
- Deleting a project from Archived deletes only archived histories and preserves
  its active sessions. Deleting from Projects still covers all project histories.
- Project visibility comes only from its sessions (and active drafts), never saved
  project archive/delete flags or empty remembered roots. Changing a draft's
  directory moves it to that directory group and expands the destination without
  restoring archived sibling sessions. Archive toggle has a selected background.

- Project expand/collapse uses a reversible measured reveal with reduced-motion
  support; repeated clicks reverse smoothly without snapping or losing rows.

- Rename projects inline in their sidebar row, using the same editor styling as
  sessions. Enter saves a nonempty display name; Escape or blur cancels. Clicking
  inside the editor must not collapse the project. Do not open a rename modal.
- Project deletion keeps an opaque theme-aware confirmation dialog with readable
  copy, Cancel, and a red Permanently delete button. Setting Dialog button props
  alone does not create a footer; render the buttons.
- Archiving/deleting the final project leaves an empty usable new-session page.
  Do not create a replacement `/` or home project from an automatic fallback draft.
  Only explicit draft tabs and saved sessions determine project visibility.
  Startup also uses an unlisted empty composer. It must support provider/model,
  directory, Chat/Terminal selection, and submission without requiring a plus click.
  Submission creates the real session; an explicit plus creates a draft tab.
- Animated project contents must fill the sidebar width throughout expansion and
  collapse; measuring content must not shrink or clip session titles.

- Clicking Projects, Archived, or the nonempty Pinned heading toggles that entire
  section with a reversible reveal. Persist each section independently from the
  individual projects' expansion states. Keep contents full-width, respect Reduce
  Motion, and keep the Projects plus action independent of the heading toggle.

- Reasoning headers use shimmering Thinking while active and static Thought when
  finished. Click the header to expand/collapse; no spinner or show/hide text.
  Respect Reduce Motion.
- Do not show the token-usage line above the composer. Existing session folder
  labels show the project name only, with the branch in the hover tooltip.

- Stop must survive provider resume/send preparation: retain the request and
  interrupt a subsequently starting turn. Pause automatic queued follow-ups after
  Stop; explicit prompt delivery resumes them. Show Stopping… until the provider
  finishes instead of silently accepting a no-op interrupt. Keep Stop accessible.

- A follow-up rejected as Busy after completion is still undelivered: clear its
  optimistic running state and retry after yielding. Only provider TurnStarted
  events establish an active turn; do not wait forever for a second completion.
  Automatic retries must respect Stop, interrupted/failed turns, queue order,
  and the single in-flight delivery guard. Steering must retain later follow-ups.

- Claude steering is not necessarily one input per result. Enable native input
  replay acknowledgements and retire every input consumed by a completed
  response. Keep unconsumed inputs pending; never fabricate a running turn for
  steering already absorbed into the finished response. Cover both native queued
  steering and merged steering, plus a later Del follow-up, in regression tests.

- Codex Stop is idempotent: its explicit “no active turn to interrupt” response
  means the turn already ended, not a user-facing failure. Preserve all other
  interrupt errors. Live provider queue checks cover Claude, Codex, and OpenCode;
  verify ordered delivery, steering followed by queued work, and Stop/resume.

- Sidebar file edits remain drafts until Save/Cmd-S. Preserve drafts across file
  switches and restarts, detect external changes before saving, and preserve
  symlinks and file permissions. Changes is a read-only Git review pane that
  refreshes automatically while visible. No Refresh, Edit file, or Ask about this
  buttons; no Ask about this buttons in Files or Terminal either.
- The no-Git indicator is informational and must not accept clicks.
- Each click of the right-sidebar plus adds an independent New tab with a
  hover-only close button. Switching tools preserves blank tabs and their address
  drafts; restore explicitly created blanks across restarts. Fresh sidebars still
  default to Files, Changes, and Terminal only. Closing a blank removes only that
  tab. If the sidebar would become empty,
  show one fresh New tab with its label and hover-close control.
- The right-sidebar plus follows the last tab instead of filling empty space.
  When tabs overflow, keep the plus reachable and gradually fade the clipped
  strip edges; fades must not intercept clicks or scrolling.
- Right-sidebar tabs can be reordered independently across tool kinds. Preserve
  the active instance, drafts, live shells, and each session’s saved order.
- Move tool tabs between the session row and right sidebar by dragging. Keep the
  original conversation fixed. Transfer live entities rather than reopening tools:
  shells keep their PID, editors keep drafts and undo history, and side chats keep
  their conversation, model/account selection, and storage identity.
- Main and right-sidebar tool tabs share one compact 128-point width so transfers do not resize them. Top-level session tabs are 144 points wide. Overflow fades occupy only 16 points at each edge; selecting an overflowed tab reveals its full slot, including reordered and transferred tabs.
- The left sidebar resize target fills the six-point page gutter, outside the sidebar rows and chat tabs/content.
- Only transfer on a valid drop inside the same session. Entering the other row
  immediately opens an insertion gap; neighboring tabs shift as the pointer moves
  between slots. Do not replace this behavior with a blue zone highlight.
- Keep tool-tab dragging simple: horizontal pointer tracking locked to the source
  row outside a destination and aligned with the destination row while inside it,
  with no vertical pointer following, magnetic resistance,
  or spring effects. Use discrete native haptic ticks when the insertion target changes. Use a darker opaque preview of the normal tab
  width. Collapse the source layout slot when it leaves its row; the plus button
  must immediately follow the remaining tabs. Restore that slot if it returns.
- Insertion gaps participate in layout; do not translate neighboring tabs beyond
  their measured widths or underneath the plus button. Keep the dragged preview
  attached to the pointer at its original horizontal grab offset in both zones;
  entering a destination or changing insertion slots must not snap it into place.
  Continue reordering under the held pointer and commit the transfer only on release.
- Both inner tab rows use the same shell background. All three tab levels use
  the sidebar's 11-point close glyph and 16-point hover button geometry.
- Apply gradual, non-interactive overflow fades to both tool-tab rows.
- Restore moved tools in their destination row after restart. Preserve the existing
  side-chat manifest when migrating to full session-tool persistence. Restored
  terminals start fresh shells and never replay previous commands.
- Use matching tool icons in the launcher and both tab rows. New tab uses a page
  icon; reserve the plus icon for adding tabs.
- Each terminal appears directly in its session or right-sidebar tab row, with
  no nested terminal subtab strip. All are labeled Terminal, without numbers.
  The plus icon stays neutral. Choosing Terminal
  from a new tab always creates a fresh shell, even when other terminals exist.
- Workspace terminal tabs own independent PTYs. Switching tabs keeps shells alive;
  closing one tab must not terminate another. Restore tab layout with fresh shells,
  never by replaying old commands.

- Public releases follow Alpha's Apple Silicon DMG plus SHA-256 format, with an
  Applications shortcut and explicit signing/notarization status. Public bundles
  use computer.butter.del; never replace the local development signing identity
  or include user data, credentials, or provider account profiles in a release.

- Command-T and Control-T create an inner tab in the last-used zone. Reveal the
  active tab immediately, including repeated opens after horizontal overflow.
- Command-W closes only the active inner tab in that zone; it must never
  fall through to closing the top-level session. Command-Shift-W closes the top-level
  session even while an inner tool is active.
- Changes (Control-Shift-G), Terminal (Control-backtick), Files (Command-P), and
  Side chat (Option-Command-S) open in the same active zone. Show these shortcuts
  in the new-tab launcher with neutral icons and subtle surfaces. Stack the four
  options vertically in a centered column, capped at 320 points wide, below the
  URL bar in both zones; allow scrolling in short panes. Keep cards and the URL
  field visibly lighter than the page background, without a horizontal divider
  below the URL bar in either zone.
- Empty left-sidebar space must not move the window. Its divider owns only
  sidebar resizing and stops mouse-down propagation.

- Side chat uses the shared main-chat provider chip and searchable model/effort
  picker, including Fast mode and reset. Keep account names in the provider
  submenu; all changes stay scoped to the side-chat fork.
- Settings includes a read-only Keybinds category with grouped current shortcuts
  and their scopes. Start directly with the shortcut groups, without explanatory
  intro text. Do not imply remapping is available until it is implemented.

- Keep the left-sidebar edge border invisible. Use matching faint 0.5-point
  hairlines below the top bar and beside the right sidebar's tab row. Inset the
  short vertical segment in the middle of the gutter, 6 points from each end
  of the tab row so it floats;
  do not draw a divider alongside the content. Keep line thickness independent
  of app zoom; preserve the sidebar resize hit targets. Search result rows use
  the same provider and terminal marks as sidebar rows instead of visible
  Chat/Terminal type labels.
- Top-level session tabs, inner tabs in both zones, and their drag previews share
  the same zoom-scaled height. Keep close targets and labels inside that height.

- Side-chat tabs in either zone show a working spinner for their own live turn.
  The parent session's top tab and left-sidebar row aggregate all its side chats
  and the main conversation. Finishing, failing, or closing one side chat must
  not clear another chat's working indicator. Keep the original inner chat tab's
  indicator scoped to the main conversation. Sending from a side chat keeps its
  parent top-level preview tab open, including before a draft becomes a session.
- The untouched Files split gives an empty preview only the bottom fifth of the
  pane. Preserve manually resized heights; clamp the tree in shorter windows.
  Anchor resize drags to the measured tree height at mouse-down.
- Terminals always paint their default black background, including in light
  appearance. Preserve explicit terminal colors and inverse-video rendering.

- Successful Side chat replies completed out of view mark that fork unread and
  show the blue dot in its tab, its parent's top tab, and the left sidebar. Keep
  unread state across restarts and clear only the side reply actually viewed in
  an active window. Reading the main conversation must not clear hidden forks.
  Cancelled/failed turns and history hydration must not create new unread dots.
- Tab opening uses a short eased expansion/fade (220 ms); closing is faster
  (160 ms). Match top, main-inner, and right-sidebar strips. Release closed tools
  immediately; retain only a noninteractive visual for the exit. Reduced Motion
  settles instantly, as do cross-pane transfers and active drags.

## Composer picker shortcuts

- ⌃M opens model selection, ⌃⇧M opens effort controls, and ⌃⇧P opens
  provider/account selection for the active chat in the last-used zone. Follow
  Side chat ownership after tab transfers; do not change the parent chat.
- ⌃⇧B opens starting-branch selection only in a new-session draft.
- Show shortcut badges in control tooltips and list the bindings in Settings.
  Respect locked controls and ignore these actions on non-chat tabs.
- Model popovers own search focus when opening; do not defer it to a later
  frame. Preserve typing, arrow navigation, Return, and Escape across reopening
  and switches between model and effort controls. Model rows retain horizontal
  padding inside their hover/selection background.
- Side chat provider menus read live account identities and profiles from the
  owning app state, rather than fixing labels at fork creation.
- Return in the effort popup confirms the current settings and closes the
  popup, returning focus to the composer without submitting its draft.

## Scroll edges and draft rows

- File-tree rows share the session-menu moving hover highlight, clipped to the
  scroll viewport and disabled as motion when reduced motion is enabled.
  Closed folders use neutral grey icons; expanded folders use white. Root folder
  icons align with the Files tab icon, while children retain their indentation.
- Top-level session tabs use the same gradual overflow-edge fades as inner tabs.
  The add-tab button remains outside the fade. Fades disappear at the scroll limits.
- The left sidebar session list fades at its scroll edges above the fixed Settings
  footer and below the toolbar, only while more content remains offscreen.
- New-session draft rows expose the standard small close button on hover. Closing
  a row uses the same draft cleanup and next-tab selection as closing its top tab.

## Agent browser preview

- Agent browser actions preserve the user's current session and tool pane. Hidden
  browser activity shows one small floating thumbnail on the right, not an
  automatically opened sidebar. Tab-list and tab-close actions do not summon it.
- Thumbnail snapshots come from the existing WebKit tab at most once per second;
  they do not resize, reload, or interact with the page. Pause snapshots while
  the browser is visible, the preview is dismissed, or a modal obscures it.
- Drag the card freely inside the window; keep it reachable after resizing or
  zooming. A drag must not also open the browser.
- Clicking reveals that exact browser tab in the right sidebar, reusing its
  page state and moving it back from the inner workspace if necessary. Closing
  the preview does not close the browser or cancel the agent. Repeated actions
  on the same dismissed tab do not reopen the preview.

### Session-scoped browser preview

The floating browser preview belongs to its owning session. Switching sessions
hides it without dismissing it; returning restores that session’s preview and
position. Background browser activity cannot replace the current session’s
preview. Dismissal survives session switches, and closed sessions release their
preview state. The page image itself clips to rounded corners. There is no
URL/header banner: only a small translucent close control with a locally blurred
page backdrop overlays its top-right corner. Clip the backdrop and tint to the
button’s circle, without blurring the rest of the app.

The floating preview frame follows the page snapshot’s aspect ratio within its
size limit. Do not add an outer border or black letterbox padding around it.

Hide floating browser previews while any Browser pane is visible in either
zone of their session, including hosted/moved panes and a different selected
browser tab. Native WebKit surfaces paint above GPUI; leaving the thumbnail
visible there clips it. This visibility suppression is not dismissal: hiding
the Browser pane restores the preview.

Hidden previews must allow frame-driven page rendering to progress. During
sampling, render the existing WebKit view transparently instead of setting
`NSView.hidden`, which pauses animation frames and can strand YouTube on its
loading skeleton. Keep native hit testing, accessibility, and page dialogs
disabled until the user opens the Browser pane. Stop this rendering lease after
sampling stops; a stale lease must never hide a reopened pane. Do not reload,
resize, or inject replacement page scheduling functions to refresh a thumbnail.
Hide native sidebar content at the start of its closing animation, preserving
the browser's last viewport instead of sampling a nearly zero-width page.

## Project peer communication

- Chat agents can create actual chat peers through `spawn_session`, inheriting the
  caller’s directory, project, permissions, and same-provider account/model. The
  first task is a durable peer request with a reply route, not an untracked user
  prompt. Unopened worker chats are driven by the same background peer scheduler.

- Peers perform delegated tasks within existing permissions and standing user
  constraints, including new requests after an earlier assignment completes.
  Continue only unfinished original work. Do not reject a request solely because
  it came from another agent or differs from a completed task.
- Sessions in the same Del project are equal peers across providers. No dedicated
  orchestrator is required. Sibling worktrees use the configured project boundary;
  unrelated projects are excluded.
- Agents can read recorded public context and exchange requests for information
  or changes using authenticated tools. Preserve the recipient’s account, model,
  permissions, and original task. Peer messages are attributed agent input.
- Deliver peer messages ahead of queued user messages at a safe input boundary.
  Never interrupt an active tool or equate a turn ending or a reply with completion
  of the original task. Release queued user messages only when that work completes.
- Persist receipts, work state, and Side chat routing across restarts. Ambiguous
  deliveries need user review before retrying; do not silently replay mutations.
- Show only a centered `Current agent ⇄ Other agent` exchange row, with each
  provider’s logo and clickable conversation name. Use stacked opposing arrows;
  no message body, status label, error copy, or card background. Keep recovery
  actions in the context menu for failed/uncertain deliveries.
- Peer work participates in navigation ticks, working indicators, unread dots,
  and keeping an interacted-with session tab open. Context reads alone must not
  mark another conversation read or change the selected chat.
- Terminal agents use a cooperative inbox; do not claim control over their CLI’s
  native user-input queue. See [peer-sessions.md](peer-sessions.md).
