//! Embedded terminal for del: PTY spawn, emulation and a GPUI view.

mod colors;
pub mod keys;
mod local_command;
pub mod mouse;
mod search;
mod session;
mod view;

pub use colors::TerminalTheme;

pub use view::{ChatRequested, TerminalView, init, set_layout_animating};

pub use session::{
    TerminalEvent, TerminalSession, TerminalShutdown, TerminalSize, TerminalSpec, shutdown_all,
};

#[doc(hidden)]
pub use session::TerminalEventProxy;
