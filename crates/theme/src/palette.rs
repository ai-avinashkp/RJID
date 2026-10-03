use gpui::{Rgba, rgb};
use serde::{Deserialize, Serialize};

/// One of the 5 built-in themes. Persisted by name in settings, so the enum
/// variant names must stay stable once shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    CustomMonokai,
    CustomDracula,
    CustomDarkContrast,
    CustomOneDark,
    CustomLight,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 5] = [
        ThemeKind::CustomMonokai,
        ThemeKind::CustomDracula,
        ThemeKind::CustomDarkContrast,
        ThemeKind::CustomOneDark,
        ThemeKind::CustomLight,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ThemeKind::CustomMonokai => "Custom Monokai",
            ThemeKind::CustomDracula => "Custom Dracula",
            ThemeKind::CustomDarkContrast => "Custom Dark Contrast",
            ThemeKind::CustomOneDark => "Custom One Dark",
            ThemeKind::CustomLight => "Custom Light",
        }
    }

    /// Cycles to the next theme, wrapping around — used by the theme picker.
    pub fn next(self) -> ThemeKind {
        let idx = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
}

impl Default for ThemeKind {
    fn default() -> Self {
        ThemeKind::CustomOneDark
    }
}

/// Syntax-highlighting colors for a theme.
#[derive(Debug, Clone, Copy)]
pub struct SyntaxColors {
    pub keyword: Rgba,
    pub string: Rgba,
    pub comment: Rgba,
    pub number: Rgba,
    pub function: Rgba,
    pub type_name: Rgba,
    pub punctuation: Rgba,
}

/// A full UI + syntax color palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub kind: ThemeKind,
    pub background: Rgba,
    pub surface: Rgba,
    pub border: Rgba,
    pub foreground: Rgba,
    pub foreground_muted: Rgba,
    pub accent: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
    pub syntax: SyntaxColors,
}

impl Theme {
    pub fn for_kind(kind: ThemeKind) -> Theme {
        match kind {
            ThemeKind::CustomMonokai => Theme {
                kind,
                background: rgb(0x232420),
                surface: rgb(0x2b2c27),
                border: rgb(0x55564c),
                foreground: rgb(0xf1efe6),
                foreground_muted: rgb(0x9a9b8e),
                accent: rgb(0xffb454),
                error: rgb(0xef5d6f),
                warning: rgb(0xe6b450),
                success: rgb(0x9fd88b),
                syntax: SyntaxColors {
                    keyword: rgb(0xff6fae),
                    string: rgb(0xe1d287),
                    comment: rgb(0x767867),
                    number: rgb(0xb98cf2),
                    function: rgb(0xa6e13d),
                    type_name: rgb(0x66d9e8),
                    punctuation: rgb(0xc9c7b8),
                },
            },
            ThemeKind::CustomDracula => Theme {
                kind,
                background: rgb(0x21222c),
                surface: rgb(0x282a3a),
                border: rgb(0x5a5e7d),
                foreground: rgb(0xf1f2f8),
                foreground_muted: rgb(0x8890b5),
                accent: rgb(0xbe95ff),
                error: rgb(0xff7597),
                warning: rgb(0xf6c177),
                success: rgb(0x8be9a0),
                syntax: SyntaxColors {
                    keyword: rgb(0xff7edb),
                    string: rgb(0xf6f6a1),
                    comment: rgb(0x6c7188),
                    number: rgb(0xbe95ff),
                    function: rgb(0x8be9a0),
                    type_name: rgb(0x8bd8fd),
                    punctuation: rgb(0xd0d2e8),
                },
            },
            ThemeKind::CustomDarkContrast => Theme {
                kind,
                background: rgb(0x000000),
                surface: rgb(0x121212),
                border: rgb(0x6b6b6b),
                foreground: rgb(0xffffff),
                foreground_muted: rgb(0xc9c9c9),
                accent: rgb(0x4dd0ff),
                error: rgb(0xff5c5c),
                warning: rgb(0xffd54d),
                success: rgb(0x66e07a),
                syntax: SyntaxColors {
                    keyword: rgb(0x4dd0ff),
                    string: rgb(0xffd54d),
                    comment: rgb(0x8f8f8f),
                    number: rgb(0xff9e64),
                    function: rgb(0x66e07a),
                    type_name: rgb(0xc27dff),
                    punctuation: rgb(0xffffff),
                },
            },
            ThemeKind::CustomOneDark => Theme {
                kind,
                background: rgb(0x282c34),
                surface: rgb(0x2f333d),
                border: rgb(0x5c6472),
                foreground: rgb(0xdcdfe4),
                foreground_muted: rgb(0x828a9c),
                accent: rgb(0x61aeee),
                error: rgb(0xe06c75),
                warning: rgb(0xe5c07b),
                success: rgb(0x98c379),
                syntax: SyntaxColors {
                    keyword: rgb(0xc57bdb),
                    string: rgb(0x98c379),
                    comment: rgb(0x5c6370),
                    number: rgb(0xd19a66),
                    function: rgb(0x61aeee),
                    type_name: rgb(0xe5c07b),
                    punctuation: rgb(0xacb2be),
                },
            },
            ThemeKind::CustomLight => Theme {
                kind,
                background: rgb(0xfafafa),
                surface: rgb(0xf0f0f0),
                border: rgb(0x9a9a9a),
                foreground: rgb(0x1f2328),
                foreground_muted: rgb(0x63697a),
                accent: rgb(0x2f6feb),
                error: rgb(0xd1242f),
                warning: rgb(0x9a6700),
                success: rgb(0x1a7f37),
                syntax: SyntaxColors {
                    keyword: rgb(0xcf222e),
                    string: rgb(0x0a6e1c),
                    comment: rgb(0x6e7781),
                    number: rgb(0x8250df),
                    function: rgb(0x2f6feb),
                    type_name: rgb(0x953800),
                    punctuation: rgb(0x1f2328),
                },
            },
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::for_kind(ThemeKind::default())
    }
}
