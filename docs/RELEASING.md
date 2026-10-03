# Releasing RJID

RJID releases are **source-only**. No `.exe` or installers are attached. GitHub
automatically adds `Source code (zip)` and `Source code (tar.gz)` to every release.
The pattern follows small, frequent, clearly-named releases.

## Naming

| Thing | Pattern | Example |
|---|---|---|
| Git tag | `vX.Y.Z` | `v0.1.0` |
| Release title | `RJID X.Y.Z` | `RJID 0.1.0` |
| Notes | 1–2 sentence summary, then bullets | see the template below |

Versioning:
- **patch** (`0.1.1`): fixes;
- **minor** (`0.2.0`): new features;
- **1.0.0**: once Windows, macOS and Linux are all tested.

## Steps

1. Update `version` in `crates/app/Cargo.toml` (the version shown in Help ▸ About
   and used by the self-updater) and add a section to `CHANGELOG.md`.
2. Check:
   ```bash
   cargo test --workspace
   cargo build --release -p rji-app
   ```
3. Commit and tag:
   ```bash
   git commit -am "RJID 0.1.0"
   git tag -a v0.1.0 -m "RJID 0.1.0"
   git push origin main --tags
   ```
4. Create the release (web UI, or with the GitHub CLI):
   ```bash
   gh release create v0.1.0 --title "RJID 0.1.0" --notes-file docs/release-notes/0.1.0.md
   ```
   Don't upload binaries.

## Release-notes template

```markdown
RJID 0.2.0 adds <the headline feature> and <second one>.

- New: …
- Improved: …
- Fixed: …

Build from source: see the README ("Get RJID").
RJID is source-available under the PolyForm Noncommercial License 1.0.0.
```

## Repository settings (once)

- **Description:** `RJID — Rust for Java IDE. A fast, GPU-rendered Java IDE written in Rust (Maven, Gradle, Spring Boot, JavaFX, debugger).`
- **Topics:** `rust`, `java`, `ide`, `gpui`, `spring-boot`, `javafx`, `maven`, `gradle`,
  `code-editor`, `jdtls`.
- **Social preview image** (Settings ▸ General): a 1280×640 PNG export of
  `docs/assets/rjid-banner.svg` or a real screenshot.
- **Disable "Packages"**, and leave release assets empty except GitHub's
  auto-generated source archives.
