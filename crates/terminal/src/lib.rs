//! Embedded terminal for del: PTY spawn, emulation and a GPUI view.

mod colors;
pub mod keys;
pub mod mouse;
mod search;
mod session;
mod view;

pub use view::{TerminalView, init};

pub use session::{TerminalEvent, TerminalSession, TerminalSize, TerminalSpec, shutdown_all};

#[doc(hidden)]
pub use session::TerminalEventProxy;
