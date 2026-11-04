//! The shell's type scale, in logical pixels.
//!
//! Keep dense controls distinct from text that people read for long periods.

/// Secondary text: placeholder-adjacent labels, chip values.
pub const TEXT_SM: f32 = 12.0;
/// Compact controls, chip labels, and tab titles.
pub const TEXT_MD: f32 = 13.0;
/// Conversation prose and the composer input.
pub const TEXT_BODY: f32 = 14.0;
/// The one heading size, used for the page prompt.
pub const TEXT_LG: f32 = 16.0;

// Checked at compile time: the scale must stay strictly increasing.
const _: () = assert!(TEXT_SM < TEXT_MD && TEXT_MD < TEXT_BODY && TEXT_BODY < TEXT_LG);
