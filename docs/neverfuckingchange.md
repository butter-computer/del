# Del: agreed UI and behavior rules

These are the product decisions agreed with the user. Preserve them during refactors,
theme changes, dependency upgrades, and UI work. Change them only when the user
explicitly requests a different behavior. Later explicit requests take precedence.
This records the final decisions, including corrections to earlier experiments.

## Colors and surfaces

- Theme presets retain the calm palette principles: calm and
  low-chrome, surfaces only a few steps apart, few and quiet borders.
- The default Graphite dark palette uses: chrome (title bar, sidebar, gutters, status
  bar) `#1D1E21`, page `#212225`, composer `#2A2B2F`, menus and dialogs `#28292D`.
  Text is off-white `#ECECEA` / `#DCDCD8`, muted `#A3A39F`, faint `#83837F`.
- The default Graphite light palette uses warm off-white: chrome `#F7F6F3`, page `#FBFAF8`,
  composer `#F1EFEB`, menus and dialogs `#F6F5F2`, ink `#2A2926`, muted `#625F59`.
- Appearance has a Theme dropdown with swatches: Graphite, Dune, Grove, Tide,
  Rosewood, Ember, Midnight, Aubergine, Olive, Porcelain, Cocoa, Iris, Onyx,
  Obsidian, Soot, Pine, Oxblood, Eclipse, and Vercel. Vercel uses pitch-black
  chrome and workspace with crisp monochrome controls and no animated backdrop by default. Onyx uses monochrome near-black;
  Obsidian, Soot, Pine, Oxblood, and Eclipse add subtle tints to near-black.
  Each has a light and dark variant; color mode and preset persist independently.
  Selecting either updates the whole app immediately. Existing boolean appearance
  preferences migrate to Graphite; unknown preset names fall back to Graphite.
  Preserve provider hues, semantic status meanings, and terminal ANSI colors.
- Appearance includes an independent Background animation dropdown: Off, Life, Liquid,
  Contours, Drift, Fluted Glass, Curtains, Rain, Clouds, Falling Leaves, Pudding, Window Curtains, Seafloor. Persist the choice across restarts and
  theme changes. With nothing saved it starts Off for Vercel and Life otherwise; users
  can explicitly enable any animation on Vercel. Older files saved as Theme default, and
  unknown animation names, start the same way without losing other preferences. Waves and
  Orbit are removed from the picker; saved values migrate to Liquid and Fluted Glass.
  Liquid uses flowing folds and highlights; Fluted Glass keeps vertical ribs fixed
  while the refracted light field moves behind them.
- Backgrounds use the current theme accent, never a fixed orange hue. The base tint is
  the darker accent at 16% in light mode and 14% in dark; simulated styles may exceed it
  in their brightest areas, capped so chat text stays readable. Monochrome palettes remain neutral.
  Preserve the rounded clipping, downward opacity fade, and composer-region blur.
  Off/Reduce Motion omit the animation surface. Pause all GPU motion while inactive
  or hidden.
- The saved Mouse reactive switch under Background animation starts off, is disabled and
  reads as off while the effective background is Off, keeping the saved choice for later.
  Its on and off states must be clearly different in every theme, including those whose
  primary is white. It applies to every style, but only on
  the new-session page: behind a chat or terminal, or under an open dialog, sheet, or
  prompt, the background ignores the cursor.
  Smooth the cursor response and fade it away on exit, except Window Curtains:
  sample its hover position and force on the current frame without input easing.
  Bound force rather than delaying the pointer. Every style has its own physically
  motivated response, never a shared warp of the picture; see the README table. Life is
  stirred by a moving cursor, with Conway rules untouched everywhere else. Pudding reacts
  only to a press that starts on it: it stretches when pulled and rebounds when released.
  Seafloor makes ripples only on click, never on movement. Cloth response has a hard ceiling:
  however fast the cursor moves it must never whip fabric round itself or clip through it.
  Toggling off stops all new cursor influence immediately; simulated motion settles.
  Inactive/reduced-motion behavior still wins.
- Curtains are real 3D cloth, simulated and meshed at high resolution: pleats form from
  gathered fabric on a pinned rail, with hems and bending stiffness so folds stay round.
  Sunlight comes through a mullioned window behind the fabric: pane edges are hard,
  bend across the folds, and soften only with cloth speed and distance. Fold shadows
  use a real shadow map. Never fake this with drifting bands or displaced textures.
- Window Curtains keep the same cloth in front of a real window: sharp where no fabric
  hangs, refracted and diffused through fabric, with hard-edged sunbeams marched through a
  real volume. Cloth divides the light: beams stay sharp only through real gaps, and fabric
  blocks them, passing on only a soft glow. Passing cursors can never lift a panel. A press
  and hold on a panel drags it: behaviour depends on where it is held (near the rail it slides
  the hooks, low down it swings until pulled tight), each panel has its own hard limits and
  stays where it is left, neighbours are undisturbed, and presses on the input bar don't count.
  The grabbed vertex follows the current pointer exactly after the solver constraints,
  without a grip ramp, target easing, held-panel damping, or rail speed cap. Spatial
  travel limits still apply. Preserve the press location across GPU readback so motion
  during picking is not lost. Letting go settles the fabric gradually; this release
  behavior must not delay a new drag. Hover stays away from a resting cursor after release
  until the cursor moves. Window movement uses the owning window's live WindowServer
  bounds at GPU submission, including when GPUI reuses the painted surface. Apply 25%
  of that displacement immediately to free fabric, tapering to zero at the pinned rail.
  This softens small window movements without input smoothing. Use desktop coordinates,
  never sidebar/composer layout changes. Reset sampling on inactive/resume so moving
  an inactive window does not kick the fabric when reactivated.
- Seafloor caustics come from refracting a mesh of light rays through a wave surface, never
  a scrolling caustic texture. They fill the whole page edge to edge, stay subtle, and have
  no sand or other visible floor. Render Seafloor at 60% of its former opacity,
  including capped highlights, in both themes.
- Clouds are volume rendered with self-shadowing. Pudding is a soft body on the input bar
  and wobbles when nudged or when the bar moves. Falling Leaves use tapered silhouettes,
  veins, flutter and tumbling motion.
- The new-chat page shows the background sharp. After a message is sent it stays behind
  the conversation as a soft blur. Page-level terminals of every provider show the same
  blur through default-background cells and a program's dominant compatible dark fill;
  light/bright fills and other program colours stay opaque to preserve text contrast. Off/Reduce Motion return terminals to fully opaque.
- Behind the bars, the background is only ever blurred and collided with inside each measured
  shape. The empty space between the footer's two groups is plain background, never a band.
- Rain falls at a clear angle. Each drop is one straight line colliding with the measured
  rounded composer/footer outline, including side walls. Landed water beads
  along the edge, merges, runs round the corners and down the sides, then falls. Rain has no
  haptics and uses no GPU readbacks. Layout changes reset impact detection.
- Prefer quality over GPU cost in backgrounds, but they must hold frame rate. Check any
  change with `scripts/render-backgrounds.m`, in dark and light.
- The sidebar uses the same chrome surface as the title bar: no seam between them.
- Keep the light-mode composer darker than the main workspace. Do not brighten
  the composer past the workspace or reverse the surface hierarchy.
- Separators are quiet: white 6% in dark, warm ink 8% in light.
- Neutral colour literals in the UI are mapped onto this palette centrally
  (`appearance::rgb`/`rgba`), so one palette change reaches every pane.
- Preserve readable selections, hover states, separators, and muted text.
- Do not change unrelated colors when asked to adjust one surface. Keep dark-mode
  styling independent of light-mode adjustments.
- Keep surfaces stable across wallpapers; avoid washed-out translucent greys.
- Accents are desaturated and used only as small, purposeful cues. Graphite uses
  softened blue (`#8FB4E6` dark / `#2F5F9E` light); other presets coordinate their
  action and selection accent with the palette. Retain muted green
  for done/success, soft amber for "needs you"/caution, dusty red for failed or
  destructive. Ordinary tabs and text stay neutral.
- Do not restore an opacity slider or load old saved opacity values.

## Sidebar header, footer and keep-awake

- The sidebar header holds the Space switcher on the left and Space actions (+)
  on the right. Search remains available through ⌘K, with no sidebar Search button.
- The footer holds Keep Mac awake, Archive, Memory usage, and Settings, in that
  order. Restore the equal-width rounded button tiles: 9 px corners, 6 px gaps,
  and distinct neutral background, hover, and pressed fills. Keep the same
  appearance in dark and light themes. Footer tooltips open upward.
- Archive toggles the archived view and has a selected background while active.
- Space actions opens Create Space, Create Project, Rename Space and Delete Space.
  New sessions come from project plus buttons, the `+` after the top tabs, and ⌘N.
  The Space switcher menu contains only Space switching and management actions;
  Create Project stays in the + menu and Archive stays in the footer.
- Empty Spaces show a playful orbit illustration, “A little space for your next
  big idea”, and a hint to use + to create a project. An empty archive uses
  “Nothing in this orbit yet” and “Archived sessions will land here.” Only show
  this when there are no visible sessions or projects, never for collapsed groups.
- Keep-awake uses a neutral hover-style background, never a yellow filled background.
- When off, the keep-awake cup uses the same color as the other toolbar icons; never dim it.
- While enabled, use the gold cup with subtle rising steam above it. Respect Reduce Motion.
- Static archive and pin marks use the shared SF Symbols (`archivebox`, `pin`, and
  `pin.fill`), with SVG as a platform fallback. Interactive controls use matching
  native vector silhouettes so individual parts can animate.
- Tooltips must remain visible and clear of the title bar; toolbar hints open below.

## Memory usage

- The visible heading, tooltip, and accessibility label are **Memory usage**.
- Anchor the memory popup above its footer button so it never overlaps the top tabs.
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
  Clear the shared session hover when the pointer moves onto a project heading,
  draft, or gap; only visible session bounds count as part of that hover group.
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

- Opening/closing the left sidebar slides it: contents keep their full width and
  move with the divider, clipped at the edge. Never squeeze or reflow the contents.
- The sidebar footer's buttons share the row height, with equal left and right
  insets; the last button must not touch the divider.

- Keep a visible theme-aware vertical divider between the workspace and each sidebar,
  including the right Files/Terminal panel. Preserve its resize target.

- Pinned sessions (chats and terminals together) keep their pin order, oldest first. A new pin goes after existing
  pins; activity must not reorder them. Unpinning and pinning again appends it.
- Sidebar rows support drag-reordering like top tabs, with live row movement.
  Lock the preview horizontally inside the sidebar; use vertical movement for reordering.
  Hide the source row and its highlight while dragging; show only the floating preview.
  Neighbor rows use the top tabs’ 90 ms easing, with a dead zone to prevent jitter.
- Top-level and inner tab drags use the same native level-change haptic as sidebar session reordering. Tick once when the reorder target changes, while reordering within its row; movement within the same slot stays silent.
- Persist manual order across restarts. Reordering must preserve selection and history.
- Restrict dragging to the same project or the shared pinned section. Chats and
  terminals can reorder together. Cross-project or pinned/unpinned drops must not
  change membership, pin status, history, or the selected session.
- Preserve the visible order of old pins when migrating saved data without order metadata.
- Pinned sidebar rows have no persistent pin glyph or reserved pin space. Keep
  the hover Unpin action and working/unread indicators.

## Unread conversations

- Mark replies completed in chats the user is not viewing as unread. Sidebar
  rows show unread as semibold title text and a filled done check; top tabs keep
  the blue dot.
- Completion in the currently viewed, active chat stays read and clears any stale dot.
- The selected chat must also become unread when it finishes in an inactive Del window.
- Clear the unread marker when the user opens the chat, returns focus to its window,
  or submits another prompt in it. Do not clear unread markers in other chats.
  On hover, a row's time and status glyph give way to its action icons, laid out
  through render state; keep actions at the same right inset regardless of unread
  state. Hovering alone does not mark it read.

## Model and effort controls

- Resolve configured effort before opening the picker. Preserve Side chat sessions,
  transcripts, and catalogs during model/effort/tier-only changes. A loading catalog
  must never be described as a model without effort support. Unknown defaults
  must not be displayed as the lowest advertised effort.
- Temporary Claude settings changes must restart without disk resume and deliver
  the inherited context plus Side chat history on the next prompt. Keep the same
  Del session, subscribers, and peer identity.

- An empty model catalog must show an explicit empty state with Refresh. Keep
  automatic selection distinct from an explicit model while displaying its resolved name.
  Resolve provider settings before using catalog recommendations; the configured default
  can differ. Keep model selection unset when merely displaying the resolved name, including
  in Side chats. If resolution is unavailable, show Automatic rather than guessing.

- Effort slider changes emit one native haptic tick per level transition on supported
  macOS hardware, for dragging, clicking stops, and arrow keys.
- Repeated pointer movement within the same level stays silent. Opening the picker
  or synchronizing saved settings must not trigger feedback.

## Working indicators

- Working replaces unread in one trailing slot; never show a spinner and unread marker
  side by side. Preserve the unread state so the marker returns after work finishes
  unless the reply was actually read. Apply this to sidebar rows, drafts, top tabs,
  and sidebar tool tabs. Sidebar unread ticks and spinners share one centered
  14 px status slot, with its right edge aligned to timestamps on unmarked rows.

- Each sidebar row shows its relative age (now, 4m, 2h, 5d) and one status glyph:
  working spinner, amber waiting ring (needs you), red failed mark, or filled unread
  tick that clears when the chat is read. Hide the relative age while the unread
  tick is visible; restore it once read. Read, inactive rows have no status marker
  or completion tick. Restore provider logos
  on session and draft rows; terminal rows use the terminal glyph with a provider
  badge. Do not show `term` or `chat` text labels.
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
- Top-level session tabs and sidebar tool tabs share 11-point close glyphs
  and 16-point hover buttons, including Browser pages.
- Each Browser tab owns one page, with no nested tab strip. Browser pages use the
  same outer row, ordering, close controls, and reordering as other tools.
  Reordering a page preserves its live WebKit view, profile, and agent tab ID.
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
- Restoring Terminal history into Chat must not expose native command wrappers
  or ANSI control codes as user messages. Use provider metadata to distinguish
  local commands and background notifications from literal user examples; keep
  their useful output in expandable activity. Preserve native transcript files.
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
  Keep markers compact: 10 px spacing, 6 px idle width, and at most 18 px on hover.
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

- Interactive icons animate their parts and vector paths, rather than applying
  generic whole-icon rotation or scaling. Keep hover/press and state morphs short
  and reversible inside stable hit targets. Respect Reduce Motion, disabled
  actions, and cancelled presses. Do not delay an action until its animation
  completes. Main and Side chat share this behavior; preserve provider identities.

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
  Changing between Codex, Claude, and OpenCode continues the same chat with a compact
  switch row and a fresh native session on every switch. Preserve displayed history,
  queue, browser, project, and tab identity. Attach public context separately from
  user input; retain readable saved history across all provider segments.
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
  toggle, and Chat/Terminal controls on one row. Directory, branch and New worktree
  form one container on the left and Chat/Terminal its own container on the
  right, each flush with the composer's edge, with an empty middle and no
  full-width row container. Do not add a second row. The new-tab page has no
  "What are we building?" heading.
  Show the starting branch as a muted grey branch icon followed by its name, without a "Branch:" prefix.
  Hide Git controls for non-repositories; disable them while launch is in progress.
- Create unique owned worktrees and branches asynchronously, retain their directory
  on resume, and reuse the same worktree on a failed-start retry.
- Never discard Git work when a session closes, unloads, archives, or is deleted.
  Explicit unused-worktree cleanup must preserve referenced checkouts, dirty files,
  ignored/untracked files, new commits, and all branches.
- Every new-session action creates an independent draft. A project's plus uses
  that project's directory; the general plus inherits the currently selected
  session or draft directory. Use the remembered directory only without a selected
  tab. The + menu offers Create Project (a directory picker), and
  project row plus buttons appear only on hover.
- Terminal sessions use a terminal glyph with a small provider badge at its lower
  right. Chat sessions retain their provider glyph.
- New worktree is a compact checkbox, not a switch, with 6 px of extra right padding. Keep keyboard activation
  and disabled state, without an extra outer outline.
- Projects follow the sidebar header directly, grouped without a section heading.
  Each project row shows its name without a session count. When sessions wait for
  approval, show an amber "N needs you" badge; it gives way to the three-dot menu and
  plus on hover. Indent session and draft rows 12 px under their project heading. Hide Pinned when empty.
  Show the first five sessions per project by default, plus the selected session if
  it is farther down. Keep drafts visible. Show more reveals all sessions; Show less
  restores the limit. Keyboard navigation follows the same visible rows.
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

- Label the archive view “Archived” and hide its project-row plus buttons. The
  global New Session action remains available and returns to the active view.

- Restoring one session makes its project visible in Projects without restoring
  archived siblings. The project can appear in both views with disjoint sessions;
  restoring the last archived session removes its archive group. A project with
  only archived sessions stays out of Projects unless it has an active draft.
- Deleting a project from Archived deletes only archived histories and preserves
  its active sessions. Deleting from Projects still covers all project histories.
- Project visibility comes only from its sessions (and active drafts), never saved
  project archive/delete flags or empty remembered roots. Changing a draft's
  directory moves it to that directory group and expands the destination without
  restoring archived sibling sessions. The footer Archive button is highlighted while viewing archives.

- Project expand/collapse is instant: no animation, transition, or delay when a
  project group opens or closes. (Section headings keep their reveal.)

- Rename projects inline in their sidebar row, using the same editor styling as
  sessions. Enter saves a nonempty display name; Escape or blur cancels. Clicking
  inside the editor must not collapse the project. Do not open a rename modal.
- Project deletion keeps an opaque theme-aware confirmation dialog with readable
  copy, Cancel, and a red Permanently delete button. Setting Dialog button props
  alone does not create a footer; render the buttons.
- Archiving/deleting the final project leaves an empty usable new-session page.
  Do not create a replacement `/` or home project from an automatic fallback draft.
  Only explicit draft tabs and saved sessions determine project visibility.
  Switching to an empty Space also uses an unlisted composer, skips archived/deleted
  folders when choosing its directory, and ignores stale deleted-project draft tabs
  in saved decks. Only explicitly adding a deleted folder may bring it back.
  Startup also uses an unlisted empty composer. It must support provider/model,
  directory, Chat/Terminal selection, and submission without requiring a plus click.
  Submission creates the real session; an explicit plus creates a draft tab.
- Animated project contents must fill the sidebar width throughout expansion and
  collapse; measuring content must not shrink or clip session titles.

- Clicking the nonempty Pinned heading toggles that section with a reversible
  reveal, persisted independently from the individual projects' expansion states.
  Keep contents full-width and respect Reduce Motion.

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
  refreshes automatically while visible. Show only pending changes whenever staged,
  unstaged, or untracked files exist; otherwise show recent commits on the current
  branch. Include added/removed lines per commit and totals for the displayed
  commits; ignore binary line counts and compare merges against their first parent.
  Switch automatically, with no mode toggle or redundant clean-state message.
  No Refresh, Edit file, or Ask about this
  buttons; no Ask about this buttons in Files or Terminal either.
- The no-Git indicator is informational and must not accept clicks.
- Each click of the right-sidebar plus adds an independent New tab with a
  hover-only close button. Switching tools preserves blank tabs and their address
  drafts; restore explicitly created blanks across restarts. Fresh sidebars
  default to one New tab chooser only, without preopened tools. Closing a blank removes only that
  tab. If the sidebar would become empty,
  show one fresh New tab with its label and hover-close control.
- Mount the right-sidebar tab strip and its plus in the top title bar, aligned
  above the right panel beside the session tabs. Do not repeat the strip inside
  the panel. Keep tool identity, close/reorder behavior, and shortcuts scoped to
  the current session. Reveal the header and panel together; rapid toggles reverse
  smoothly from the current position, and reduced motion skips the transition.
- The right-sidebar plus follows the last tab instead of filling empty space.
  When tabs overflow, keep the plus reachable and gradually fade the clipped
  strip edges; fades must not intercept clicks or scrolling.
- Right-sidebar tabs can be reordered independently across tool kinds. Preserve
  the active instance, drafts, live shells, and each session’s saved order.
- The main area contains only the selected agent conversation or terminal session,
  with no inner tab bar or plus button. Tool tabs live only in the right sidebar.
  No drag target may transfer tools into the main area.
- Migrate saved main-area tool tabs into their owning session's sidebar. Keep
  browser identities/profiles, editor drafts, and side-chat storage identities.
  Import IDs must be stable so retrying migration cannot duplicate tabs.
- Sidebar tool tabs use a compact 112-point width; top-level session tabs are
  144 × 26 points with an 8-point radius. Both share the shell background, 11-point close glyphs, 16-point
  hover buttons, and gradual overflow fades. Keep the plus after the last tab.
- Sidebar dragging stays horizontal with a darker held preview, no magnetic
  resistance, and a haptic tick when its reorder target changes. Leaving the row
  must not remove its tab or create another zone. Preserve selection and live tools.
- The left sidebar has an invisible 16-point resize target centered on the
  six-point page gutter, extending slightly over both neighboring surfaces.
  The full target must show the resize cursor and accept drags.
  Tool icons match the launcher; New tab uses a page icon.
- Each right-sidebar terminal appears directly in the right-sidebar tab row, with
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

- Command-[ and Command-] step back and forward through visited sessions and tabs.
- Command-Left/Right cycle top session tabs. Command-Up/Down cycle visible left-sidebar
  sessions in displayed order (pinned, then project drafts and sessions), skipping collapsed
  groups. These shortcuts work from the composer too; Shift variants retain text selection.
- Command-T and Control-T follow the same active zone as Command-W: create a new
  session from the main chat/session strip, or a new sidebar tab from the right
  sidebar/strip. A hidden sidebar never receives these shortcuts. Command-N always
  creates a session; explicit tool shortcuts and the sidebar plus still open on
  the right. Reveal the active tab immediately, including opens after overflow.
- Command-W follows the active zone: close the top-level session from the main
  chat or its session strip, and the active tool tab from the right sidebar or its
  strip. Keyboard focus back to the main composer also selects the main zone.
  A hidden sidebar never receives Command-W; an empty active sidebar never falls
  through to closing the session. Command-Shift-W always closes only the active
  right-sidebar tab.
- Changes (Control-Shift-G), Terminal (Control-backtick), Files (Command-P), and
  Side chat (Option-Command-S) always open in the right sidebar. Show these shortcuts
  in the new-tab launcher with neutral icons and subtle surfaces. Stack the four
  options vertically in a centered column, capped at 320 points wide, below the
  URL bar; allow scrolling in short panes. Keep cards and the URL
  field visibly lighter than the page background, without a horizontal divider
  below the URL bar.
- Dragging the left sidebar divider 80 logical pixels past its minimum width
  collapses it with a smooth, reversible reveal. Keep the same drag active after
  collapse: dragging back across a reopen threshold 32 points above the close
  threshold reopens it and allows continued resizing without releasing the mouse.
  Preserve the last valid width while closed, avoid flicker near the threshold,
  and respect Reduce Motion.
- Empty left-sidebar space must not move the window. Its divider owns only
  sidebar resizing and stops mouse-down propagation.

- Side chat uses the shared main-chat provider chip and searchable model/effort
  picker, including Fast mode and reset. Keep account names in the provider
  submenu; all changes stay scoped to the side-chat fork.
- Side chat uses the same queued-message rows and actions as main chat: Steer,
  Edit, Delete, drag reordering, attachment previews, and scrolling after four
  rows. Enter queues during a turn; Cmd-Return steers the draft or sends the first
  queued message when empty. Editing preserves the existing draft and attachments
  in the queue. All actions stay scoped to that chat and share delivery/Stop rules.
- Side chat remains available while the main conversation is open in Terminal.
  Opening or refreshing it reads the provider's latest saved context without
  resuming, stopping, or taking ownership of the terminal conversation. Replies
  run in an independent fork for Claude, Codex, and OpenCode.
- `/terminal` and `/chat` are local mode commands for Claude, Codex, and OpenCode.
  Preserve the native conversation and saved account/settings. Wait for the
  terminal to exit before Chat resumes; retain a button to return after terminal
  exit or a failed resume. Mode changes preserve peer mail and tool ownership.
  Main-chat picker shortcuts must not edit a hidden composer in Terminal mode.
- Settings includes a read-only Keybinds category with grouped current shortcuts
  and their scopes. Start directly with the shortcut groups, without explanatory
  intro text. Do not imply remapping is available until it is implemented.

- Keep the left-sidebar edge, the area below the top tab bar, and the right-sidebar
  content gutter free of visible divider lines. In the title bar only, separate
  session tabs from sidebar tools with a thin, vertically centered 20px line.
  Leave breathing room before each tab strip's plus button.
  Preserve the sidebar resize hit targets. Search result rows use
  the same provider and terminal marks as sidebar rows instead of visible
  Chat/Terminal type labels.
- Top-level session tabs, sidebar tool tabs, and their drag previews share
  the same zoom-scaled height. Keep close targets and labels inside that height.

- Side-chat tabs in the right sidebar show a working spinner for their own live turn.
  The parent session's top tab and left-sidebar row aggregate all its side chats
  and the main conversation. Finishing, failing, or closing one side chat must
  not clear another chat's working indicator. Sending from a side chat keeps its
  parent top-level preview tab open, including before a draft becomes a session.
- The untouched Files split gives an empty preview only the bottom fifth of the
  pane. Preserve manually resized heights; clamp the tree in shorter windows.
  Anchor resize drags to the measured tree height at mouse-down.
- Right-sidebar shell terminals follow the selected theme, including light mode,
  and use rounded page corners. Provider-session terminals retain their dark
  default/glass treatment. Preserve explicit terminal colors and inverse-video
  rendering in both surfaces.

- Successful Side chat replies completed out of view mark that fork unread and
  show the blue dot in its tab, its parent's top tab, and the left sidebar. Keep
  unread state across restarts and clear only the side reply actually viewed in
  an active window. Reading the main conversation must not clear hidden forks.
  Cancelled/failed turns and history hydration must not create new unread dots.
- Tab opening uses a short eased expansion/fade (220 ms); closing is faster
  (160 ms). Match top and right-sidebar strips. Release closed tools
  immediately; retain only a noninteractive visual for the exit. Reduced Motion
  settles instantly, as do cross-pane transfers and active drags.

## Composer picker shortcuts

- ⌃M opens model selection, ⌃⇧M opens effort controls, and ⌃⇧P opens
  provider/account selection for the active chat in the last-used zone. Do not change the parent chat when using a side-chat picker.
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

Hide floating browser previews while any Browser pane is visible in the sidebar of their session, including hosted/moved panes and a different selected
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

- Main agents and Side chats share the ordinary peer mailbox in both directions.
  Discovery exposes parent IDs and durable open/closed child history. Preserve
  one entry across native ID changes; reject sends to closed Side chats instead
  of silently queuing work that cannot run. Keep project isolation unchanged.

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


## Spaces

- The Space switcher in the sidebar header shows only the active Space name with
  a chevron, without a Spaces prefix. Its menu lists the Spaces (the active one
  checked) and then offers Create Space and Rename Space with
  icons. A red Delete Space action appears only with multiple Spaces and moves
  projects, pins, and tabs to the first remaining Space without deleting chats or
  files. Existing projects and pins belong to Default; new Spaces start empty.
  The menu stays compact; Space dots above the utility buttons also switch Spaces.
- Create/Rename Space uses a compact dialog with a borderless name input, no blue
  focus outline, and simple Cancel/Create or Save actions. Focus the name on open;
  Enter submits exactly once and Escape dismisses.
- Each Space owns its project collection, pins, and session tab deck. A project
  may be included in more than one Space without copying sessions or changing
  the project boundary for agent communication. Project menus offer Move to Space
  when another Space exists. Move membership, pins, ordering, drafts, and tab references;
  merge an existing destination project without duplicates. Preserve live processes,
  global tabs, and files. A pin in either collection remains pinned in the merge.
- Two-finger horizontal swipes over the left sidebar slide outgoing and incoming
  project/pin lists directly with finger travel. Release past one quarter of the
  width to complete; shorter/cancelled swipes ease back. Keep toolbar and footer fixed,
  avoid switching the active tab deck until the slide finishes, and respect Reduce Motion.
  Schedule settling via next-frame callbacks; never request a rendering-view
  animation frame directly from an input event or next-frame callback.
  Switch one Space per gesture;
  vertical scrolling remains ordinary list scrolling. There is no dot row; the
  header switcher lists every Space.
- Session tabs are space-specific by default. Appearance settings has a Global
  session tabs toggle in a plain row matching Theme, without an extra card background. Returning to scoped tabs restores earlier decks and keeps
  newly opened global tabs in the active Space. Changing the toggle keeps Settings open.
- Switching Spaces must preserve drafts, running agents, terminal processes, side
  chats, and browser ownership. Async startup updates the original Space's tab
  without selecting it in another Space. Persist collections and selections across
  app restarts; never delete or relocate project files when switching or renaming.

### Provider switching with Chat/Terminal

- `/terminal` resumes the active provider segment, preserving its account and
  separately attached context. `/chat` imports new terminal turns into the same
  displayed conversation without replacing earlier provider segments or details.
- Side chats and historical reads include previous segments and new persisted
  terminal turns. Keep exclusive ownership: return to Chat before switching
  provider or account again. Preserve recovery across restarts and failed returns.

## Harp workbench

The Harp redesign wraps Del's page in a calm, VS Code-like workbench with no
activity bar. It supersedes earlier rules where they conflict.

- Title bar (40 px), Linear layout. Above the sidebar, in its width: traffic
  lights, sidebar toggle, then Back and Forward
  right-aligned to the sidebar's padding. Back and Forward step through visited
  sessions and tabs, disable at the ends, and use ⌘[ / ⌘]; a new visit drops the
  forward history; entries that no longer exist are skipped.
- Session tabs start at the chat page's left edge (0 px), are 144 × 26 with an
  8 px radius and active fill, and show the working spinner. The new-tab `+`
  (24 px) sits 8.5 px after the last tab, not at the far edge. With the sidebar
  hidden the tabs follow the navigation buttons. There is no search pill in the
  title bar. At the far right: the right
  sidebar toggle (⌘\\ / ⌘⇧B / ⌥⌘B), with the right sidebar's tool strip. Keep
  the bottom terminal panel available through ⌘J without a title-bar button.
- The chat page sits directly under the title bar with no header row (no session
  name, path or status bar above it). Status shows as the tab spinner and in the
  status bar. The composer radius is 8 px on every page.
- Status bar (22 px, full width, chrome surface). Left: Space · project (Space
  menu), branch with `*` when dirty and `↑N` when ahead (View changes; Choose
  starting branch in drafts), worktree (shows Files), N running and N need you
  (each jumps to the next such session). Right: provider (provider/account
  picker), model · effort (model picker), Chat/Terminal (switches mode like
  `/chat` and `/terminal`), keep awake, queued follow-up count (focuses the
  composer), version (checks for updates). Git state refreshes when the shown
  project changes and after each reply.
- Bottom terminal panel (⌘J): a real shell per project (`zsh — <project>`),
  framed like the page, kept alive while hidden. Resize with its top sash
  (minimum 96 px), double-click the sash to reset to 200 px, maximize and
  restore, close. Its height persists with the sidebar widths.
- The left sidebar defaults to 264 px and resizes between 180 and 320 px.

- The Space switcher arrow morphs down/up with menu open state, including Escape
  and outside-click dismissal. Space menu dividers span the menu width. Restore
  clickable Space dots above the utility footer when more than one Space exists.
- Right-sidebar terminals use the current theme's default foreground, background,
  and cursor with rounded page corners. ANSI colors and program overrides remain
  intact; default color queries report the themed values.

## Workbench status metrics

- The bottom status bar has branch/files/colored line changes, context, allowance, session cost, running commands, and the Chat/Terminal toggle. Keep provider/model in the composer and keep-awake in the sidebar.
- Context means the active conversation carried by the model, measured by its latest usage snapshot, never cumulative lifetime tokens. Show percent used with a reported capacity or an exact known model specification; clear it on provider switch, and show unavailable in Terminal mode.
- Missing allowance or price data is unknown, not zero. Account allowances identify the selected profile by full email (including the default CLI account), include reset times when reported, and refresh no more than once per minute, including failed queries.
- The cost headline sums the main chat and all its side chats, including closed ones; hover/click breaks down that family by provider across every switch. Reconcile each chat independently before summing. Side-chat costs follow their stable storage identity and migrate with draft-to-session/terminal-to-chat ownership; keep them when a side tab closes. Cost updates are cumulative within a stable provider scope and replace earlier values. Persist captured costs across app restarts; don't reconstruct CLI process costs from native history without reliable boundaries. Disclose that captured pricing may exclude older/Terminal activity and may differ from actual billing.
- Git and process inspection run off the UI thread with deadlines. Count renamed files once. Process lists include only Del-owned trees and attached provider roots; hide background provider connections, shells and helpers. Count this session’s commands and offer both This session and All sessions sections with listening ports. Drafts count all sessions. Empty sections say “0 running”. Shared provider commands with unverifiable ownership belong only in All sessions; never collect command arguments.

- Keep allowance account identity/reset information, but omit provider/account actions and all checked/refresh interval rows from status popovers. Composer overflow fades use the theme surface and actual wrapped text geometry without intercepting editing.
- Estimated costs use a dated, source-linked model rate table and stable message/thread scopes. Reconcile reported/estimated coverage within each native session (each step for OpenCode), then sum across visits without adding overlapping totals; unknown models remain unpriced. Estimates use standard rates, may differ after model/tier changes, and are never subscription bills.

- New session right sidebars start closed. Remember each session's last open/closed
  state across tab and Space switches and app restarts, including the Changes pane.
  Explicit tool-opening actions can still reveal the sidebar.
- Omit the redundant “clean” text from the branch status bar when no files changed.


## Session controls follow-up

- Hide relative ages while a session’s working spinner is visible, including
  activity from its Side chats.
- The right sidebar has a 360-logical-pixel minimum. Keep provider and model on
  one row in Side chat composers, including Claude / Opus 5.5 High.
- Steer is explicit user input even between native turns while waiting for peers.
  Deliver that amendment without completing, pausing, or dropping peer obligations
  and without releasing later follow-ups. Failed steering remains reviewable.
- Agent-created chats apply the same built-in provider permissions as ordinary
  Del chats before launch, including parents with legacy permission metadata.
- Each new Side chat action creates a separate fork from the main conversation’s
  current context. Preserve independent settings, draft, queue, history, unread
  state, lifecycle, and persisted identity for all open side chats. Selecting an
  existing tab or opening a peer link reuses that chat rather than creating one.
