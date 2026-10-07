# Changelog

All notable changes to RJID. Versions follow `MAJOR.MINOR.PATCH`; releases are
tagged `vX.Y.Z` on GitHub and are source-only.

## Unreleased

- Java language server set up automatically on first use (verified download from
  eclipse.org, with your consent).
- Go to Definition (F12 / Ctrl+Click, including JDK sources), Go to File (Ctrl+P)
  and Command Palette (Ctrl+Shift+P).
- Source Action menu: generate, override/implement, delegate methods, and more.
- Terminal tabs; `exit` closes a terminal; build results colored green / red.
- Java-aware project tree: compact package folders, New Java Class / Interface /
  Enum / Record / Package.
- Build-file changes re-import with a progress indicator; missing dependency
  versions are fixed automatically.
- Theme & Fonts: 13 themes and your choice of coding fonts, with live preview.
- Auto-save on focus change; tidier Run, Edit and Plugins menus.

## 0.1.0 (first public preview)

RJID (Rust for Java IDE) is a Java IDE written in Rust on GPUI.

- **Projects:** detects plain Java, Maven, Gradle, Spring Boot (web or not), JavaFX
  and Android, with matching Build / Test / Run / Debug tasks.
- **New Project wizard:** Java console, Spring Boot web or JavaFX, with Maven,
  Gradle or no build tool.
- **Editor:**
  - syntax highlighting, find/replace, go to line, undo/redo, auto-indent,
    comment toggling;
  - completion with auto-import;
  - hover docs, diagnostics with quick fixes, rename, extract refactorings;
  - generate constructor, getters/setters, toString, equals/hashCode;
  - organize imports and formatting.
- **Debugger:** breakpoints, stepping, variables, call stack, attach to a JVM.
- **Terminal:** a VT emulator. Spring Boot's "started on port" becomes a
  one-click localhost link.
- **Toolchain update checks:** JDK, Maven, Spring Boot and JavaFX; safe patch
  auto-apply is opt-in.
- **UI:** custom title bar and menus, 5 themes, zoom, movable panels,
  drag-and-drop, run configurations panel.
- **Plugins and Android:** sandboxed WebAssembly plugins; early Android support
  (devices, emulators, install and run, logcat).
- **License:** PolyForm Noncommercial 1.0.0.
