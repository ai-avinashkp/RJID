//! Theming: color palettes and file-type icon glyphs, shared by every UI crate.
//!
//! All colors here are original values authored for this project (inspired by,
//! not copied from, other editors' theme files — see THIRD_PARTY_LICENSES.md).

mod icons;
mod palette;

pub use gpui::Rgba;
pub use icons::{FileIcon, icon_for};
pub use palette::{SyntaxColors, Theme, ThemeKind};
