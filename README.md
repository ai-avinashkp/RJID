<p align="center">
  <img src="docs/assets/rjid-banner.svg" alt="RJID — Rust for Java IDE" width="100%">
</p>

<p align="center">
  <b>A fast, GPU-rendered Java IDE written in Rust.</b><br>
  Maven · Gradle · Spring Boot · JavaFX · debugger · terminal · in one native window.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: PolyForm Noncommercial 1.0.0" src="https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-orange"></a>
  <img alt="Written in Rust" src="https://img.shields.io/badge/written%20in-Rust-b7410e">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20(macOS%2FLinux%20experimental)-555">
  <img alt="Status" src="https://img.shields.io/badge/status-early%20preview-yellow">
</p>

---

## Why RJID

Java IDEs are powerful but heavy. RJID is an experiment in the other direction: a
Java IDE that starts in a moment and stays responsive, written from scratch in Rust
on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the GPU UI
framework behind the Zed editor). It doesn't reinvent Java tooling. It drives the
tools you already have (the JDK, Maven, Gradle and the Eclipse JDT Language Server)
from one fast native window.

<p align="center">
  <img src="docs/assets/spring-boot-run.png" alt="RJID running a Spring Boot app: the toolbar shows the detected project type and a localhost:8080 link while Tomcat logs stream in the terminal" width="100%">
  <br><sub>One click on ▶ runs a Spring Boot app; when Tomcat is up, a <b>localhost:8080 ↗</b> link appears in the toolbar.</sub>
</p>

<table>
  <tr>
    <td width="50%"><img src="docs/assets/editor.png" alt="Editing a Spring REST controller with syntax highlighting and an error underlined"></td>
    <td width="50%"><img src="docs/assets/theme-high-contrast.png" alt="The high-contrast dark theme"></td>
  </tr>
  <tr>
    <td align="center"><sub>Java editing with live diagnostics from jdtls</sub></td>
    <td align="center"><sub>One of 5 built-in themes (high contrast)</sub></td>
  </tr>
</table>

## Features

- **Knows your project.** Detects plain Java, Maven, Gradle, Spring Boot (and
  whether it serves HTTP), JavaFX and Android. Build / Test / ▶ Run do the right
  thing for each.
- **New Project wizard.** Java console, Spring Boot web or JavaFX, with Maven,
  Gradle or no build tool. Library versions are matched to your JDK.
- **Java intelligence** via the Eclipse JDT Language Server:
  - completion that adds the `import` for you;
  - hover with signatures and Javadoc;
  - error squiggles and quick fixes;
  - rename across files and extract refactorings;
  - generate constructor, getters/setters, `toString`, `equals`/`hashCode`.
- **Debugger.** Breakpoints, step over/into/out, variables and call stack, for
  `java`, `mvn spring-boot:run`, `exec:java`, tests, or attaching to a running JVM.
- **Real terminal.** A VT emulator (`alacritty_terminal`) with colors and full-screen
  apps. Spring Boot's "started on port" shows up as a one-click
  **localhost ↗** link.
- **Toolchain updates.** Checks JDK, Maven, Spring Boot and JavaFX like IntelliJ:
  notifies by default, auto-applies only safe patch updates if you opt in, keeps
  backups, and never installs a JDK on its own.
- **Comfortable UI.** Custom title bar with menus, 5 themes, zoom, movable panels
  (tree left/right, terminal bottom/right), drag-and-drop folders, and a responsive
  layout down to 400×500.
- **Portable.** All settings live next to the app (`.rji-settings/`), plus a
  per-project `.rji/` folder. Nothing is scattered across your system.
- **Extensible.** Sandboxed WebAssembly plugins (no file, network or process access).
- **Android (early).** Device and emulator list, build → install → run, logcat.

## Get RJID

RJID is distributed as **source only** for now. Build it yourself:

```bash
git clone https://github.com/ai-avinashkp/RJID.git
cd RJID
cargo run --release -p rji-app
```

**Requirements**

| To build | To use the Java features |
|---|---|
| Rust 1.88+ (edition 2024) | A JDK (17+ recommended) |
| Windows 10/11 (macOS/Linux untested) | Maven and/or Gradle for those projects |
| | jdtls: the VS Code *Language Support for Java* extension, or `JDTLS_HOME` pointing at a [jdtls download](https://download.eclipse.org/jdtls/) |

The first build downloads and compiles GPUI and takes a few minutes; rebuilds are
quick. The binary is `target/release/rjid` (`rjid.exe` on Windows):
`rjid <folder>` opens a project, `rjid <File.java>` opens a file in its project.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+Space | Completion |
| Ctrl+. / F2 | Quick fix / rename symbol |
| Alt+Insert or right-click | Generate, refactor, imports, format |
| Shift+Alt+O / Shift+Alt+F | Organize imports / format document |
| Ctrl+F / Ctrl+H / Ctrl+G | Find / replace / go to line |
| F5 / Shift+F5 | Debug / stop |
| F8 / F10 / F11 / Shift+F11 | Continue / step over / into / out |
| Ctrl+B / Ctrl+` | Toggle file tree / terminal |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom |

All shortcuts are listed under Help ▸ Keyboard Shortcuts.

## Status

RJID is an **early preview**. It's developed and tested on Windows; macOS and Linux
builds are written but untested. What works, how it was verified, and the honest list
of limitations are in [docs/ROADMAP.md](docs/ROADMAP.md). Changes are recorded in
[CHANGELOG.md](CHANGELOG.md).

## License

RJID is **source-available, not open source.** It's licensed under the
[PolyForm Noncommercial License 1.0.0](LICENSE):

- ✅ Free for personal use, study, research, hobby projects, and noncommercial
  organizations (charities, schools, public research, government bodies).
- ✅ You may read, build, modify and share it for those purposes, keeping the
  license and copyright notice.
- ❌ **Commercial use is not permitted** without a separate license. That includes
  using it inside a company for business, selling it, bundling it, or offering it as
  a service.

For a commercial license, contact the author (see the copyright notice in
[LICENSE](LICENSE)).

RJID bundles no third-party IDE code. It's built on Apache-2.0 / MIT / BSD
libraries (GPUI, alacritty_terminal, wasmi and others) listed in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). The JDK, Maven, Gradle and
jdtls are run as separate programs that you install yourself.

## Contributing

Issues and ideas are welcome. Before sending a pull request, please read
[CONTRIBUTING.md](CONTRIBUTING.md). Contributions have to be licensable under the
project's terms, including future commercial licensing.
