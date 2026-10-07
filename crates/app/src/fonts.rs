//! Font families. GPUI has no generic "monospace" alias — an unknown family
//! silently falls back to the proportional UI font — so code and the
//! terminal name a monospace face that ships with each OS, unless the user
//! picked one of the well-known coding fonts installed on their system.

use gpui::SharedString;

/// Monospace family guaranteed to exist on this platform.
pub const MONO: &str = if cfg!(target_os = "windows") {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

/// Well-known coding fonts offered in Theme & Fonts (shown when installed),
/// with where to get the free ones.
pub const CODING_FONTS: &[(&str, Option<&str>)] = &[
    ("JetBrains Mono", Some("https://www.jetbrains.com/lp/mono/")),
    ("Fira Code", Some("https://github.com/tonsky/FiraCode")),
    ("Cascadia Code", Some("https://github.com/microsoft/cascadia-code")),
    ("Cascadia Mono", Some("https://github.com/microsoft/cascadia-code")),
    ("Source Code Pro", Some("https://github.com/adobe-fonts/source-code-pro")),
    ("Hack", Some("https://sourcefoundry.org/hack/")),
    ("IBM Plex Mono", Some("https://github.com/IBM/plex")),
    ("Iosevka", Some("https://typeof.net/Iosevka/")),
    ("Inconsolata", Some("https://fonts.google.com/specimen/Inconsolata")),
    ("Roboto Mono", Some("https://fonts.google.com/specimen/Roboto+Mono")),
    ("Ubuntu Mono", Some("https://design.ubuntu.com/font")),
    ("Consolas", None),
    ("Menlo", None),
    ("SF Mono", None),
    ("Monaco", None),
    ("DejaVu Sans Mono", None),
    ("Lucida Console", None),
    ("Courier New", None),
];

/// The family to use for a saved choice (`None` = platform default).
pub fn family_or_default(choice: Option<&str>) -> SharedString {
    choice.filter(|f| !f.trim().is_empty()).map_or_else(|| SharedString::from(MONO), |f| SharedString::from(f.to_string()))
}
