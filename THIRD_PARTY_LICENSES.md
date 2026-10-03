# Third-Party Licenses

RJID's own source code is source-available under the PolyForm Noncommercial License 1.0.0 (see `LICENSE`). The third-party components below remain under their own licenses, which permit this. Their copyright and license notices must be kept in any copy you share.

## gpui / gpui_platform

- **Project**: [Zed](https://github.com/zed-industries/zed) (Zed Industries, Inc.)
- **Crates used**: `gpui`, `gpui_platform` (and platform sub-crates `gpui_windows`, `gpui_macos`, `gpui_linux`, `gpui_web`, pulled in transitively)
- **Version pinned**: tag `v1.21.0`
- **License**: Apache License 2.0 — https://www.apache.org/licenses/LICENSE-2.0

These crates are explicitly marked `license = "Apache-2.0"` in their own `Cargo.toml`, distinct from the GPL-3.0-or-later license that covers Zed's own application code (editor, workspace, etc.). RJID depends on `gpui`/`gpui_platform` only, as a library, via their public API — no source from Zed's GPL-licensed crates is copied or vendored into this project.

## Other dependencies

Direct dependencies added since, all under permissive licenses (no
copyleft): `alacritty_terminal` (Apache-2.0), `portable-pty` (MIT),
`wasmi` (MIT OR Apache-2.0), `ureq` + `rustls` (MIT OR Apache-2.0 / ISC),
`ed25519-dalek` (BSD-3-Clause), `sha2`, `base64`, `rand`, `regex`,
`roxmltree`, `serde`, `serde_json`, `anyhow` (MIT OR Apache-2.0).

External tools the IDE runs as separate processes, never bundles or links:
the JDK, Maven, Gradle, Eclipse JDT Language Server (EPL-2.0), and the
Android SDK tools. Plugins are user-installed and carry their own licenses.

A full transitive license report (e.g. via `cargo-about`) should be
generated for each binary release (tracked in Phase 6 of `docs/ROADMAP.md`).
