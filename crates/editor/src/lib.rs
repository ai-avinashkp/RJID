//! Line-based Java highlighting (the GPUI editor view itself lives in the
//! `app` crate, alongside the rest of the window's GPUI entities).

mod buffer;
mod java_highlight;
pub mod search;

pub use buffer::{INDENT, TextBuffer};
pub use java_highlight::{
    Language, LineState, Token, TokenKind, highlight_java_line, highlight_line, scan_line_state,
};
