# RJID (Rust for Java IDE) Roadmap

The living status of the project: what's built and how it was verified,
what's deliberately out of scope, and what's next.

## Status at a glance

| Area | State |
|---|---|
| Editor (selection, clipboard, undo/redo, auto-indent, highlighting, find/replace, go to line) | Done, unit-tested |
| Code completion (jdtls + offline fallback) | Done, verified against real jdtls |
| Diagnostics (jdtls) | Done, verified |
| Debugger (breakpoints, step over/into/out, variables, call stack, attach) | Done, verified against a real JVM |
| Project-type detection (plain Java, Spring Boot / web, JavaFX, Android; Maven, Gradle, none) | Done, unit-tested |
| Maven / Gradle / Spring Boot / JavaFX run, build and debug | Done |
| New Project wizard (console Java, Spring Boot web, JavaFX × Maven/Gradle/none) | Done; Maven + plain verified by building and running |
| Hover info (diagnostics + jdtls signature/Javadoc) and error underlines | Done, verified against real jdtls |
| Auto-import, quick fixes, rename, refactor, generate (constructor, getters/setters, toString, equals/hashCode), organize imports, format | Done; jdtls request/response shapes verified against real jdtls, editing logic unit-tested |
| Run Configurations panel | Done |
| Movable panels (tree left/right, terminal bottom/right), drag-and-drop of files/folders (this or a new window) | Done |
| Terminal (VT grid via `alacritty_terminal`) | Done, unit-tested + visually verified |
| Custom titlebar with menus, responsive layout, 400×500 minimum | Done, visually verified |
| Toolchain update checks (JDK, Maven, wrapper, Spring Boot, JavaFX) | Done, verified against live sources |
| IDE self-update (signed manifest, staged swap on restart) | Done, verified end to end; needs release hosting |
| WASM plugins + signed plugin registry | Done, tested; no public registry yet |
| Android (SDK, devices, AVDs, build/install/run, logcat) | Done, verified against a local SDK |
| macOS / Linux | Code is `cfg`-portable; **never built or run there** |
| Installers (MSI/DMG/AppImage) | Not started |

## How it's verified

- `cargo test --workspace`: unit tests for:
  - the text buffer, the highlighter and search/replace;
  - completion, the terminal emulator and key encoding;
  - LSP and JDWP parsing, debug launch rewriting;
  - version classification and POM/Gradle rewriting;
  - signature verification and the self-update download/stage/apply cycle (against a local HTTP server);
  - the plugin sandbox (fuel and memory limits, refused imports) and registry install/rollback;
  - Android output parsing, settings and file-open safety.
- `cargo test -p rji-app -- --ignored debug_session_against_real_jvm`
  runs `DebugSession` against a real JVM (breakpoints, step into/over/out,
  variables).
- `cargo test -p rji-android -- --ignored` uses the real local SDK.
- `cargo run -p rji-jdwp-client --example smoke`,
  `cargo run -p rji-lsp-client --example completion_smoke -- <project> <java.exe>`,
  `cargo run -p rji-updates --example check -- <project>`.
- Manually and visually checked:
  - layout, themes and zoom;
  - the titlebar at full width and at the 400×500 minimum;
  - terminal rendering;
  - the ⬆ update indicator;
  - a clean shutdown.

## What's built

### Editor (`crates/editor`, `crates/app/src/editor_view.rs`)
- `TextBuffer` is a GPUI-free model (caret, selection anchor,
  edit-based undo/redo with word-sized grouping). Every editing rule is
  unit-tested without a window.
- Keys:
  - Selection and navigation: Shift+arrows/Home/End select; Ctrl+arrows move by word; Ctrl+Home/End; PageUp/Down.
  - Clipboard: Ctrl+A/C/X/V. Copy and cut take the whole line when nothing is selected.
  - Undo and redo: Ctrl+Z/Y/Shift+Z.
  - Indentation: Tab/Shift+Tab indent and outdent, including blocks. Enter auto-indents and splits `{}`. Backspace removes a whole indent level.
  - Other: Ctrl+Backspace deletes a word; Ctrl+D duplicates; Ctrl+/ toggles comments.
- Find/replace bar (Ctrl+F / Ctrl+H) with case, whole-word and regex
  (`$1` in replacements) toggles. Go to line (Ctrl+G).
- Mouse:
  - click, Shift+click, drag-select;
  - double-click selects a word, triple-click selects a line;
  - clicking the gutter toggles a breakpoint.
- Highlighting covers Java (and C-like files), XML, `#`-comment configs and plain text. Multi-line comments and text blocks are tracked per line.
- File safety:
  - CRLF files stay CRLF.
  - Binary, non-UTF-8 and >20 MB files are refused instead of being opened.
  - Open tabs are reloaded after an update rewrites their file. There's no general file watcher yet, so other external edits aren't picked up.

### Window chrome (`crates/app/src/root/titlebar.rs`)
- Zed-style integrated titlebar: File / Edit / Selection / View / Go /
  Run / Terminal / Plugins / Help menus, the workspace title, and native
  window buttons on Windows (`WindowControlArea` hit-testing, so snap
  layouts and dragging work).
- Responsive layout:
  - Below 820 px the menus collapse into ☰.
  - Below 640 px the file tree hides; Ctrl+B forces it back.
  - The toolbar and status bar drop secondary items as the window narrows.
  - The window can't shrink below 400×500.
- Alt+letter opens a menu; Escape closes any overlay. File tree right-click
  menu: new file/folder (a `.java` file gets a package template), rename,
  delete, reveal, copy path.

### Project types and running (`crates/project-java/src/project.rs`)
- On opening a folder the IDE reads the build script and scans the sources (bounded) to classify the project:
  - build tool: Maven (or its `mvnw` wrapper), Gradle (or `gradlew`), or none;
  - frameworks: Spring Boot, plus whether it has an embedded web server; JavaFX; Android;
  - entry points: `@SpringBootApplication`, JavaFX `Application` subclasses, and `main` methods.
- The toolbar shows the type (e.g. "Spring Boot · Web · Maven") with Build and Test. Run / Debug use the best run task automatically until you add your own run config. The Run menu lists every task, including one per main class.
- Run tasks: `spring-boot:run` / `bootRun`, `javafx:run` / `run`, `exec:java "-Dexec.mainClass=…"`, or `javac` + `java -cp out` for plain sources. Debug works for each of them.
- Spring Boot: the terminal output is watched for "Tomcat/Netty/Jetty started on port N", which shows a "● localhost:N ↗" button that opens the browser. "Port N was already in use" gets a hint. Running a Spring Boot app that has no web starter shows why it exits without a port.
- Notices appear as a toast above the status bar (click to dismiss).

### New Project (`crates/project-java/src/templates.rs`, `root/new_project_ui.rs`)
- File ▸ New Project… (or the welcome page): Java console, Spring Boot web (REST controller, DevTools, test), or JavaFX. Built with Maven, Gradle (Kotlin DSL), or no build tool (console only).
- Versions are the latest stable on Maven Central at creation time that suit the detected JDK, with offline fallbacks. Examples: Spring Boot 4 uses `-webmvc`; JavaFX is limited to releases the JDK supports.
- Never overwrites an existing folder; opens the new project and its main class.
- Gradle templates have no wrapper (it is a binary), so they need `gradle` on PATH.

### Completion and language server
- Completion opens as you type in Java files (and on `.`), or with
  Ctrl+Space. jdtls items are merged with an instant offline fallback:
  keywords, snippets (`psvm`, `sout`, `fori`, …) and words in the file.
- jdtls is found via `JDTLS_HOME` (a standalone download) or the Java
  extension of VS Code, VS Code Insiders, VSCodium or Cursor. The newest version wins, using the OS-specific config dir. It runs as a separate process and is never bundled.
- Server-to-client requests are always answered. Switching workspaces
  shuts the old server down.
- Code actions (`editor_view/code_actions.rs`), from right-click, the Edit menu or shortcuts:
  - **Auto-import:** accepting a class from completion adds its import. When jdtls reports "X cannot be resolved to a type", unambiguous imports are added automatically, but only if the edit touches import lines alone. Ambiguous names (`List`, `Date`) offer every candidate as a quick fix.
  - **Quick Fix (Ctrl+.):** also shown inside the hover popup.
  - **Rename Symbol (F2):** across files; open tabs are edited in place, other files on disk.
  - **Refactor (Ctrl+Shift+R):** extract variable, method or field; surround with try/catch.
  - **Generate (Alt+Insert):** constructor, getters/setters, toString, equals/hashCode. Each opens a dialog to pick fields first. These use the jdtls `java/*` requests the VS Code Java extension uses.
  - **Organize Imports (Shift+Alt+O), Add Missing Imports, Format Document (Shift+Alt+F).**
  - Edits arrive as one undo step, and jdtls tabs are converted to spaces.
- Hover: resting the mouse on a symbol for about 0.45 s shows its signature (highlighted) and Javadoc from jdtls, fetched only then. Errors and warnings are underlined; hovering one (or its tinted line number) shows the message. The popup stays inside the editor and scrolls.

### Debugger (`crates/jdwp-client`, `crates/app/src/debug_session.rs`)
- Debug (F5) launches the active run config under the JDWP agent:
  - plain `java …`;
  - `mvn spring-boot:run`;
  - `mvn test`/`verify` (surefire);
  - `mvn exec:java` (via `MAVEN_OPTS`);
  - Gradle `run`/`bootRun`/`test` (`--debug-jvm`).
- **Attach…** connects to an already-running JVM.
- Breakpoints are synced live. Ones in classes that aren't loaded yet arm when the class loads.
- Continue (F8), Step Over (F10), Step Into (F11), Step Out (Shift+F11), Stop.
- On a stop the IDE shows the paused line, local variables and a clickable
  call stack.

### Terminal (`crates/terminal`, `crates/app/src/terminal_view.rs`)
- PTY shell plus an `alacritty_terminal` grid, so full-screen programs, colors (16/256/truecolor), the cursor and the alternate screen all render.
- Device-status replies are written back to the PTY, which ConPTY needs.
- The terminal resizes with its panel.
- Mouse selection: drag, double-click a word, triple-click a line. Right-click copies the selection, or pastes when nothing is selected. Ctrl+C copies when there's a selection and sends SIGINT otherwise.
- Bracketed paste, a scrollback thumb, and Shift+PgUp/PgDn.
- IDE shortcuts still work while the terminal is focused.

### Toolchain updates (`crates/updates`, `root/updates_ui.rs`)
Modelled on IntelliJ: check in the background, show what's available, and change nothing without consent.
- Sources and scope:
  - Maven Central for Spring Boot, JavaFX and Maven; the Adoptium API for the JDK.
  - The check runs at startup and every `check_interval_hours`, only if *Check automatically* is on (default on).
  - Results show as a ⬆ count in the status bar and in the Updates panel (Help → Check for Updates).
- What it rewrites in the project:
  - `pom.xml`: the parent and every `org.springframework.boot` pin together; `${property}` versions are resolved and edited at the property; openjfx dependencies.
  - `build.gradle[.kts]`: plugins, coordinates and the `javafx {}` block.
  - `.mvn/wrapper/maven-wrapper.properties`.
- How each update is applied:
  - Every edit verifies the exact bytes it replaces and writes a `.rji-backup` first.
  - Files with unsaved edits are skipped.
  - Major upgrades (e.g. Spring Boot 3→4) always ask first.
  - *Auto-apply safe updates* (default **off**) applies only patch-level bumps within the same major/minor line.
  - Updates can be ignored per item.
- JDK and system Maven updates are **never** installed silently. The IDE
  shows the new version and opens the official download page.

### IDE self-update (`crates/updates/src/self_update.rs`)
- On the update check, the IDE fetches a signed release manifest from `RJI_UPDATE_URL`.
- The manifest must be Ed25519-signed by the key baked in at build time
  (`RJI_UPDATE_PUBKEY`). Unsigned or tampered manifests are rejected.
- The binary for this target is downloaded and checked against the
  manifest's SHA-256 and size, then staged next to the executable.
- On restart (or the next launch) the staged binary is swapped in, with
  rollback if the swap fails.
- Auto-download is opt-in. Builds made without the two env vars show
  "updates not configured" and never touch the network for this.

Release workflow (`cargo run -p rji-updates --bin rji-release -- …`):
1. `keygen <secret-file>` once. Keep the secret offline, never in the repo,
   and build releases with `RJI_UPDATE_PUBKEY=<printed key>` and
   `RJI_UPDATE_URL=<manifest url>`.
2. `hash <binary>` for each platform asset. Write `manifest.json`
   (version, notes, assets with url/sha256/size/target).
3. `sign manifest.json <secret-file>` and publish the output at the URL.

### Plugins (`crates/plugins`, `root/plugins_ui.rs`, `plugins/java-tools`)
- Sandboxing (plugins are WebAssembly modules run by `wasmi`):
  - no host imports at all, so no filesystem, network or process access;
  - fuel-limited CPU, 64 MB memory and 16 MB I/O caps;
  - a plugin that misbehaves fails its command without crashing the IDE.
- ABI v1: `alloc` + `run(ptr,len) -> packed ptr/len`, with JSON in and out.
  Commands transform the selection (or the whole file) or show a message,
  and appear under the Plugins menu.
- Plugins live in the portable `.rji-settings/plugins/`. Install from a folder,
  uninstall and reload in the Plugins panel.
- Registry: a signed `plugins.json` index (`RJI_PLUGIN_REGISTRY_URL`,
  `rji-release sign-index`). Installs are hash-verified, staged and swapped with rollback. Auto-update is opt-in.
- Sample plugin `plugins/java-tools` (getters/setters, sort lines,
  camel↔snake, case, count) shows the ABI. Build it with
  `cargo build --release --target wasm32-unknown-unknown` in that folder.

### Android (`crates/android`, `root/android_ui.rs`)
- Finds the SDK through `ANDROID_HOME` or `ANDROID_SDK_ROOT`, then the default install location. It lists devices and AVDs, and launches emulators.
- For Gradle Android projects: build, install and launch on a device, install an APK, and stream logcat. Each command runs in the terminal.

### Workspace & settings
- All app state lives in the portable `<app_dir>/.rji-settings/`, and per-project state lives in `<project>/.rji/`. That covers theme, zoom, panel sizes, recents, update preferences, ignored updates, run configs and plugins. Corrupt JSON falls back to defaults.
- Unsaved work prompts on tab close, folder switch and window close.
  Closing stops jdtls, the debuggee and the shell explicitly.

## Known limitations (honest list)

- **Platforms**: only Windows has been built and run. The macOS/Linux
  paths (`java` vs `java.exe`, `gradlew`, `sh -c`, jdtls config dirs,
  `MAVEN_OPTS=…` syntax) are written and `cfg`-gated but untested. The
  titlebar relies on the native frame on macOS/Linux.
- **Interactive UI is not auto-tested**: GPUI ignores synthetic window
  messages, so menus, dropdowns, panels, the find bar, prompts and the
  context menu are verified by hand only.
- **Debugger**: no conditional breakpoints, watches or object-field
  expansion. Breakpoints don't follow lines moved by later edits.
- **Editor**: no multi-cursor, line wrapping, IME composition (CJK) or
  tree-sitter. Completion doesn't add imports. No quick-fixes (code actions) on errors yet.
- **Gradle templates** are generated but untested here (no Gradle installed on the dev machine).
- **Updates/plugins** need real hosting and a release key before the public
  can use them. Plugins have no host API beyond text-in/text-out yet.
- No installers, crash reporting or settings UI. The settings files are
  JSON and are edited by hand or through the panels.

## GPUI notes (read before changing `RootView::render`)

- Flex-row children don't stretch by default; use `.flex_1()`/`.w_full()`
  and `.min_w_0()` down the chain.
- Give every panel an explicit pixel size from the viewport. Without one, siblings below the
  tree/editor row failed to paint.
- `uniform_list` sizes its width from one item; point
  `with_width_from_item` at the longest line.
- The Windows backend paints no scrollbar thumbs; `scrollbar.rs` draws them.
- The `"monospace"` family isn't resolved on Windows. Use `fonts::MONO`.
- `window.prompt` must never be called while another prompt is open.
- Overlays use `deferred(...).with_priority(n)` so they paint above panels.
- Rust 2024: helpers returning `impl IntoElement` that take `cx` need
  `+ use<>`.

## Next

- Build and smoke-test on macOS and Linux (CI matrix); fix what breaks.
- Installers (MSI, DMG, AppImage/deb) plus a `cargo-about` license report per
  release, and set up release hosting with the signing key.
- Debugger: conditional breakpoints, watches, object expansion.
- Plugin host API v2: read-only workspace queries, diagnostics, and
  capability-scoped permissions declared in `plugin.json`.
- Editor: multi-cursor, wrapping, tree-sitter, import-on-complete.
- Android: build variants, device file explorer, layout preview.
