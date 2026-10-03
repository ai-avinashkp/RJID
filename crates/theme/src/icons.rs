use gpui::{Rgba, rgb};

/// A minimal, self-drawn file-type "icon": a short monogram and a color,
/// rendered by the UI as a small colored badge. Deliberately not an imported
/// icon font/pack, to keep the project free of third-party icon-asset
/// licensing questions.
#[derive(Debug, Clone, Copy)]
pub struct FileIcon {
    pub glyph: &'static str,
    pub color: Rgba,
}

fn folder_icon() -> FileIcon {
    FileIcon {
        glyph: ">",
        color: rgb(0xe8b04b),
    }
}

fn generic_file_icon() -> FileIcon {
    FileIcon {
        glyph: "•",
        color: rgb(0x9a9a9a),
    }
}

/// Picks an icon for a directory entry by name/extension. `is_dir` takes
/// priority; for files, a handful of well-known extensions and filenames get
/// a distinct monogram/color, everything else falls back to a generic dot.
pub fn icon_for(name: &str, is_dir: bool) -> FileIcon {
    if is_dir {
        return folder_icon();
    }

    let lower = name.to_lowercase();

    if lower == "pom.xml" {
        return FileIcon {
            glyph: "M",
            color: rgb(0xc71a36),
        };
    }
    if lower == "build.gradle" || lower == "build.gradle.kts" {
        return FileIcon {
            glyph: "G",
            // Bright teal: readable on both the dark and light themes.
            color: rgb(0x3fb6c9),
        };
    }

    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "java" => FileIcon {
            glyph: "J",
            color: rgb(0xe8813b),
        },
        "kt" | "kts" => FileIcon {
            glyph: "K",
            color: rgb(0xa26bf2),
        },
        "xml" => FileIcon {
            glyph: "X",
            color: rgb(0xe89b3b),
        },
        "json" => FileIcon {
            glyph: "{}",
            color: rgb(0xe0c341),
        },
        "yml" | "yaml" => FileIcon {
            glyph: "Y",
            color: rgb(0xcb4b4b),
        },
        "properties" => FileIcon {
            glyph: "#",
            color: rgb(0x7fae4c),
        },
        "toml" => FileIcon {
            glyph: "T",
            color: rgb(0x9c6bd1),
        },
        "md" => FileIcon {
            glyph: "M↓",
            color: rgb(0x4f9de0),
        },
        "js" | "ts" => FileIcon {
            glyph: "JS",
            color: rgb(0xe0c341),
        },
        "html" | "htm" => FileIcon {
            glyph: "<>",
            color: rgb(0xe0673b),
        },
        "css" => FileIcon {
            glyph: "#",
            color: rgb(0x4f9de0),
        },
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" => FileIcon {
            glyph: "img",
            color: rgb(0x5cb87a),
        },
        "jar" => FileIcon {
            glyph: "jar",
            color: rgb(0xe8813b),
        },
        "class" => FileIcon {
            glyph: "cl",
            color: rgb(0x8a8a8a),
        },
        "sh" | "bat" | "ps1" | "cmd" => FileIcon {
            glyph: "$",
            color: rgb(0x6ac47e),
        },
        "gitignore" | "gitattributes" => FileIcon {
            glyph: "git",
            color: rgb(0xe0673b),
        },
        _ => generic_file_icon(),
    }
}
