//! Integrated terminal panel: a real PTY shell driving a real VT emulator
//! (`rji_terminal::Emulator`, built on `alacritty_terminal`). Colors,
//! cursor addressing, full-screen programs, scrollback, mouse selection,
//! and copy/paste all work like a standalone terminal.

use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, FontWeight, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    Rgba, ScrollDelta, ScrollWheelEvent, Window, canvas, div, font, prelude::*, px, rgb,
};
use rji_terminal::{CellStyle, Emulator, PtySession, SelectKind, TermColor, default_shell, key_bytes, xterm_256};
use rji_theme::Theme;

const LINE_HEIGHT: f32 = 18.0;
const FONT_SIZE: f32 = 13.0;
const PADDING: f32 = 6.0;
pub const HEADER_HEIGHT: f32 = 26.0;

pub struct TerminalView {
    session: Option<PtySession>,
    cwd: PathBuf,
    emulator: Emulator,
    pub theme: Theme,
    focus_handle: FocusHandle,
    /// UI zoom factor, set by `RootView`; font and cell sizes scale with it.
    pub zoom: f32,
    /// Where the text grid was painted last frame (for mouse hit-testing
    /// and to size the PTY to the panel).
    body_bounds: Option<Bounds<Pixels>>,
    cell_width: f32,
    selecting: bool,
    /// Recent output with escape sequences removed, scanned for server
    /// start-up lines (see `server_events`).
    output_tail: String,
    /// The shell is started on the first real measurement of the panel,
    /// at that exact size: starting it at a guessed size and resizing at
    /// once leaves ConPTY and the emulator disagreeing about the screen
    /// (blank rows above the prompt, a banner lost to scrollback).
    shell_started: bool,
    /// Input sent before the shell started (a Run command), flushed then.
    pending_input: Vec<u8>,
}

/// Panel heights below this many rows are transient layout states (the
/// window opening, a splitter drag) — never sized to.
const MIN_FIT_LINES: usize = 3;

/// Things the root view wants to know about what runs in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// An embedded web server (Tomcat, Netty, Jetty, Undertow) reported
    /// that it is listening.
    ServerStarted(u16),
    /// A server couldn't start because its port is taken.
    PortInUse(u16),
}

impl gpui::EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    pub fn new(cwd: PathBuf, theme: Theme, cx: &mut Context<Self>) -> Self {
        let view = TerminalView {
            session: None,
            cwd,
            emulator: Emulator::new(120, 32),
            theme,
            focus_handle: cx.focus_handle(),
            zoom: 1.0,
            body_bounds: None,
            cell_width: 8.0,
            selecting: false,
            output_tail: String::new(),
            shell_started: false,
            pending_input: Vec::new(),
        };
        // No shell yet: `fit_to` starts it once the panel has a real size.
        view
    }

    fn start_shell(&mut self, cx: &mut Context<Self>) {
        match PtySession::spawn(&default_shell(), &[], &self.cwd) {
            Ok(session) => {
                let _ = session.resize(self.emulator.lines() as u16, self.emulator.columns() as u16);
                let rx = session.output_rx.clone();
                let mut session = session;
                if !self.pending_input.is_empty() {
                    let _ = session.write_input(&std::mem::take(&mut self.pending_input));
                }
                self.session = Some(session);
                cx.spawn(async move |this, cx| {
                    while let Ok(chunk) = rx.recv().await {
                        let alive = this
                            .update(cx, |view: &mut TerminalView, cx| {
                                let reply = view.emulator.feed(&chunk);
                                for event in view.scan_output(&chunk) {
                                    cx.emit(event);
                                }
                                if !reply.is_empty()
                                    && let Some(session) = view.session.as_mut()
                                {
                                    let _ = session.write_input(&reply);
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                    // The shell exited on its own (e.g. `exit`).
                    this.update(cx, |view, cx| {
                        view.session = None;
                        view.emulator
                            .feed(b"\r\n\x1b[2m[process exited \xe2\x80\x94 press Restart to start a new shell]\x1b[0m\r\n");
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Err(err) => {
                self.emulator
                    .feed(format!("Failed to start terminal ({}): {err}\r\n", default_shell()).as_bytes());
            }
        }
    }

    pub fn restart(&mut self, cx: &mut Context<Self>) {
        // Dropping the old session kills its shell.
        self.session = None;
        let (cols, lines) = (self.emulator.columns(), self.emulator.lines());
        self.emulator = Emulator::new(cols, lines);
        self.start_shell(cx);
        cx.notify();
    }

    /// Kills the shell (and whatever it's running) — used on app close.
    pub fn shutdown(&mut self) {
        self.session = None;
    }

    /// Clears screen and scrollback, then asks the shell to redraw its
    /// prompt (Ctrl+L) — without submitting whatever is typed.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.emulator.feed(b"\x1b[3J\x1b[2J\x1b[H");
        self.write(b"\x0c");
        cx.notify();
    }

    fn write(&mut self, bytes: &[u8]) {
        match self.session.as_mut() {
            Some(session) => {
                let _ = session.write_input(bytes);
            }
            // Not started yet (panel not measured): keep it for the shell.
            None if !self.shell_started => self.pending_input.extend_from_slice(bytes),
            None => {}
        }
    }

    /// Appends a chunk of output to the scan buffer and reports any server
    /// start-up lines completed by it. Only whole lines are scanned, so a
    /// line split across reads is seen once, when its newline arrives.
    fn scan_output(&mut self, chunk: &[u8]) -> Vec<TerminalEvent> {
        self.output_tail.push_str(&strip_escapes(&String::from_utf8_lossy(chunk)));
        let Some(last_newline) = self.output_tail.rfind('\n') else {
            if self.output_tail.len() > 4096 {
                self.output_tail.clear();
            }
            return Vec::new();
        };
        let complete: String = self.output_tail.drain(..=last_newline).collect();
        complete.lines().filter_map(server_event).collect()
    }

    /// Sends literal text to the shell as if typed — used by Run/Build.
    pub fn send_text(&mut self, text: &str) {
        self.emulator.scroll_to_bottom();
        self.emulator.clear_selection();
        self.write(text.as_bytes());
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        self.emulator.scroll_to_bottom();
        if self.emulator.bracketed_paste() {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.as_bytes());
        }
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.emulator.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.emulator.clear_selection();
                true
            }
            None => false,
        }
    }

    fn line_height(&self) -> f32 {
        LINE_HEIGHT * self.zoom
    }

    /// Screen (row, column) under a window position, and whether it's on
    /// the right half of that cell. Positions outside the grid clamp to its
    /// edges (so dragging past the panel still extends the selection).
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let bounds = self.body_bounds?;
        let x = (f32::from(position.x - bounds.left()) - PADDING).max(0.0);
        let y = f32::from(position.y - bounds.top()).max(0.0);
        let row = (y / self.line_height()) as usize;
        let exact = x / self.cell_width;
        let col = exact as usize;
        let last_col = self.emulator.columns() - 1;
        let right_half = col > last_col || exact.fract() >= 0.5;
        Some((row.min(self.emulator.lines() - 1), col.min(last_col), right_half))
    }

    /// Sizes the emulator + PTY to the panel (called after layout).
    fn fit_to(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.body_bounds = Some(bounds);
        let cols = ((f32::from(bounds.size.width) - 2.0 * PADDING) / self.cell_width).floor().max(2.0) as usize;
        let lines = (f32::from(bounds.size.height) / self.line_height()).floor().max(1.0) as usize;
        if lines < MIN_FIT_LINES {
            return;
        }
        if !self.shell_started {
            self.shell_started = true;
            self.emulator.resize(cols, lines);
            self.start_shell(cx);
            cx.notify();
            return;
        }
        if cols != self.emulator.columns() || lines != self.emulator.lines() {
            self.emulator.resize(cols, lines);
            if let Some(session) = self.session.as_ref() {
                let _ = session.resize(lines as u16, cols as u16);
            }
            cx.notify();
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        let ctrl = m.control;
        let key = k.key.as_str();

        // IDE-wide shortcuts stay with the IDE.
        let ide = (ctrl && matches!(key, "=" | "+" | "-" | "0" | "`" | "b" | "o" | "w" | "tab"))
            || (!ctrl && matches!(key, "f5" | "f8" | "f10" | "f11"))
            || (m.alt && !ctrl && key.len() == 1);
        if ide {
            return;
        }

        match key {
            // Copy: Ctrl+Shift+C, or Ctrl+C while text is selected (like
            // Windows Terminal); otherwise Ctrl+C interrupts.
            "c" if ctrl && (m.shift || self.emulator.selected_text().is_some()) => {
                self.copy_selection(cx);
            }
            "v" if ctrl => self.paste(cx),
            "pageup" if m.shift => self.emulator.scroll(self.emulator.lines() as i32 - 1),
            "pagedown" if m.shift => self.emulator.scroll(-(self.emulator.lines() as i32 - 1)),
            _ => {
                let bytes = key_bytes(key, ctrl, m.alt, m.shift, self.emulator.app_cursor()).or_else(|| {
                    if ctrl || m.function {
                        return None;
                    }
                    let text = k.key_char.as_ref()?;
                    let mut bytes = Vec::new();
                    if m.alt {
                        bytes.push(0x1b);
                    }
                    bytes.extend_from_slice(text.as_bytes());
                    Some(bytes)
                });
                let Some(bytes) = bytes else {
                    return;
                };
                self.emulator.scroll_to_bottom();
                self.emulator.clear_selection();
                self.write(&bytes);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        if let Some((row, col, right_half)) = self.cell_at(event.position) {
            let kind = match event.click_count {
                2 => SelectKind::Word,
                3 => SelectKind::Line,
                _ => SelectKind::Simple,
            };
            // A plain press starts an empty selection that grows as you
            // drag; double/triple-click selects a word/line at once.
            self.emulator.start_selection(row, col, right_half, kind);
            self.selecting = kind == SelectKind::Simple;
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        if let Some((row, col, right_half)) = self.cell_at(event.position) {
            self.emulator.extend_selection(row, col, right_half);
            cx.notify();
        }
    }

    /// Right-click: copy the selection if there is one, else paste.
    fn on_right_click(&mut self, cx: &mut Context<Self>) {
        if !self.copy_selection(cx) {
            self.paste(cx);
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => (delta.y * 3.0).round() as i32,
            ScrollDelta::Pixels(delta) => (f32::from(delta.y) / self.line_height()).round() as i32,
        };
        if lines == 0 {
            return;
        }
        if self.emulator.alt_screen() {
            // Full-screen programs (less, vim) have no scrollback: send
            // arrow keys instead, as other terminals do.
            let key = if lines > 0 { "up" } else { "down" };
            if let Some(bytes) = key_bytes(key, false, false, false, self.emulator.app_cursor()) {
                for _ in 0..lines.unsigned_abs() {
                    self.write(&bytes);
                }
            }
        } else {
            self.emulator.scroll(lines);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Maps a terminal color through the theme: the 16 ANSI colors get a
    /// palette tuned per theme (dark vs light), the rest are exact.
    fn color(&self, color: TermColor, style: &CellStyle) -> Hsla {
        let theme = self.theme;
        let rgba: Rgba = match color {
            TermColor::DefaultFg => theme.foreground,
            TermColor::DefaultBg => theme.surface,
            TermColor::Indexed(i) if i < 16 => ansi_color(i, is_light(theme)),
            TermColor::Indexed(i) => {
                let (r, g, b) = xterm_256(i);
                rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
            }
            TermColor::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        };
        let hsla: Hsla = rgba.into();
        if style.dim { hsla.opacity(0.6) } else { hsla }
    }
}

fn is_light(theme: Theme) -> bool {
    let bg: Hsla = theme.background.into();
    bg.l > 0.5
}

/// A readable 16-color ANSI palette (darker variants on light themes).
fn ansi_color(index: u8, light: bool) -> Rgba {
    const DARK: [u32; 16] = [
        0x3b3b3b, 0xf0525a, 0x7bd88f, 0xf1c40f, 0x5aa9f6, 0xd17bf0, 0x4fd6d6, 0xd8d8d8,
        0x6c6c6c, 0xff7a82, 0x9df5ae, 0xffe066, 0x8cc6ff, 0xe6a4ff, 0x86f0f0, 0xffffff,
    ];
    const LIGHT: [u32; 16] = [
        0x1e1e1e, 0xc4221f, 0x2d8a3e, 0x9c6b00, 0x1f5fc4, 0x9a2bb8, 0x0f7f86, 0x6e6e6e,
        0x555555, 0xe0362f, 0x3aa14f, 0xb88400, 0x2f76e0, 0xb240d6, 0x1597a0, 0x2a2a2a,
    ];
    rgb(if light { LIGHT } else { DARK }[(index & 15) as usize])
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let focused = self.focus_handle.is_focused(window);
        let font_size = px(FONT_SIZE * self.zoom);
        let line_h = self.line_height();

        // Real monospace cell width at this size, for cursor placement,
        // hit-testing, and sizing the grid to the panel.
        let mono = font(crate::fonts::MONO);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&mono);
        if let Ok(advance) = text_system.advance(font_id, font_size, 'm') {
            self.cell_width = f32::from(advance.width).max(1.0);
        }

        let screen = self.emulator.screen();
        let title = self.emulator.title().unwrap_or_else(default_shell);
        let running = self.session.is_some();

        let header = div()
            .id("terminal-header")
            // Drag the header to dock the terminal at the bottom or right.
            .on_drag(crate::docking::PanelDrag { panel: crate::docking::DockPanel::Terminal, theme }, |drag, _, _, cx| {
                crate::docking::start_drag(drag, cx)
            })
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .h(px(HEADER_HEIGHT))
            .px_2()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .font_family(".SystemUIFont")
            .child(div().text_color(if focused { theme.accent } else { theme.foreground_muted }).child("TERMINAL"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.foreground_muted)
                    .child(if running { title } else { "stopped".to_string() }),
            )
            .when(screen.display_offset > 0, |h| {
                h.child(
                    header_button("term-bottom", "↓ Latest", theme)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.emulator.scroll_to_bottom();
                            cx.notify();
                        })),
                )
            })
            .child(header_button("term-clear", "Clear", theme).on_click(cx.listener(|this, _, _, cx| this.clear(cx))))
            .child(header_button("term-restart", "Restart", theme).on_click(cx.listener(|this, _, _, cx| this.restart(cx))));

        let cell_width = self.cell_width;
        let rows = screen.rows.iter().map(|runs| {
            div()
                .flex()
                .flex_row()
                .h(px(line_h))
                .whitespace_nowrap()
                .children(runs.iter().map(|run| {
                    let style = &run.style;
                    let (fg, bg) = (self.color(style.fg, style), self.color(style.bg, style));
                    // Exactly as wide as its cells, so what's drawn lines up with
                    // the mouse's cell math (fallback glyphs can be wider).
                    div()
                        .flex_shrink_0()
                        .w(px(run.text.chars().count() as f32 * cell_width))
                        .overflow_hidden()
                        .text_color(fg)
                        .when(style.bg != TermColor::DefaultBg, |d| d.bg(bg))
                        .when(style.selected, |d| d.bg(theme.accent.opacity(0.35)))
                        .when(style.bold, |d| d.font_weight(FontWeight::BOLD))
                        .when(style.italic, |d| d.italic())
                        .when(style.underline, |d| d.underline())
                        .child(run.text.clone())
                }))
        });

        // Cursor: a block when focused, an outline otherwise.
        let cursor = screen.cursor.filter(|_| running).map(|(row, col)| {
            div()
                .absolute()
                .left(px(PADDING + col as f32 * self.cell_width))
                .top(px(row as f32 * line_h))
                .w(px(self.cell_width))
                .h(px(line_h))
                .when(focused, |d| d.bg(theme.accent.opacity(0.55)))
                .when(!focused, |d| d.border_1().border_color(theme.accent))
        });

        // Scrollback position indicator.
        let history = screen.history_size;
        let thumb = (history > 0).then(|| {
            let total = (history + self.emulator.lines()) as f32;
            let visible = self.emulator.lines() as f32 / total;
            let top = (history - screen.display_offset) as f32 / total;
            div()
                .absolute()
                .right(px(1.))
                .top(gpui::relative(top))
                .h(gpui::relative(visible.max(0.04)))
                .w(px(8.))
                .rounded_sm()
                .bg(theme.accent.opacity(if screen.display_offset > 0 { 0.8 } else { 0.25 }))
        });

        let entity = cx.entity();
        let body = div()
            .id("terminal-body")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .pl(px(PADDING))
            .font_family(crate::fonts::MONO)
            .text_size(font_size)
            .line_height(px(line_h))
            .cursor_text()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.on_right_click(cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.on_mouse_move(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.selecting = false),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.on_scroll(event, cx)))
            .children(rows)
            .children(cursor)
            .children(thumb)
            // Measures the panel after layout to size the emulator + PTY.
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        entity.update(cx, |view, cx| view.fit_to(bounds, cx));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                // Pinned to the corner: without an inset an absolute element sits
                // at its "static position" (after the rows), which skewed every
                // mouse position and made selections land on the first line.
                .top_0()
                .left_0()
                .size_full(),
            );

        div()
            .id("terminal")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::handle_key_down))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .text_color(theme.foreground)
            .child(header)
            .child(body)
    }
}

fn header_button(id: &'static str, label: &'static str, theme: Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .rounded_sm()
        .text_color(theme.foreground_muted)
        .cursor_pointer()
        .hover(|s| s.bg(theme.accent.opacity(0.16)).text_color(theme.foreground))
        .child(label)
}

/// Removes CSI/OSC escape sequences (colors, cursor moves, titles) so log
/// lines can be matched as plain text.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters until a final byte in @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: until BEL or ESC \
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Recognizes embedded-server log lines:
/// - `Tomcat started on port 8080 (http) with context path '/'` (Boot 3/4)
/// - `Tomcat started on port(s): 8080 (http)` (Boot 2)
/// - `Netty started on port 8080`, Jetty, Undertow
/// - `Port 8080 was already in use.`
fn server_event(line: &str) -> Option<TerminalEvent> {
    let lower = line.to_ascii_lowercase();
    if let Some(at) = lower.find(" started on port") {
        let rest = &lower[at + " started on port".len()..];
        let rest = rest.trim_start_matches("(s)").trim_start_matches(['s', ':', ' ']);
        return leading_port(rest).map(TerminalEvent::ServerStarted);
    }
    if let Some(at) = lower.find("port ")
        && lower[at..].contains("already in use")
    {
        return leading_port(&lower[at + 5..]).map(TerminalEvent::PortInUse);
    }
    None
}

fn leading_port(text: &str) -> Option<u16> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|p| *p > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_server_start_lines() {
        let boot3 = "2026-10-03T17:27:07.403+05:30  INFO 3784 --- [boot] [  restartedMain] o.s.boot.tomcat.TomcatWebServer          : Tomcat started on port 8080 (http) with context path '/'";
        assert_eq!(server_event(boot3), Some(TerminalEvent::ServerStarted(8080)));
        assert_eq!(server_event("Tomcat started on port(s): 9090 (http) with context path ''"), Some(TerminalEvent::ServerStarted(9090)));
        assert_eq!(server_event("Netty started on port 8081"), Some(TerminalEvent::ServerStarted(8081)));
        assert_eq!(
            server_event("Web server failed to start. Port 8080 was already in use."),
            Some(TerminalEvent::PortInUse(8080))
        );
        assert_eq!(server_event("Started Application in 0.98 seconds"), None);
    }

    #[test]
    fn strips_colors_before_matching() {
        let colored = "\x1b[32mINFO\x1b[0m Tomcat started on port \x1b[1m8080\x1b[0m (http)\r";
        assert_eq!(strip_escapes(colored), "INFO Tomcat started on port 8080 (http)");
    }
}
