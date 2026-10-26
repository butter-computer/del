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
- Shell opacity is fixed at 85%. Do not restore the opacity slider or load old
  saved opacity values. Light surfaces are opaque to preserve the reference colors.

## Sidebar toolbar and keep-awake

- Four equally sized buttons, in order: Search, New session, Keep Mac awake,
  Memory usage. Search must not expand wider than the others.
- Cap control growth; empty space is fine. Do not stretch buttons to fill a wide sidebar.
- Keep-awake uses a neutral hover-style background, never a yellow filled background.
- While enabled, use the gold cup with subtle rising steam above it. Respect Reduce Motion.
- Archive and pin controls use the shared SF Symbols (`archivebox`, `pin`, and
  `pin.fill`) like the search and activity icons, with SVG only as a platform fallback.
- Tooltips must remain visible and clear of the title bar; toolbar hints open below.

## Memory usage

- The visible heading, tooltip, and accessibility label are **Memory usage**.
- Open the popup to the right of the toolbar icon with a pointer on its left.
- Keep it compact: about 240 logical pixels wide, at most four visible rows.
- Sort loaded chat and terminal sessions by highest process RAM usage first.
- Each single-line row contains the correct provider logo, a short fading title,
  and RAM on the right. Terminal providers must resolve correctly, including Codex.
- Clicking the title opens that session.
- Show delete and unload icons on hover, with consistent padding and usable targets.
- Use one row hover background, not a second nested highlight on the title.
- Fade long titles like the sidebar; do not allot most of a wide popup to the title.
- Allow scrolling, hide the scrollbar, and fade content at the lower edge.
- Do not show “Process RAM · highest first,” “shared” suffixes, or shared-RAM
  explanatory text in the popup. Keep measurement caveats in documentation.
- Unload preserves history and releases the live session; delete follows the
  existing permanent-delete behavior. Unavailable measurements are a dash, not zero.

## Sidebar ordering

- Pinned chats keep their pin order, oldest first. A new pin goes after existing
  pins; activity must not reorder them. Unpinning and pinning again appends it.
- Sidebar rows support drag-reordering like top tabs, with live row movement.
  Hide the source row and its highlight while dragging; show only the floating preview.
  Neighbor rows use the top tabs’ 90 ms easing, with a dead zone to prevent jitter.
- Top-tab and sidebar chat/terminal drags emit one native haptic tick when the
  destination position changes. Do not tick on every pointer event, on invalid
  cross-section drops, or for non-drag reorder operations.
- Persist manual order across restarts. Reordering must preserve selection and history.
- Restrict dragging to the same section and pin group: pinned chats, regular
  chats, pinned terminals, and regular terminals remain separate. Dropping across
  those boundaries must never pin, unpin, replace, or move a session into another group.
- Preserve the visible order of old pins when migrating saved data without order metadata.

## Unread conversations

- Show a blue dot for replies completed in chats the user is not viewing.
- The selected chat must also become unread when it finishes in an inactive Del window.
- Clear the blue dot when the user opens the chat. Leave room for it beside hover actions.

## Model and effort controls

- Effort slider changes emit one native haptic tick per level transition on supported
  macOS hardware, for dragging, clicking stops, and arrow keys.
- Repeated pointer movement within the same level stays silent. Opening the picker
  or synchronizing saved settings must not trigger feedback.

## Tabs and session previews

- Opening an existing chat or terminal without interacting creates a temporary,
  muted italic preview tab. Opening another preview replaces that slot.
- Double-clicking either a sidebar entry or its top tab makes it permanent.
  This must work for terminals exactly as it does for chats.
- Sending/queueing a chat prompt, typing/pasting into a terminal, or moving the
  tab keeps it open. Merely receiving terminal output must not keep a preview.
- Explicitly created sessions remain open. Replacing a preview preserves history
  and drafts; it is not deletion.
- Close glyphs in top tabs and browser tabs are slightly smaller: 14 logical
  pixels instead of 16. Keep the surrounding click targets unchanged.

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

- Follow the supplied Codex desktop reference with left-aligned user message cards with their original background, unboxed assistant prose, and compact muted tool rows with semantic SF icons.
- Tool rows use muted text and inline disclosure arrows; omit shell launch wrappers from summaries.
  Merge matching successful edit/file events into one row, keep output/diffs in disclosure, and omit zero-count diff badges.
- Keep tool input/output available through disclosure. Group consecutive tools immediately, including while running.
  Show the latest running action and failure/decline counts in the header; keep each tool’s
  output expandable. Messages and approval requests remain separate boundaries.
- Tool group contents have a small indent and a reversible animated expand/collapse.
  Respect Reduce Motion and keep virtual-list heights synchronized throughout the reveal.
- Keep search highlights, message navigation, and immediate streaming intact.
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
- Bound event draining so a busy stream cannot monopolize the UI update loop.
- Preserve selection, Markdown rendering, reasoning/tool events, and saved transcript loading.

## Settings

- Settings is a modal over the existing workspace, not a replacement page.
- Left navigation has icons before labels, consistently aligned. Use Appearance and Accounts.
- The main heading is the selected category, not “Settings” followed by a duplicate category heading.
- Use an opaque modal surface so the underlying composer cannot bleed through.
- Use the same fade-and-slide entrance as Cmd-K, with a heavily blurred, dim backdrop.
- Done, Escape, and clicking outside close Settings and return to the workspace.
- No opacity controls; the value is fixed at 85%.

## Titles

- Keep automatic conversation-title generation enabled.
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
