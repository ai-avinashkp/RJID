//! A real VT terminal model: `alacritty_terminal`'s grid + parser behind a
//! small, GPUI-free API. Feed it PTY output; read back a styled snapshot of
//! the visible screen. Full-screen programs (vim, htop, less), cursor
//! positioning, colors, and scrollback all work because the grid is a real
//! terminal emulation rather than an appended text log.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

/// Lines of scrollback kept above the screen.
pub const SCROLLBACK: usize = 10_000;

/// A color as the program asked for it; the UI maps the 16 ANSI colors and
/// the defaults through its theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    DefaultFg,
    DefaultBg,
    /// 0–15 ANSI (themed), 16–255 the xterm 256-color cube/greys.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellStyle {
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub dim: bool,
    pub selected: bool,
}

/// A run of same-styled characters on one screen row.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: CellStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub rows: Vec<Vec<Run>>,
    /// Cursor (row, column) on screen, if visible.
    pub cursor: Option<(usize, usize)>,
    /// How many lines the view is scrolled up into history (0 = live).
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Copy)]
struct Size {
    columns: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Collects what the terminal wants written back to the program (replies
/// to queries such as cursor-position reports, which ConPTY sends at
/// startup) and title changes.
#[derive(Clone, Default)]
struct Listener {
    replies: Rc<RefCell<Vec<u8>>>,
    title: Rc<RefCell<Option<String>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.replies.borrow_mut().extend_from_slice(text.as_bytes()),
            Event::Title(title) => *self.title.borrow_mut() = Some(title),
            Event::ResetTitle => *self.title.borrow_mut() = None,
            _ => {}
        }
    }
}

pub struct Emulator {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
}

impl Emulator {
    pub fn new(columns: usize, lines: usize) -> Self {
        let size = Size {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        let listener = Listener::default();
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Config::default()
        };
        Emulator {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            listener,
        }
    }

    /// Processes program output. Returns bytes that must be written back
    /// to the program (terminal query replies), usually empty.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, bytes);
        std::mem::take(&mut *self.listener.replies.borrow_mut())
    }

    pub fn columns(&self) -> usize {
        self.term.columns()
    }

    pub fn lines(&self) -> usize {
        self.term.screen_lines()
    }

    pub fn resize(&mut self, columns: usize, lines: usize) {
        let size = Size {
            columns: columns.max(2),
            lines: lines.max(1),
        };
        if size.columns != self.columns() || size.lines != self.lines() {
            self.term.resize(size);
        }
    }

    /// Scrolls the view through history: positive = up (older output).
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn title(&self) -> Option<String> {
        self.listener.title.borrow().clone()
    }

    /// Arrow/Home/End keys must use the `ESC O` form in this mode (set by
    /// full-screen programs and readline).
    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Program is using the alternate screen (vim, less, htop, …).
    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    // ---- selection (screen coordinates: row 0 = top visible line) --------

    fn grid_point(&self, row: usize, column: usize) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        Point::new(
            Line(row.min(self.lines() - 1) as i32 - offset),
            Column(column.min(self.columns() - 1)),
        )
    }

    /// `right_half`: the press was on the right half of the cell, so the
    /// selection boundary is that cell's right edge (as in other terminals;
    /// it makes dragging backwards and partial-cell drags select exactly
    /// what's under the pointer).
    pub fn start_selection(&mut self, row: usize, column: usize, right_half: bool, kind: SelectKind) {
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        let point = self.grid_point(row, column);
        let side = if right_half { Side::Right } else { Side::Left };
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn extend_selection(&mut self, row: usize, column: usize, right_half: bool) {
        let point = self.grid_point(row, column);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, if right_half { Side::Right } else { Side::Left });
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// The visible screen as styled runs per row.
    pub fn screen(&self) -> Screen {
        let content = self.term.renderable_content();
        let offset = content.display_offset;
        let lines = self.lines();
        let mut rows: Vec<Vec<Run>> = vec![Vec::new(); lines];
        let selection = content.selection;

        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset as i32;
            if row < 0 || row as usize >= lines {
                continue;
            }
            let selected = selection.is_some_and(|s| s.contains(indexed.point));
            let (mut fg, mut bg) = (map_color(cell.fg, true), map_color(cell.bg, false));
            if cell.flags.contains(Flags::INVERSE) {
                // Defaults trade places along with everything else.
                std::mem::swap(&mut fg, &mut bg);
            }
            let style = CellStyle {
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                dim: cell.flags.contains(Flags::DIM),
                selected,
            };
            let ch = if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' || cell.c == '\t' {
                ' '
            } else {
                cell.c
            };
            let runs = &mut rows[row as usize];
            match runs.last_mut() {
                Some(run) if run.style == style => run.text.push(ch),
                _ => runs.push(Run {
                    text: ch.to_string(),
                    style,
                }),
            }
        }

        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + offset as i32;
        let cursor = (content.mode.contains(TermMode::SHOW_CURSOR)
            && cursor_row >= 0
            && (cursor_row as usize) < lines)
            .then(|| (cursor_row as usize, cursor_point.column.0));

        Screen {
            rows,
            cursor,
            display_offset: offset,
            history_size: self.term.grid().history_size(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

fn map_color(color: Color, foreground: bool) -> TermColor {
    match color {
        Color::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => TermColor::Indexed(i),
        Color::Named(named) => {
            let n = named as usize;
            match named {
                _ if n < 16 => TermColor::Indexed(n as u8),
                NamedColor::Background => TermColor::DefaultBg,
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                    TermColor::DefaultFg
                }
                NamedColor::Cursor => TermColor::DefaultFg,
                // Dim variants: the matching base color (the view dims it).
                NamedColor::DimBlack => TermColor::Indexed(0),
                NamedColor::DimRed => TermColor::Indexed(1),
                NamedColor::DimGreen => TermColor::Indexed(2),
                NamedColor::DimYellow => TermColor::Indexed(3),
                NamedColor::DimBlue => TermColor::Indexed(4),
                NamedColor::DimMagenta => TermColor::Indexed(5),
                NamedColor::DimCyan => TermColor::Indexed(6),
                NamedColor::DimWhite => TermColor::Indexed(7),
                _ if foreground => TermColor::DefaultFg,
                _ => TermColor::DefaultBg,
            }
        }
    }
}

/// RGB for xterm palette indices 16–255 (6×6×6 cube + 24 greys).
pub fn xterm_256(index: u8) -> (u8, u8, u8) {
    match index {
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
        _ => (0, 0, 0),
    }
}

/// Bytes a key should send, given the terminal's current modes. `key` uses
/// GPUI key names. `None` = not a special key (send the typed text).
pub fn key_bytes(key: &str, ctrl: bool, alt: bool, shift: bool, app_cursor: bool) -> Option<Vec<u8>> {
    let csi_or_ss3 = |c: char| {
        if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let modifier = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let arrow = |c: char| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{c}")
        } else {
            csi_or_ss3(c)
        }
    };
    let tilde = |n: u8| {
        if modifier > 1 {
            format!("\x1b[{n};{modifier}~")
        } else {
            format!("\x1b[{n}~")
        }
    };
    let s = match key {
        "enter" => if alt { "\x1b\r".to_string() } else { "\r".to_string() },
        "backspace" => if ctrl { "\x08".to_string() } else if alt { "\x1b\x7f".to_string() } else { "\x7f".to_string() },
        "tab" => if shift { "\x1b[Z".to_string() } else { "\t".to_string() },
        "escape" => "\x1b".to_string(),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => arrow('H'),
        "end" => arrow('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => "\x1bOP".to_string(),
        "f2" => "\x1bOQ".to_string(),
        "f3" => "\x1bOR".to_string(),
        "f4" => "\x1bOS".to_string(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if ctrl => "\0".to_string(),
        k if ctrl && k.len() == 1 => {
            let b = k.as_bytes()[0];
            let code = match b {
                b'a'..=b'z' => b - b'a' + 1,
                b'[' => 0x1b,
                b'\\' => 0x1c,
                b']' => 0x1d,
                b'^' | b'6' => 0x1e,
                b'_' | b'-' => 0x1f,
                _ => return None,
            };
            let mut bytes = Vec::new();
            if alt {
                bytes.push(0x1b);
            }
            bytes.push(code);
            return Some(bytes);
        }
        _ => return None,
    };
    Some(s.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(screen: &Screen, row: usize) -> String {
        screen.rows[row].iter().map(|r| r.text.as_str()).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn plain_text_and_newlines() {
        let mut emu = Emulator::new(20, 4);
        emu.feed(b"hello\r\nworld");
        let screen = emu.screen();
        assert_eq!(row_text(&screen, 0), "hello");
        assert_eq!(row_text(&screen, 1), "world");
        assert_eq!(screen.cursor, Some((1, 5)));
    }

    #[test]
    fn cursor_positioning_overwrites_like_a_real_terminal() {
        let mut emu = Emulator::new(20, 4);
        // Clear screen, write, then jump to row 1 col 1 and overwrite.
        emu.feed(b"\x1b[2J\x1b[1;1Habcdef\x1b[1;3HXY");
        assert_eq!(row_text(&emu.screen(), 0), "abXYef");
    }

    #[test]
    fn colors_and_attributes() {
        let mut emu = Emulator::new(20, 2);
        emu.feed(b"\x1b[1;31mred\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m \x1b[7minv");
        let runs = &emu.screen().rows[0];
        let red = runs.iter().find(|r| r.text == "red").unwrap();
        assert_eq!(red.style.fg, TermColor::Indexed(1));
        assert!(red.style.bold);
        assert!(runs.iter().any(|r| r.text == "rgb" && r.style.fg == TermColor::Rgb(1, 2, 3)));
        let inv = runs.iter().find(|r| r.text.starts_with("inv")).unwrap();
        assert_eq!((inv.style.fg, inv.style.bg), (TermColor::DefaultBg, TermColor::DefaultFg));
    }

    #[test]
    fn answers_cursor_position_query() {
        let mut emu = Emulator::new(20, 4);
        let reply = emu.feed(b"ab\x1b[6n");
        assert_eq!(reply, b"\x1b[1;3R");
    }

    #[test]
    fn split_escape_sequence_across_chunks() {
        let mut emu = Emulator::new(20, 2);
        emu.feed(b"ok\x1b[3");
        emu.feed(b"2mgreen");
        assert_eq!(row_text(&emu.screen(), 0), "okgreen");
    }

    #[test]
    fn scrollback_and_view_scrolling() {
        let mut emu = Emulator::new(10, 3);
        for i in 0..10 {
            emu.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(emu.screen().history_size >= 7);
        emu.scroll(2);
        let screen = emu.screen();
        assert_eq!(screen.display_offset, 2);
        assert_eq!(row_text(&screen, 0), "line6");
        emu.scroll_to_bottom();
        assert_eq!(emu.screen().display_offset, 0);
    }

    #[test]
    fn selection_copies_text() {
        let mut emu = Emulator::new(20, 2);
        emu.feed(b"hello world");
        emu.start_selection(0, 0, false, SelectKind::Simple);
        emu.extend_selection(0, 4, true);
        assert_eq!(emu.selected_text().as_deref(), Some("hello"));
        // Dragging backwards from the right half of the 'o' of "world".
        emu.start_selection(0, 10, true, SelectKind::Simple);
        emu.extend_selection(0, 6, false);
        assert_eq!(emu.selected_text().as_deref(), Some("world"));
        // A press and release inside one cell selects nothing.
        emu.start_selection(0, 2, false, SelectKind::Simple);
        emu.extend_selection(0, 2, false);
        assert_eq!(emu.selected_text(), None);
        emu.start_selection(0, 7, false, SelectKind::Word);
        assert_eq!(emu.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn keys_respect_application_cursor_mode() {
        assert_eq!(key_bytes("up", false, false, false, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("up", false, false, false, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes("right", true, false, false, false).unwrap(), b"\x1b[1;5C");
        assert_eq!(key_bytes("c", true, false, false, false).unwrap(), vec![3]);
        assert_eq!(key_bytes("pagedown", false, false, false, false).unwrap(), b"\x1b[6~");
        assert_eq!(key_bytes("x", false, false, false, false), None);
    }

    #[test]
    fn resize_reflows_dimensions() {
        let mut emu = Emulator::new(10, 3);
        emu.resize(40, 12);
        assert_eq!((emu.columns(), emu.lines()), (40, 12));
        assert_eq!(emu.screen().rows.len(), 12);
    }

    #[test]
    fn palette_cube() {
        assert_eq!(xterm_256(16), (0, 0, 0));
        assert_eq!(xterm_256(231), (255, 255, 255));
        assert_eq!(xterm_256(232), (8, 8, 8));
    }
}
