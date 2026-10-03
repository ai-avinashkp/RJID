//! Font families. GPUI has no generic "monospace" alias — an unknown family
//! silently falls back to the proportional UI font — so code and the
//! terminal name a monospace face that ships with each OS.

/// Monospace family guaranteed to exist on this platform.
pub const MONO: &str = if cfg!(target_os = "windows") {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};
