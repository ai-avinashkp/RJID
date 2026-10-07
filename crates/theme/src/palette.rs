use gpui::{Rgba, rgb};
use serde::{Deserialize, Serialize};

/// A built-in theme. Persisted by name in settings, so the enum variant
/// names must stay stable once shipped (new ones are only ever appended).
/// Palettes are original values authored for RJID, named after the
/// well-known styles they're inspired by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeKind {
    CustomMonokai,
    CustomDracula,
    CustomDarkContrast,
    CustomOneDark,
    CustomLight,
    CustomTokyoNight,
    CustomNord,
    CustomGruvboxDark,
    CustomCatppuccinMocha,
    CustomSolarizedDark,
    CustomGitHubDark,
    CustomSolarizedLight,
    CustomAyuLight,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 13] = [
        ThemeKind::CustomOneDark,
        ThemeKind::CustomMonokai,
        ThemeKind::CustomDracula,
        ThemeKind::CustomTokyoNight,
        ThemeKind::CustomNord,
        ThemeKind::CustomGruvboxDark,
        ThemeKind::CustomCatppuccinMocha,
        ThemeKind::CustomSolarizedDark,
        ThemeKind::CustomGitHubDark,
        ThemeKind::CustomDarkContrast,
        ThemeKind::CustomLight,
        ThemeKind::CustomSolarizedLight,
        ThemeKind::CustomAyuLight,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ThemeKind::CustomMonokai => "Monokai",
            ThemeKind::CustomDracula => "Dracula",
            ThemeKind::CustomDarkContrast => "High Contrast Dark",
            ThemeKind::CustomOneDark => "One Dark",
            ThemeKind::CustomLight => "Light",
            ThemeKind::CustomTokyoNight => "Tokyo Night",
            ThemeKind::CustomNord => "Nord",
            ThemeKind::CustomGruvboxDark => "Gruvbox Dark",
            ThemeKind::CustomCatppuccinMocha => "Catppuccin Mocha",
            ThemeKind::CustomSolarizedDark => "Solarized Dark",
            ThemeKind::CustomGitHubDark => "GitHub Dark",
            ThemeKind::CustomSolarizedLight => "Solarized Light",
            ThemeKind::CustomAyuLight => "Ayu Light",
        }
    }

    pub fn is_light(self) -> bool {
        matches!(self, ThemeKind::CustomLight | ThemeKind::CustomSolarizedLight | ThemeKind::CustomAyuLight)
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
            ThemeKind::CustomTokyoNight => palette(
                kind,
                [0x1b1d2b, 0x222436, 0x4a4f6e, 0xc8d0f0, 0x7e86ad, 0x7aa2f7, 0xf7768e, 0xe0af68, 0x9ece6a],
                [0xbb9af7, 0x9ece6a, 0x5f678f, 0xff9e64, 0x7aa2f7, 0x2ac3de, 0xa9b1d6],
            ),
            ThemeKind::CustomNord => palette(
                kind,
                [0x2e3440, 0x353b48, 0x5a647a, 0xe5e9f0, 0x8f9bb3, 0x88c0d0, 0xbf616a, 0xebcb8b, 0xa3be8c],
                [0x81a1c1, 0xa3be8c, 0x6d7a91, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0xd8dee9],
            ),
            ThemeKind::CustomGruvboxDark => palette(
                kind,
                [0x282828, 0x32302f, 0x665c54, 0xebdbb2, 0xa89984, 0xfe8019, 0xfb4934, 0xfabd2f, 0xb8bb26],
                [0xfb4934, 0xb8bb26, 0x928374, 0xd3869b, 0xfabd2f, 0x8ec07c, 0xd5c4a1],
            ),
            ThemeKind::CustomCatppuccinMocha => palette(
                kind,
                [0x1e1e2e, 0x262637, 0x585b70, 0xcdd6f4, 0x9399b2, 0xcba6f7, 0xf38ba8, 0xf9e2af, 0xa6e3a1],
                [0xcba6f7, 0xa6e3a1, 0x6c7086, 0xfab387, 0x89b4fa, 0xf9e2af, 0xbac2de],
            ),
            ThemeKind::CustomSolarizedDark => palette(
                kind,
                [0x002b36, 0x073642, 0x2f5866, 0xd4dcd6, 0x839496, 0x2aa1b3, 0xe0605b, 0xc49a10, 0x9aaa22],
                [0x9aaa22, 0x2aa198, 0x5e7a84, 0xd33682, 0x268bd2, 0xc49a10, 0xa3b1b3],
            ),
            ThemeKind::CustomGitHubDark => palette(
                kind,
                [0x0d1117, 0x161b22, 0x3d444d, 0xe6edf3, 0x8d96a0, 0x58a6ff, 0xff7b72, 0xd29922, 0x3fb950],
                [0xff7b72, 0xa5d6ff, 0x7d8590, 0x79c0ff, 0xd2a8ff, 0xffa657, 0xc9d1d9],
            ),
            ThemeKind::CustomSolarizedLight => palette(
                kind,
                [0xfdf6e3, 0xf3ecd8, 0xa8a291, 0x3d4f56, 0x6f7f83, 0x1f7fbf, 0xc4302b, 0x936f00, 0x6a7a00],
                [0x6a7a00, 0x1d8c84, 0x8a9a9c, 0xb52e72, 0x1f7fbf, 0x936f00, 0x3d4f56],
            ),
            ThemeKind::CustomAyuLight => palette(
                kind,
                [0xfcfcfc, 0xf0f1f2, 0xa9aeb5, 0x353b44, 0x6b7380, 0xd27100, 0xd9313b, 0x9a6a00, 0x5a8a00],
                [0xd25a00, 0x5a8a00, 0x8a929c, 0x9c56c9, 0xb07a00, 0x2a7fb8, 0x4b5360],
            ),
        }
    }
}

/// Builds a theme from `[background, surface, border, foreground,
/// foreground_muted, accent, error, warning, success]` and syntax colors
/// `[keyword, string, comment, number, function, type_name, punctuation]`.
fn palette(kind: ThemeKind, ui: [u32; 9], syntax: [u32; 7]) -> Theme {
    Theme {
        kind,
        background: rgb(ui[0]),
        surface: rgb(ui[1]),
        border: rgb(ui[2]),
        foreground: rgb(ui[3]),
        foreground_muted: rgb(ui[4]),
        accent: rgb(ui[5]),
        error: rgb(ui[6]),
        warning: rgb(ui[7]),
        success: rgb(ui[8]),
        syntax: SyntaxColors {
            keyword: rgb(syntax[0]),
            string: rgb(syntax[1]),
            comment: rgb(syntax[2]),
            number: rgb(syntax[3]),
            function: rgb(syntax[4]),
            type_name: rgb(syntax[5]),
            punctuation: rgb(syntax[6]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two colors.
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let lum = |c: Rgba| {
            let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        };
        let (la, lb) = (lum(a), lum(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn every_theme_is_readable_and_classified() {
        for kind in ThemeKind::ALL {
            let t = Theme::for_kind(kind);
            // Body text: WCAG AA (4.5:1) against the editor background.
            assert!(contrast(t.foreground, t.background) >= 4.5, "{kind:?} text contrast");
            // Code colors and muted text: at least 3:1.
            for (name, c) in [
                ("muted", t.foreground_muted),
                ("keyword", t.syntax.keyword),
                ("string", t.syntax.string),
                ("number", t.syntax.number),
                ("function", t.syntax.function),
                ("type", t.syntax.type_name),
            ] {
                assert!(contrast(c, t.background) >= 3.0, "{kind:?} {name} contrast {:.2}", contrast(c, t.background));
            }
            let bg: gpui::Hsla = t.background.into();
            assert_eq!(kind.is_light(), bg.l > 0.5, "{kind:?} light/dark flag");
        }
    }

    #[test]
    fn names_are_unique_and_settings_names_stable() {
        let names: std::collections::HashSet<_> = ThemeKind::ALL.iter().map(|k| k.display_name()).collect();
        assert_eq!(names.len(), ThemeKind::ALL.len());
        // Saved settings refer to themes by variant name.
        assert_eq!(serde_json::to_string(&ThemeKind::CustomOneDark).unwrap(), "\"CustomOneDark\"");
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::for_kind(ThemeKind::default())
    }
}
