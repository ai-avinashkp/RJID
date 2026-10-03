# Changelog

All notable changes to RJID. Versions follow `MAJOR.MINOR.PATCH`; releases are
tagged `vX.Y.Z` on GitHub and are source-only.

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
