//! The shell's type scale, in logical pixels.
//!
//! Every text size in the UI comes from one of these tokens, so there are no
//! one-off sizes: secondary text, chrome text, reading text, and two headings.

/// Fine print: times, sources, keyboard hints.
pub const TEXT_XS: f32 = 11.0;
/// Secondary text: placeholder-adjacent labels, chip values.
pub const TEXT_SM: f32 = 12.0;
/// Chrome text: rows, tabs, menus, chip labels.
pub const TEXT_MD: f32 = 13.0;
/// Reading text: messages and the composer input people write them in.
pub const TEXT_PROSE: f32 = 14.0;
/// Section headings inside messages and dialogs.
pub const TEXT_LG: f32 = 16.0;
/// Page and modal titles.
pub const TEXT_XL: f32 = 18.0;
/// Reading line height for `TEXT_PROSE`, as a multiple of the size.
pub const PROSE_LEADING: f32 = 1.55;
/// Code, diffs and paths.
pub const MONO_FONT: &str = "Menlo";

// Checked at compile time: the scale must stay strictly increasing.
const _: () = assert!(
    TEXT_XS < TEXT_SM
        && TEXT_SM < TEXT_MD
        && TEXT_MD < TEXT_PROSE
        && TEXT_PROSE < TEXT_LG
        && TEXT_LG < TEXT_XL
);
