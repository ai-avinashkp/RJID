//! Workspace/project model, shared across the IDE.

pub mod fs_tree;

pub use fs_tree::{FileNode, read_dir_tree};
