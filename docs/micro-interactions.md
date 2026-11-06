# Animated control icons

Del adapts the compact, action-specific interactions demonstrated by
[Amicro](https://amicro.dev/buttons) using original native GPUI vector paths.
The icons animate individual parts or interpolate between shapes. There is no
JavaScript runtime, imported web animation code, or whole-icon rotation/scale
in this control system.

| Controls | Animation |
| --- | --- |
| Sidebar Search | A highlight draws inside the lens and the handle extends |
| New session, project new session, main/sidebar new-tab plus, Attach files | The two strokes extend and retract independently on hover/press |
| Settings | The gear stays anchored while its center aperture opens |
| Model refresh/reset | The curved arrow retraces its stroke |
| Session archive and archived-view toggle | The tray lid lifts and the inset slot moves |
| Pin/unpin | The pin head seats along its stem |
| Restore archived session | The return arrow retraces |
| Session delete, memory delete, queued-message delete, clear goal | The lid lifts independently of the bin; inner strokes move |
| Main/sidebar tab close, draft close, attachment removal, memory unload, file-preview close | The cross strokes retract on press |
| Left/right sidebar toggles | The divider moves inside the fixed frame |
| Files launcher | The front opens while the folder back remains anchored |
| Terminal launcher | The cursor stroke extends |
| Changes launcher | The document's detail strokes extend |
| Side chat launcher | The plus strokes expand inside the bubble |
| Microphone | The pickup arc expands around the fixed capsule |
| Queued-message edit | The pencil moves along a newly drawn underline |
| Queued-message Steer | The arrowhead opens along the fixed turn path |
| Reply Copy | The front copy outline morphs into a check; the back outline fades |
| Composer Send/Stop | The arrow polygon morphs into the stop square |
| Goal Pause/Resume | The play triangle divides into pause bars and rejoins on resume |
| Goal details | The chevron's points interpolate between collapsed and expanded shapes |
| Memory popup | The waveform changes amplitude when the popup opens/closes |

`crates/app/src/animated_icons.rs` defines the vector parts and morph geometry.
`crates/app/src/micro_motion.rs` uses GPUI's target-value transitions: hover takes
160 ms, press 90 ms, and state morphs 180 ms. Rapid reversals start from the current
presentation. Initial mounting starts at the correct final state. Frames are
requested only while transitioning, with no idle animation timer.

Hand-built controls can ease their background fill with the same hover and press
values through `micro_motion::control`, so the fill and glyph arrive together. The
shared row highlight fades in over 100 ms and out over 120 ms around its 90 ms slide.

Control bounds, labels, tooltips, focus behavior, and click handlers retain their
layout and ownership. A separate transparent hover surface avoids replacing the
native tooltip listener. Releasing outside cancels pressed feedback. Disabled
controls do not animate on pointer input. Reduce Motion keeps icons stationary
and applies state morphs immediately. Actions take effect on click, without waiting
for animation. Copy retains its existing 1.5-second confirmation and full-reply copy.

Main chat, drafts, and Side chat use the same composer/model controls for Claude,
Codex, and OpenCode. Provider identities, streaming text, native terminal output,
and informational file/status icons keep their existing behavior. Existing menu,
tab, sidebar, tool-disclosure, keep-awake steam, and Space transitions remain.

Regression checks cover animated geometry staying within fixed icon bounds,
independent moving parts, intermediate morph paths, hover reversal, tooltip
coexistence, normal/cancelled clicks, disabled controls, and Reduce Motion. Composer
checks click Send and Stop on the first transition frame for all three providers.

Validated on 2026-11-02: the locked workspace suite passed 603 tests (45 ignored),
and workspace/all-target Clippy passed with warnings denied. The signed macOS
debug bundle was rebuilt and launched. Native UI checks covered light/dark icon
contrast, launcher silhouettes, Copy confirmation, Settings, and the Memory popup.
Provider coverage here is the shared composer's automated interaction test;
this visual change did not require new live provider inference.

After integration with main's 0.1.24 provider-switching, shared Side chat queue,
and Harp workbench changes, the locked workspace suite passed 748 tests (46 ignored), and
workspace/all-target Clippy passed again with warnings denied. The queue controls
retain the shared edit callback, accessibility labels, and regression selectors.

The main tab strip uses the current intrinsic width of its animated slots, so
the new-tab button follows a closing tab on every frame. Closing ghosts no longer
reserve their full width until cleanup. A GPUI regression checks intermediate
closing widths and verifies that overflow keeps the plus button reachable.
The rebuilt macOS app was also checked by opening and closing a temporary tab
on 2026-11-03.
