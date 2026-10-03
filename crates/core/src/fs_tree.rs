use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// A single entry in the file tree. Directories are loaded lazily: `children`
/// is `None` until the UI expands the node and calls [`read_dir_tree`] again.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// Reads the immediate children of `dir`, directories first, then files,
/// both alphabetically (case-insensitive). Hidden entries (dotfiles) and
/// common noise directories are skipped so real projects stay usable.
pub fn read_dir_tree(dir: &Path) -> Result<Vec<FileNode>> {
    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?;

    let mut entries = Vec::new();
    // One unreadable entry (permissions, a file deleted mid-listing) is
    // skipped rather than hiding the whole directory.
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();

        if should_skip(&name) {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        entries.push(FileNode {
            path: entry.path(),
            is_dir: file_type.is_dir(),
            name,
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

fn should_skip(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(name, "target" | "node_modules")
}
