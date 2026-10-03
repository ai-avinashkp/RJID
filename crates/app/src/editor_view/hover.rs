//! Hover info: rest the mouse on a word (or on a diagnostic's underline,
//! or on a tinted line number) and a popup shows the diagnostics there plus
//! the language server's signature and Javadoc for the symbol.
//!
//! Lazy on purpose: nothing is requested until the pointer has rested for
//! `SHOW_DELAY`, the server is asked only for that one position, and the
//! popup shows "Loading…" until the answer arrives. Moving to another word
//! re-targets after a short delay, so sweeping across code doesn't spam
//! the server; moving into the popup keeps it open (to read or scroll).

use std::ops::Range;
use std::time::Duration;

use gpui::{
    Context, HighlightStyle, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, StyledText, div, prelude::*, px,
};
use rji_editor::{Language, LineState, highlight_line};
use rji_lsp_client::DiagnosticSeverity;
use rji_theme::Theme;

use super::{CodeEditorView, color_for_kind};

const SHOW_DELAY: Duration = Duration::from_millis(450);
/// Delay before switching/hiding while a popup is already up.
const SWITCH_DELAY: Duration = Duration::from_millis(250);
const MAX_WIDTH: f32 = 560.0;
const MAX_HEIGHT: f32 = 320.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HoverTarget {
    /// A word (or a diagnostic range) in the text, as document offsets.
    Text(Range<usize>),
    /// A line number in the gutter (shows that line's diagnostics).
    Gutter(usize),
}

pub(super) enum HoverInfo {
    /// No language-server info for this target (diagnostics only).
    None,
    Loading,
    Ready(String),
}

pub(super) struct HoverState {
    target: HoverTarget,
    /// Popup anchor in the editor's own coordinates.
    x: f32,
    line_top: f32,
    line_bottom: f32,
    diagnostics: Vec<(DiagnosticSeverity, String)>,
    info: HoverInfo,
    /// Quick fixes for the diagnostics shown (fetched with the hover).
    fixes: Vec<super::code_actions::Fix>,
}

/// A piece of hover Markdown: fenced code, or a paragraph of prose.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HoverBlock {
    Code(String),
    Text(String),
}

/// Splits Markdown into code blocks and plain-text paragraphs, removing
/// the inline markup jdtls uses (links, emphasis, backticks, headings).
pub(super) fn hover_blocks(markdown: &str) -> Vec<HoverBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    // Javadoc `<pre>` examples arrive as `>` quote lines: show them as code.
    let mut quote: Vec<String> = Vec::new();
    let flush_quote = |quote: &mut Vec<String>, blocks: &mut Vec<HoverBlock>| {
        if quote.iter().any(|l| !l.trim().is_empty()) {
            blocks.push(HoverBlock::Code(quote.join("\n").trim_matches('\n').to_string()));
        }
        quote.clear();
    };
    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<HoverBlock>| {
        if !paragraph.is_empty() {
            blocks.push(HoverBlock::Text(paragraph.join(" ")));
            paragraph.clear();
        }
    };
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => blocks.push(HoverBlock::Code(lines.join("\n"))),
                None => {
                    flush(&mut paragraph, &mut blocks);
                    code = Some(Vec::new());
                }
            }
            continue;
        }
        if let Some(lines) = code.as_mut() {
            lines.push(line);
            continue;
        }
        let trimmed = line.trim();
        if let Some(quoted) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            let content = quoted.trim_start_matches('>');
            quote.push(content.strip_prefix(' ').unwrap_or(content).trim_end().to_string());
            continue;
        }
        flush_quote(&mut quote, &mut blocks);
        if trimmed.is_empty() || trimmed == "---" || trimmed == "***" {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        let mut text = strip_inline_markdown(trimmed.trim_start_matches('#').trim_start());
        // List items and "@param"-style lines start their own line.
        if let Some(rest) = trimmed.strip_prefix("* ").or_else(|| trimmed.strip_prefix("- ")) {
            flush(&mut paragraph, &mut blocks);
            text = format!("• {}", strip_inline_markdown(rest));
        }
        paragraph.push(text);
    }
    if let Some(lines) = code {
        blocks.push(HoverBlock::Code(lines.join("\n")));
    }
    flush_quote(&mut quote, &mut blocks);
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// `[text](url)` -> `text`; drops `**`, `__`, `` ` `` and `\` escapes.
fn strip_inline_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find("](").and_then(|close| after[close..].find(')').map(|end| (close, close + end))) {
            Some((close, end)) => {
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("__", "").replace(['*', '`', '\\'], "")
}

impl CodeEditorView {
    pub(super) fn clear_hover(&mut self) {
        self.hover = None;
        self.hover_pending = None;
        self.hover_generation += 1;
    }

    /// Pointer moved over the text of `line` (no button held).
    pub(super) fn on_text_hover(&mut self, line: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.completion.is_some() || self.has_overlay() || line >= self.buffer.line_count() {
            return;
        }
        let Some((shaped, bounds)) = self.line_layouts.get(&line) else { return };
        let range = self.buffer.line_range(line);
        let line_text = &self.buffer.text()[range.clone()];
        let col = shaped.index_for_x(position.x - bounds.left()).filter(|c| *c < line_text.len());
        let (line_top, line_bottom) = (f32::from(bounds.top()), f32::from(bounds.bottom()));

        let Some(col) = col else {
            // Past the end of the line: nothing to show here.
            self.schedule_hover(None, 0.0, 0.0, 0.0, cx);
            return;
        };
        let col16 = utf16_len(&line_text[..col]) as u32;
        let has_diag = self.diagnostic_list.iter().any(|d| diagnostic_covers(d, line as u32, col16));
        let word = word_range(line_text, col);
        let target = if !word.is_empty() {
            Some(HoverTarget::Text(range.start + word.start..range.start + word.end))
        } else if has_diag {
            Some(HoverTarget::Text(range.start + col..range.start + col + 1))
        } else {
            None
        };
        let origin = self.list_origin();
        self.schedule_hover(target, f32::from(position.x) - origin.0, line_top - origin.1, line_bottom - origin.1, cx);
    }

    /// Pointer moved over the line number of `line`.
    pub(super) fn on_gutter_hover(&mut self, line: usize, cx: &mut Context<Self>) {
        if self.completion.is_some() || self.has_overlay() {
            return;
        }
        let target = self.diagnostics.contains_key(&(line as u32)).then_some(HoverTarget::Gutter(line));
        let line_top = line as f32 * self.line_height() + f32::from(self.scroll_handle.0.borrow().base_handle.offset().y);
        self.schedule_hover(target, self.gutter_width * 0.5, line_top, line_top + self.line_height(), cx);
    }

    /// Byte-column ranges on `line` to underline, colored by severity. An
    /// empty range is widened to the word at its start (or one character).
    pub(super) fn diagnostic_underlines(&self, line: usize, line_text: &str, theme: Theme) -> Vec<(usize, usize, gpui::Rgba)> {
        let line = line as u32;
        self.diagnostic_list
            .iter()
            .filter(|d| d.line <= line && line <= d.end_line)
            .filter_map(|d| {
                let start = if d.line == line { byte_col(line_text, d.start_char) } else { 0 };
                let mut end = if d.end_line == line { byte_col(line_text, d.end_char) } else { line_text.len() };
                if end <= start {
                    let word = word_range(line_text, start.min(line_text.len().saturating_sub(1)));
                    end = if word.end > start { word.end } else { (start + 1).min(line_text.len()) };
                }
                if start >= end {
                    return None;
                }
                let color = match d.severity {
                    DiagnosticSeverity::Error => theme.error,
                    DiagnosticSeverity::Warning => theme.warning,
                    DiagnosticSeverity::Info | DiagnosticSeverity::Hint => theme.accent,
                };
                Some((start, end, color))
            })
            .collect()
    }

    /// The uniform list's top-left in window coordinates.
    fn list_origin(&self) -> (f32, f32) {
        let bounds = self.scroll_handle.0.borrow().base_handle.bounds();
        (f32::from(bounds.left()), f32::from(bounds.top()))
    }

    fn schedule_hover(&mut self, target: Option<HoverTarget>, x: f32, line_top: f32, line_bottom: f32, cx: &mut Context<Self>) {
        if self.hover.as_ref().map(|h| &h.target) == target.as_ref() && target.is_some() {
            self.hover_pending = None;
            self.hover_generation += 1;
            return;
        }
        if self.hover_pending == target && target.is_some() {
            return;
        }
        if self.hover.is_none() && target.is_none() {
            self.hover_pending = None;
            self.hover_generation += 1;
            return;
        }
        self.hover_pending = target.clone();
        self.hover_generation += 1;
        let generation = self.hover_generation;
        let delay = if self.hover.is_some() { SWITCH_DELAY } else { SHOW_DELAY };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.hover_generation == generation {
                    this.show_hover(target, x, line_top, line_bottom, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_hover(&mut self, target: Option<HoverTarget>, x: f32, line_top: f32, line_bottom: f32, cx: &mut Context<Self>) {
        self.hover_pending = None;
        let Some(target) = target else {
            self.hover = None;
            cx.notify();
            return;
        };
        let (diagnostics, lsp_position): (Vec<(DiagnosticSeverity, String)>, Option<(u32, u32)>) = match &target {
            HoverTarget::Gutter(line) => {
                let diags = self
                    .diagnostic_list
                    .iter()
                    .filter(|d| d.line as usize == *line)
                    .map(|d| (d.severity, d.message.clone()))
                    .collect();
                (diags, None)
            }
            HoverTarget::Text(span) => {
                let (line, col) = self.buffer.line_col(span.start);
                let line_start = self.buffer.line_starts()[line];
                let col16 = utf16_len(&self.buffer.text()[line_start..line_start + col]) as u32;
                let diags = self
                    .diagnostic_list
                    .iter()
                    .filter(|d| diagnostic_covers(d, line as u32, col16))
                    .map(|d| (d.severity, d.message.clone()))
                    .collect();
                let is_word = span.end - span.start > 1
                    || self.buffer.text()[span.clone()].chars().all(|c| c.is_alphanumeric() || c == '_');
                (diags, is_word.then_some((line as u32, col16)))
            }
        };

        let request = match lsp_position {
            Some((line, col)) if self.is_java() => self
                .lsp
                .borrow_mut()
                .as_mut()
                .and_then(|client| client.request_hover(&self.path, line, col).ok()),
            _ => None,
        };
        if request.is_none() && diagnostics.is_empty() {
            self.hover = None;
            cx.notify();
            return;
        }
        // Quick fixes for the problems under the pointer, lazily too.
        let raw: Vec<serde_json::Value> = match &target {
            HoverTarget::Text(span) => {
                let (line, col) = self.buffer.line_col(span.start);
                let line_start = self.buffer.line_starts()[line];
                let col16 = utf16_len(&self.buffer.text()[line_start..line_start + col]) as u32;
                self.diagnostic_list.iter().filter(|d| diagnostic_covers(d, line as u32, col16)).map(|d| d.raw.clone()).collect()
            }
            HoverTarget::Gutter(line) => self.diagnostic_list.iter().filter(|d| d.line as usize == *line).map(|d| d.raw.clone()).collect(),
        };
        if !raw.is_empty() && self.is_java() && self.lsp.borrow().is_some() {
            let range = match &target {
                HoverTarget::Text(span) => span.clone(),
                HoverTarget::Gutter(line) => self.buffer.line_range(*line),
            };
            let wanted = target.clone();
            self.fetch_fixes(range, raw, cx, move |this, fixes, cx| {
                if let Some(hover) = this.hover.as_mut().filter(|h| h.target == wanted) {
                    hover.fixes = fixes;
                    cx.notify();
                }
            });
        }
        if let Some(rx) = request {
            let wanted = target.clone();
            cx.spawn(async move |this, cx| {
                let info = rx.recv().await.ok().flatten();
                this.update(cx, |this, cx| {
                    let Some(hover) = this.hover.as_mut().filter(|h| h.target == wanted) else { return };
                    match info {
                        Some(text) => hover.info = HoverInfo::Ready(text),
                        None if hover.diagnostics.is_empty() => this.hover = None,
                        None => hover.info = HoverInfo::None,
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        self.hover = Some(HoverState {
            info: if self.lsp.borrow().is_some() && lsp_position.is_some() && self.is_java() {
                HoverInfo::Loading
            } else {
                HoverInfo::None
            },
            target,
            x,
            line_top,
            line_bottom,
            diagnostics,
            fixes: Vec::new(),
        });
        cx.notify();
    }

    pub(super) fn render_hover(&self, view_w: f32, view_h: f32, theme: Theme, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let hover = self.hover.as_ref()?;
        let z = self.zoom;
        let font = 12.5 * z;
        let max_w = (MAX_WIDTH * z).min(view_w - 16.0).max(180.0);
        let max_h = (MAX_HEIGHT * z).min(view_h - 8.0).max(80.0);

        let mut body = div().flex().flex_col().gap(px(6. * z));
        for (severity, message) in &hover.diagnostics {
            let (icon, color) = match severity {
                DiagnosticSeverity::Error => ("✕", theme.error),
                DiagnosticSeverity::Warning => ("⚠", theme.warning),
                DiagnosticSeverity::Info | DiagnosticSeverity::Hint => ("ℹ", theme.accent),
            };
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6. * z))
                    .child(div().flex_shrink_0().text_color(color).child(icon))
                    .child(div().flex_1().min_w_0().text_color(theme.foreground).child(message.clone())),
            );
        }
        const SHOWN_FIXES: usize = 6;
        for (i, fix) in hover.fixes.iter().take(SHOWN_FIXES).enumerate() {
            let fix_clone = fix.clone();
            body = body.child(
                div()
                    .id(("hover-fix", i))
                    .px(px(4. * z))
                    .rounded_sm()
                    .cursor_pointer()
                    .text_color(theme.accent)
                    .hover(|s| s.bg(theme.accent.opacity(0.15)))
                    .child(format!("💡 {}", fix.title))
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_fix(fix_clone.clone(), cx))),
            );
        }
        if hover.fixes.len() > SHOWN_FIXES {
            body = body.child(
                div()
                    .text_size(px(11. * z))
                    .text_color(theme.foreground_muted)
                    .child(format!("{} more — Ctrl+. for all quick fixes", hover.fixes.len() - SHOWN_FIXES)),
            );
        }
        let has_diags = !hover.diagnostics.is_empty();
        match &hover.info {
            HoverInfo::None => {}
            HoverInfo::Loading => {
                body = body.child(div().text_color(theme.foreground_muted).child("Loading…"));
            }
            HoverInfo::Ready(markdown) => {
                if has_diags {
                    body = body.child(div().h(px(1.)).bg(theme.border));
                }
                for block in hover_blocks(markdown) {
                    body = body.child(match block {
                        HoverBlock::Code(code) => code_block(&code, theme, z).into_any_element(),
                        HoverBlock::Text(text) => div().text_color(theme.foreground).child(text).into_any_element(),
                    });
                }
            }
        }

        // Rough height to decide above/below; the real one is clamped by max_h.
        let lines: usize = hover.diagnostics.len().max(1) + matches!(hover.info, HoverInfo::Ready(_)) as usize * 6;
        let est_h = (lines as f32 * font * 1.5 + 20.0 * z).min(max_h);
        // Below the line if it fits (or has more room than above); the
        // popup never extends past the editor — it scrolls instead.
        let space_below = view_h - hover.line_bottom - 6.0;
        let space_above = hover.line_top - 6.0;
        let below = space_below >= est_h || space_below >= space_above;
        let max_h = max_h.min(if below { space_below } else { space_above }).max(60.0);
        let top = hover.line_bottom + 2.0;
        let left = (hover.x - 12.0 * z).min(view_w - max_w - 8.0).max(4.0);

        Some(
            div()
                .id("hover-popup")
                .absolute()
                .left(px(left))
                .when(below, |d| d.top(px(top)))
                .when(!below, |d| d.bottom(px(view_h - hover.line_top + 2.0)))
                .max_w(px(max_w))
                .max_h(px(max_h))
                .overflow_y_scroll()
                .p(px(8. * z))
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_lg()
                .text_size(px(font))
                .font_family(".SystemUIFont")
                .occlude()
                // Inside the popup: keep it open, and let it scroll on its own.
                .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                    this.hover_pending = None;
                    this.hover_generation += 1;
                    cx.stop_propagation();
                }))
                .on_scroll_wheel(cx.listener(|_, _: &ScrollWheelEvent, _, cx| cx.stop_propagation()))
                .child(body),
        )
    }
}

/// A Java code block, syntax-highlighted line by line.
fn code_block(code: &str, theme: Theme, z: f32) -> impl IntoElement {
    let mut state = LineState::Normal;
    let lines = code.lines().map(|line| {
        let (tokens, next) = highlight_line(Language::Java, line, state);
        state = next;
        let mut start = 0;
        let highlights: Vec<(Range<usize>, HighlightStyle)> = tokens
            .iter()
            .map(|token| {
                let range = start..start + token.text.len();
                start = range.end;
                (range, HighlightStyle { color: Some(color_for_kind(token.kind, theme).into()), ..Default::default() })
            })
            .collect();
        div().child(StyledText::new(line.to_string()).with_highlights(highlights))
    });
    div()
        .flex()
        .flex_col()
        .px(px(6. * z))
        .py(px(4. * z))
        .rounded_sm()
        .bg(theme.background)
        .font_family(crate::fonts::MONO)
        .children(lines)
}

pub(super) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset in `line` of UTF-16 column `col16` (clamped to the end).
pub(super) fn byte_col(line: &str, col16: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in line.char_indices() {
        if units >= col16 {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    line.len()
}

/// The identifier around byte `col` of `line` (empty if `col` isn't on one).
pub(super) fn word_range(line: &str, col: usize) -> Range<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    if !line[col..].chars().next().is_some_and(is_word) {
        return col..col;
    }
    let start = line[..col].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(col, |(i, _)| i);
    let end = line[col..].char_indices().find(|(_, c)| !is_word(*c)).map_or(line.len(), |(i, _)| col + i);
    start..end
}

pub(super) fn diagnostic_covers(d: &rji_lsp_client::Diagnostic, line: u32, col16: u32) -> bool {
    let start = (d.line, d.start_char);
    let end = if (d.end_line, d.end_char) > start { (d.end_line, d.end_char) } else { (d.line, d.start_char + 1) };
    (line, col16) >= start && (line, col16) < end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_jdtls_hover_markdown() {
        let md = "```java\nvoid java.io.PrintStream.println(String x)\n```\n\nPrints a String and then terminates the line. See [String](jdt://x).\n\n* **Parameters:**\n* x The `String` to be printed.";
        assert_eq!(
            hover_blocks(md),
            vec![
                HoverBlock::Code("void java.io.PrintStream.println(String x)".into()),
                HoverBlock::Text("Prints a String and then terminates the line. See String.".into()),
                HoverBlock::Text("• Parameters:".into()),
                HoverBlock::Text("• x The String to be printed.".into()),
            ]
        );
    }

    #[test]
    fn javadoc_pre_quotes_become_code() {
        let md = "A typical way to write a line is:\n\n>\n>     System.out.println(data)\n>\n\nSee *PrintStream*.";
        assert_eq!(
            hover_blocks(md),
            vec![
                HoverBlock::Text("A typical way to write a line is:".into()),
                HoverBlock::Code("    System.out.println(data)".into()),
                HoverBlock::Text("See PrintStream.".into()),
            ]
        );
    }

    #[test]
    fn finds_word_under_pointer() {
        let line = "    int count = arr[2];";
        assert_eq!(&line[word_range(line, 9)], "count");
        assert_eq!(&line[word_range(line, 8)], "count");
        assert_eq!(word_range(line, 13), 13..13); // the space before '='
    }

    #[test]
    fn diagnostic_ranges() {
        let d = rji_lsp_client::Diagnostic {
            line: 3,
            start_char: 4,
            end_line: 3,
            end_char: 9,
            severity: DiagnosticSeverity::Error,
            message: "x".into(),
            raw: serde_json::Value::Null,
        };
        assert!(diagnostic_covers(&d, 3, 4) && diagnostic_covers(&d, 3, 8));
        assert!(!diagnostic_covers(&d, 3, 9) && !diagnostic_covers(&d, 2, 5));
    }
}
