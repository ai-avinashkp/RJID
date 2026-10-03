# Launch posts for RJID

Before posting:
- Links point to github.com/ai-avinashkp/RJID.
- Attach a screenshot or a short (≤ 30 s) screen recording to the first post; posts
  with media get far more reach.
- Free X accounts can post up to 280 characters per post, so the thread below keeps
  each one under that limit.

---

## X / Twitter: launch thread

**1/** (attach a screenshot or GIF)
```
I built a Java IDE in Rust.

RJID (Rust for Java IDE): a fast, GPU-rendered IDE for Java with Maven, Gradle, Spring Boot & JavaFX, a debugger and a real terminal, in one native window.

Source: github.com/ai-avinashkp/RJID
```

**2/**
```
Why? Java IDEs are powerful but heavy. I wanted one that opens instantly and never stutters while typing.

RJID is written from scratch in Rust on GPUI (the UI framework behind @zeddotdev) and drives the tools you already have: JDK, Maven, Gradle, jdtls.
```

**3/**
```
What it does today:
• Detects Spring Boot / JavaFX / plain Java and runs it right
• Completion with auto-import, hover docs, quick fixes, rename
• Generate getters/setters, constructors, toString
• Breakpoints + stepping
• Spring Boot → one-click localhost link
```

**4/**
```
New Project wizard: Spring Boot web, JavaFX or console Java, with Maven or Gradle, using the latest versions that match your JDK.

Plus IntelliJ-style update checks for JDK, Maven, Spring Boot & JavaFX. Safe patch updates only, always with a backup.
```

**5/**
```
It's an early preview, tested on Windows; macOS & Linux next.

Source-available (PolyForm Noncommercial): free for personal & noncommercial use.

Feedback, issues and ⭐ welcome → github.com/ai-avinashkp/RJID

#rustlang #java #springboot
```

---

## Single post (if you don't want a thread)

```
I built RJID — a Java IDE written in Rust 🦀☕

GPU-rendered, opens instantly. Maven, Gradle, Spring Boot & JavaFX run/debug, completion with auto-import, quick fixes, refactorings and a real terminal.

Early preview, source on GitHub:
github.com/ai-avinashkp/RJID
#rustlang #java
```

---

## Product Hunt (when you launch there)

- **Name:** RJID
- **Tagline (≤ 60 chars):** `A fast Java IDE written in Rust`
- **Topics:** Developer Tools, Code Editors, Java
- **Description:**
  > RJID (Rust for Java IDE) is a GPU-rendered Java IDE written in Rust. It detects
  > Maven, Gradle, Spring Boot and JavaFX projects and runs or debugs them in one
  > click. It has completion with auto-import, quick fixes, refactorings, a real
  > terminal and a New Project wizard. Free for personal and noncommercial use;
  > source on GitHub.
- **Maker's first comment:**
  > Hi Product Hunt! I love Java but not how heavy its IDEs feel, so I wrote one in
  > Rust on GPUI, the framework behind Zed. RJID doesn't reinvent Java tooling: it
  > drives the JDK, Maven, Gradle and the Eclipse JDT Language Server from one fast
  > native window. It's an early preview on Windows. I'd love to hear which Java
  > workflow you'd want next.
- **Gallery:** banner, Spring Boot run with the localhost link, hover docs, Generate
  dialog, debugger paused on a breakpoint.

---

## Reddit (r/rust, r/java): show-and-tell

**Title:** `I'm building RJID, a Java IDE written in Rust (GPUI)`

> I've been building a Java IDE in Rust on GPUI. It handles Maven, Gradle, Spring
> Boot and JavaFX projects. It has completion with auto-import, quick fixes, rename,
> code generation via jdtls, a JDWP debugger, and a VT terminal. It's an early
> preview, tested on Windows.
>
> Technical bits people may find interesting:
> - a JDWP client written from scratch;
> - a sandboxed wasmi plugin host;
> - signed self-update;
> - jdtls's `java/*` requests driven from a non-VS Code client.
>
> Source-available (PolyForm Noncommercial). Feedback very welcome: <link>

Tip: in r/rust lead with the implementation details; in r/java lead with the
workflow (Spring Boot run, debugging, generation).
