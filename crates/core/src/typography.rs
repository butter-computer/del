//! The shell's type scale, in logical pixels.
//!
//! Every text size in the UI comes from one of these tokens, so there are no
//! one-off sizes. Three steps are enough for the current screens: secondary
//! text, body text, and a single heading size.

/// Secondary text: placeholder-adjacent labels, chip values.
pub const TEXT_SM: f32 = 12.0;
/// Body text: the composer input, chip labels, the top bar title.
pub const TEXT_MD: f32 = 13.0;
/// The one heading size, used for the page prompt.
pub const TEXT_LG: f32 = 16.0;

// Checked at compile time: the scale must stay strictly increasing.
const _: () = assert!(TEXT_SM < TEXT_MD && TEXT_MD < TEXT_LG);
