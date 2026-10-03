//! Embedded terminal: a real PTY-backed shell (`pty_session`) driving a
//! real VT emulator (`emulator`, built on `alacritty_terminal`).

mod emulator;
mod pty_session;

pub use emulator::{CellStyle, Emulator, Run, SCROLLBACK, Screen, SelectKind, TermColor, key_bytes, xterm_256};
pub use pty_session::{PtySession, default_shell};
