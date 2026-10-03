//! The editor's text model: content, caret, selection, and undo/redo —
//! kept free of any GPUI types so every editing rule can be unit-tested
//! directly, without driving a window.
//!
//! Offsets are UTF-8 byte offsets into `text` and are always kept on char
//! boundaries. The selection is `anchor..cursor` (in either order); there
//! is no selection when `anchor` is `None` or equal to `cursor`.

use std::ops::Range;

/// Spaces per indent level. The editor always inserts spaces, never tabs.
pub const INDENT: usize = 4;
const MAX_UNDO: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Debug, Clone)]
struct Edit {
    /// Where the change happened, in the *pre-edit* text.
    start: usize,
    removed: String,
    inserted: String,
    cursor_before: usize,
    anchor_before: Option<usize>,
    cursor_after: usize,
}

#[derive(Debug, Default)]
pub struct TextBuffer {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    /// Char column the caret "wants" to be at while moving vertically, so
    /// passing through a short line doesn't lose the original column.
    preferred_col: Option<usize>,
    line_starts: Vec<usize>,
    undo_stack: Vec<Edit>,
    redo_stack: Vec<Edit>,
    last_kind: Option<EditKind>,
}

impl TextBuffer {
    pub fn new(text: String) -> Self {
        let mut buffer = TextBuffer {
            text,
            ..Default::default()
        };
        buffer.recompute_lines();
        buffer
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte range of `line`'s content, excluding its trailing newline.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .map(|s| s - 1)
            .unwrap_or(self.text.len());
        start..end
    }

    pub fn line_text(&self, line: usize) -> &str {
        &self.text[self.line_range(line)]
    }

    /// `(line, byte column within that line)` for an offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        (line, offset - self.line_starts[line])
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        self.line_col(self.cursor)
    }

    /// The selected byte range, if any text is selected.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    // ---- cursor movement -------------------------------------------------

    /// Moves the caret to `offset`. With `extend`, the selection grows from
    /// its existing anchor (or from the old caret position); without it,
    /// any selection is cleared.
    pub fn set_cursor(&mut self, offset: usize, extend: bool) {
        let offset = self.clamp_to_boundary(offset);
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
        self.preferred_col = None;
        self.last_kind = None;
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && let Some(sel) = self.selection() {
            return self.set_cursor(sel.start, false);
        }
        let target = self.prev_boundary(self.cursor);
        self.set_cursor(target, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && let Some(sel) = self.selection() {
            return self.set_cursor(sel.end, false);
        }
        let target = self.next_boundary(self.cursor);
        self.set_cursor(target, extend);
    }

    pub fn move_vertical(&mut self, delta: i64, extend: bool) {
        let (line, _) = self.cursor_line_col();
        let want_col = self.preferred_col.unwrap_or_else(|| self.char_col(self.cursor));
        let target_line = (line as i64 + delta).clamp(0, self.line_count() as i64 - 1) as usize;
        let target = if target_line == line && delta < 0 {
            self.line_starts[0]
        } else if target_line == line && delta > 0 {
            self.text.len()
        } else {
            self.offset_for_char_col(target_line, want_col)
        };
        self.set_cursor(target, extend);
        self.preferred_col = Some(want_col);
    }

    /// Home: jumps to the first non-whitespace character, or to column 0 if
    /// already there ("smart home").
    pub fn move_line_start(&mut self, extend: bool) {
        let (line, _) = self.cursor_line_col();
        let range = self.line_range(line);
        let indent_end = range.start + leading_whitespace(&self.text[range.clone()]);
        let target = if self.cursor == indent_end {
            range.start
        } else {
            indent_end
        };
        self.set_cursor(target, extend);
    }

    pub fn move_line_end(&mut self, extend: bool) {
        let (line, _) = self.cursor_line_col();
        let end = self.line_range(line).end;
        self.set_cursor(end, extend);
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.set_cursor(0, extend);
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.set_cursor(self.text.len(), extend);
    }

    pub fn move_word_left(&mut self, extend: bool) {
        let target = self.word_boundary_before(self.cursor);
        self.set_cursor(target, extend);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        let target = self.word_boundary_after(self.cursor);
        self.set_cursor(target, extend);
    }

    /// Selects `range` with the caret at its end (e.g. a search match).
    pub fn select_range(&mut self, range: Range<usize>) {
        self.anchor = Some(self.clamp_to_boundary(range.start));
        self.cursor = self.clamp_to_boundary(range.end);
        self.preferred_col = None;
        self.last_kind = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.last_kind = None;
    }

    /// Selects the word (identifier run) containing `offset` — double-click.
    pub fn select_word_at(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        let (line, _) = self.line_col(offset);
        let range = self.line_range(line);
        let bytes = self.text.as_bytes();
        let mut start = offset;
        while start > range.start && is_word_byte(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = offset;
        while end < range.end && is_word_byte(bytes[end]) {
            end += 1;
        }
        self.anchor = Some(start);
        self.cursor = end;
        self.last_kind = None;
    }

    /// Selects a whole line including its newline — triple-click.
    pub fn select_line(&mut self, line: usize) {
        let line = line.min(self.line_count() - 1);
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        self.anchor = Some(start);
        self.cursor = end;
        self.last_kind = None;
    }

    // ---- editing ---------------------------------------------------------

    /// Types `text` at the caret, replacing any selection.
    pub fn insert(&mut self, text: &str) {
        let kind = if text.chars().count() == 1 && !text.contains('\n') {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.replace(range, text, kind);
    }

    /// Enter: newline plus the current line's indentation, one extra level
    /// after an opening `{`, and the closing `}` pushed onto its own line
    /// when the caret sits between `{` and `}`.
    pub fn insert_newline(&mut self) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        let (line, _) = self.line_col(range.start);
        let line_start = self.line_starts[line];
        let before = &self.text[line_start..range.start];
        let indent = " ".repeat(leading_whitespace(before));
        let opens_block = before.trim_end().ends_with('{');
        let closes_next = self.text[range.end..]
            .trim_start_matches(' ')
            .starts_with('}');

        let mut inserted = format!("\n{indent}");
        let mut caret_offset = inserted.len();
        if opens_block {
            inserted.push_str(&" ".repeat(INDENT));
            caret_offset = inserted.len();
            if closes_next {
                inserted.push('\n');
                inserted.push_str(&indent);
            }
        }
        self.replace(range.clone(), &inserted, EditKind::Other);
        self.cursor = range.start + caret_offset;
        if let Some(edit) = self.undo_stack.last_mut() {
            edit.cursor_after = self.cursor;
        }
    }

    /// Tab: with a multi-line selection, indents every selected line;
    /// otherwise inserts spaces up to the next indent stop.
    pub fn indent(&mut self) {
        if let Some(sel) = self.selection()
            && self.text[sel.clone()].contains('\n')
        {
            return self.indent_lines(true);
        }
        let col = self.char_col(self.selection().map_or(self.cursor, |s| s.start));
        let spaces = INDENT - (col % INDENT);
        self.insert(&" ".repeat(spaces));
    }

    /// Shift+Tab: removes up to one indent level from each selected line
    /// (or the caret's line).
    pub fn outdent(&mut self) {
        self.indent_lines(false);
    }

    pub fn backspace(&mut self) {
        if let Some(sel) = self.selection() {
            return self.replace(sel, "", EditKind::Other);
        }
        if self.cursor == 0 {
            return;
        }
        // Inside leading indentation, delete back to the previous indent
        // stop instead of one space at a time.
        let (line, col) = self.cursor_line_col();
        let line_start = self.line_starts[line];
        let before = &self.text[line_start..self.cursor];
        let start = if col > 0 && before.bytes().all(|b| b == b' ') {
            let remove = match col % INDENT {
                0 => INDENT,
                n => n,
            };
            self.cursor - remove.min(col)
        } else {
            self.prev_boundary(self.cursor)
        };
        self.replace(start..self.cursor, "", EditKind::Deleting);
    }

    pub fn delete_forward(&mut self) {
        if let Some(sel) = self.selection() {
            return self.replace(sel, "", EditKind::Other);
        }
        if self.cursor >= self.text.len() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.replace(self.cursor..end, "", EditKind::Deleting);
    }

    pub fn delete_word_back(&mut self) {
        if let Some(sel) = self.selection() {
            return self.replace(sel, "", EditKind::Other);
        }
        let start = self.word_boundary_before(self.cursor);
        self.replace(start..self.cursor, "", EditKind::Other);
    }

    /// Copy: the selection, or (with nothing selected) the whole current
    /// line including its newline — the common editor convention.
    pub fn copy_text(&self) -> String {
        if let Some(text) = self.selected_text() {
            return text.to_string();
        }
        let (line, _) = self.cursor_line_col();
        format!("{}\n", self.line_text(line))
    }

    /// Cut: like `copy_text`, but also removes what was copied.
    pub fn cut(&mut self) -> String {
        let copied = self.copy_text();
        if let Some(sel) = self.selection() {
            self.replace(sel, "", EditKind::Other);
        } else {
            let (line, _) = self.cursor_line_col();
            let start = self.line_starts[line];
            let end = self
                .line_starts
                .get(line + 1)
                .copied()
                .unwrap_or(self.text.len());
            // Last line has no trailing newline to take — take the one
            // before it instead so no empty line is left behind.
            let start = if end == self.text.len() && start > 0 && line + 1 >= self.line_count() {
                start - 1
            } else {
                start
            };
            self.replace(start..end, "", EditKind::Other);
        }
        copied
    }

    pub fn paste(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.replace(range, &normalized, EditKind::Other);
    }

    /// Ctrl+D: duplicates the caret's line (or the selected lines) below.
    pub fn duplicate_lines(&mut self) {
        let (first, last) = self.selected_line_span();
        let start = self.line_starts[first];
        let end = self.line_range(last).end;
        let block = self.text[start..end].to_string();
        let cursor = self.cursor;
        let anchor = self.anchor;
        self.replace(end..end, &format!("\n{block}"), EditKind::Other);
        let shift = block.len() + 1;
        self.cursor = cursor + shift;
        self.anchor = anchor.map(|a| a + shift);
        if let Some(edit) = self.undo_stack.last_mut() {
            edit.cursor_after = self.cursor;
        }
    }

    /// Ctrl+/: toggles a `// ` line comment on the caret's line or every
    /// selected line — comments all if any line is uncommented.
    pub fn toggle_line_comment(&mut self) {
        let (first, last) = self.selected_line_span();
        let lines: Vec<(usize, String)> = (first..=last)
            .map(|l| (l, self.line_text(l).to_string()))
            .collect();
        let all_commented = lines
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
            .all(|(_, t)| t.trim_start().starts_with("//"));
        let min_indent = lines
            .iter()
            .filter(|(_, t)| !t.trim().is_empty())
            .map(|(_, t)| leading_whitespace(t))
            .min()
            .unwrap_or(0);

        let new_lines: Vec<String> = lines
            .iter()
            .map(|(_, t)| {
                if t.trim().is_empty() {
                    return t.clone();
                }
                if all_commented {
                    let ws = leading_whitespace(t);
                    let rest = &t[ws..];
                    let rest = rest.strip_prefix("// ").or_else(|| rest.strip_prefix("//")).unwrap_or(rest);
                    format!("{}{}", &t[..ws], rest)
                } else {
                    format!("{}// {}", &t[..min_indent], &t[min_indent..])
                }
            })
            .collect();

        let start = self.line_starts[first];
        let end = self.line_range(last).end;
        let (cursor_line, cursor_col) = self.cursor_line_col();
        self.replace(start..end, &new_lines.join("\n"), EditKind::Other);
        let range = self.line_range(cursor_line.min(self.line_count() - 1));
        self.cursor = self.clamp_to_boundary((range.start + cursor_col).min(range.end));
        self.anchor = None;
    }

    // ---- undo / redo -----------------------------------------------------

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else {
            return false;
        };
        let end = edit.start + edit.inserted.len();
        self.text.replace_range(edit.start..end, &edit.removed);
        self.cursor = edit.cursor_before;
        self.anchor = edit.anchor_before;
        self.redo_stack.push(edit);
        self.after_edit();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else {
            return false;
        };
        let end = edit.start + edit.removed.len();
        self.text.replace_range(edit.start..end, &edit.inserted);
        self.cursor = edit.cursor_after;
        self.anchor = None;
        self.undo_stack.push(edit);
        self.after_edit();
        true
    }

    /// Replaces the whole document (e.g. an external reload) as one
    /// undoable step.
    pub fn replace_all(&mut self, text: &str) {
        self.replace(0..self.text.len(), text, EditKind::Other);
    }

    /// Replaces `range` with `text` and puts the caret after it — used by
    /// completion to swap the typed prefix for the chosen item.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let range = self.clamp_to_boundary(range.start)..self.clamp_to_boundary(range.end);
        self.replace(range, text, EditKind::Other);
    }

    /// Applies several non-overlapping edits (e.g. a language server's
    /// import + generated code) as ONE undo step. The caret keeps its place
    /// relative to the surrounding text: an import added above it shifts it
    /// down instead of jumping it. A caret inside a replaced range moves to
    /// the end of that replacement. Ranges are clamped to char boundaries;
    /// overlapping edits are skipped.
    pub fn apply_edits(&mut self, mut edits: Vec<(Range<usize>, String)>) {
        edits.retain(|(range, text)| range.start <= range.end && range.end <= self.text.len() && !(range.is_empty() && text.is_empty()));
        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|(range, _)| range.start);
        // Drop any edit that overlaps the previous one.
        let mut kept: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            let range = self.clamp_to_boundary(range.start)..self.clamp_to_boundary(range.end);
            if kept.last().is_some_and(|(prev, _)| range.start < prev.end) {
                continue;
            }
            kept.push((range, text));
        }
        let start = kept.first().map_or(0, |(r, _)| r.start);
        let end = kept.last().map_or(0, |(r, _)| r.end);

        let mut new_region = String::new();
        let mut at = start;
        let old_cursor = self.cursor;
        // Net length change of the edits entirely before the caret.
        let mut shift: isize = 0;
        let mut inside: Option<usize> = None;
        for (range, text) in &kept {
            new_region.push_str(&self.text[at..range.start]);
            new_region.push_str(text);
            at = range.end;
            let delta = text.len() as isize - (range.end - range.start) as isize;
            let before_caret = if range.is_empty() { range.start < old_cursor } else { range.end <= old_cursor };
            if before_caret {
                shift += delta;
            } else if range.start < old_cursor && old_cursor < range.end {
                inside = Some((range.start as isize + shift) as usize + text.len());
            }
        }
        new_region.push_str(&self.text[at..end]);
        let cursor = inside.unwrap_or_else(|| (old_cursor as isize + shift).max(0) as usize);

        let cursor_before = self.cursor;
        let anchor_before = self.anchor;
        let removed = self.text[start..end].to_string();
        self.text.replace_range(start..end, &new_region);
        self.cursor = cursor.min(self.text.len());
        self.anchor = None;
        self.preferred_col = None;
        self.undo_stack.push(Edit {
            start,
            removed,
            inserted: new_region,
            cursor_before,
            anchor_before,
            cursor_after: self.cursor,
        });
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.last_kind = Some(EditKind::Other);
        self.after_edit();
        self.cursor = self.clamp_to_boundary(self.cursor);
    }

    // ---- internals -------------------------------------------------------

    fn replace(&mut self, range: Range<usize>, inserted: &str, kind: EditKind) {
        if range.is_empty() && inserted.is_empty() {
            return;
        }
        let removed = self.text[range.clone()].to_string();
        let cursor_before = self.cursor;
        let anchor_before = self.anchor;
        self.text.replace_range(range.clone(), inserted);
        self.cursor = range.start + inserted.len();
        self.anchor = None;
        self.preferred_col = None;

        let edit = Edit {
            start: range.start,
            removed,
            inserted: inserted.to_string(),
            cursor_before,
            anchor_before,
            cursor_after: self.cursor,
        };
        if !self.coalesce(&edit, kind) {
            self.undo_stack.push(edit);
            if self.undo_stack.len() > MAX_UNDO {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
        self.last_kind = Some(kind);
        self.after_edit();
    }

    /// Merges consecutive typed characters (or consecutive deletions) into
    /// one undo step, breaking at whitespace so undo removes about a word at
    /// a time rather than a whole paragraph.
    fn coalesce(&mut self, edit: &Edit, kind: EditKind) -> bool {
        if kind == EditKind::Other || self.last_kind != Some(kind) {
            return false;
        }
        let Some(prev) = self.undo_stack.last_mut() else {
            return false;
        };
        match kind {
            EditKind::Typing => {
                let contiguous = prev.removed.is_empty()
                    && edit.removed.is_empty()
                    && prev.start + prev.inserted.len() == edit.start;
                let word_break = edit.inserted.starts_with(char::is_whitespace)
                    && !prev.inserted.ends_with(char::is_whitespace);
                if !contiguous || word_break {
                    return false;
                }
                prev.inserted.push_str(&edit.inserted);
                prev.cursor_after = edit.cursor_after;
                true
            }
            EditKind::Deleting => {
                if !(prev.inserted.is_empty() && edit.inserted.is_empty()) {
                    return false;
                }
                if edit.start + edit.removed.len() == prev.start {
                    // Backspacing: the new deletion sits just before the old.
                    prev.removed.insert_str(0, &edit.removed);
                    prev.start = edit.start;
                } else if edit.start == prev.start {
                    // Forward-deleting at the same spot.
                    prev.removed.push_str(&edit.removed);
                } else {
                    return false;
                }
                prev.cursor_after = edit.cursor_after;
                true
            }
            EditKind::Other => false,
        }
    }

    fn after_edit(&mut self) {
        self.recompute_lines();
        self.cursor = self.clamp_to_boundary(self.cursor);
    }

    fn recompute_lines(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'\n' {
                self.line_starts.push(i + 1);
            }
        }
    }

    fn indent_lines(&mut self, indent: bool) {
        let (first, last) = self.selected_line_span();
        let had_selection = self.selection().is_some();
        let start = self.line_starts[first];
        let end = self.line_range(last).end;
        let new_block: Vec<String> = (first..=last)
            .map(|l| {
                let t = self.line_text(l);
                if indent {
                    if t.is_empty() {
                        String::new()
                    } else {
                        format!("{}{t}", " ".repeat(INDENT))
                    }
                } else {
                    let remove = leading_whitespace(t).min(INDENT);
                    t[remove..].to_string()
                }
            })
            .collect();
        let joined = new_block.join("\n");
        let (cursor_line, cursor_col) = self.cursor_line_col();
        self.replace(start..end, &joined, EditKind::Other);
        if had_selection {
            // Keep the (now re-indented) lines selected so Tab/Shift+Tab can
            // be pressed repeatedly.
            self.anchor = Some(start);
            self.cursor = start + joined.len();
        } else {
            let range = self.line_range(cursor_line);
            let delta_col = if indent {
                cursor_col + INDENT
            } else {
                cursor_col.saturating_sub(INDENT)
            };
            self.cursor = self.clamp_to_boundary((range.start + delta_col).min(range.end));
        }
    }

    /// First and last line touched by the selection (or the caret's line).
    /// A selection ending at column 0 doesn't count that final line.
    fn selected_line_span(&self) -> (usize, usize) {
        match self.selection() {
            Some(sel) => {
                let (first, _) = self.line_col(sel.start);
                let (mut last, last_col) = self.line_col(sel.end);
                if last_col == 0 && last > first {
                    last -= 1;
                }
                (first, last)
            }
            None => {
                let (line, _) = self.cursor_line_col();
                (line, line)
            }
        }
    }

    fn char_col(&self, offset: usize) -> usize {
        let (line, _) = self.line_col(offset);
        self.text[self.line_starts[line]..offset].chars().count()
    }

    fn offset_for_char_col(&self, line: usize, col: usize) -> usize {
        let range = self.line_range(line);
        self.text[range.clone()]
            .char_indices()
            .nth(col)
            .map(|(i, _)| range.start + i)
            .unwrap_or(range.end)
    }

    fn clamp_to_boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map(|c| offset + c.len_utf8())
            .unwrap_or(self.text.len())
    }

    /// Ctrl+Left: skip whitespace, then a run of word or punctuation chars.
    fn word_boundary_before(&self, offset: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut i = offset;
        while i > 0 && bytes[i - 1] == b' ' {
            i -= 1;
        }
        if i > 0 && bytes[i - 1] == b'\n' {
            return i - 1;
        }
        if i > 0 && is_word_byte(bytes[i - 1]) {
            while i > 0 && is_word_byte(bytes[i - 1]) {
                i -= 1;
            }
        } else if i > 0 {
            i = self.prev_boundary(i);
        }
        self.clamp_to_boundary(i)
    }

    /// Ctrl+Right: skip a run of word or punctuation chars, then whitespace.
    fn word_boundary_after(&self, offset: usize) -> usize {
        let bytes = self.text.as_bytes();
        let len = bytes.len();
        let mut i = offset;
        if i < len && bytes[i] == b'\n' {
            return i + 1;
        }
        if i < len && is_word_byte(bytes[i]) {
            while i < len && is_word_byte(bytes[i]) {
                i += 1;
            }
        } else if i < len && bytes[i] != b' ' {
            i = self.next_boundary(i);
        }
        while i < len && bytes[i] == b' ' {
            i += 1;
        }
        self.clamp_to_boundary(i)
    }
}

/// Identifier characters (plus any non-ASCII byte, so a multi-byte char is
/// never split mid-sequence by the word scanners above).
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

fn leading_whitespace(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ' || *b == b'\t').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str, cursor: usize) -> TextBuffer {
        let mut b = TextBuffer::new(text.to_string());
        b.set_cursor(cursor, false);
        b
    }

    #[test]
    fn apply_edits_is_one_undo_step_and_keeps_caret_in_place() {
        let text = "package a;\n\nclass A {\n    void f() { List x; }\n}\n";
        let caret = text.find("x;").unwrap();
        let mut b = buf(text, caret);
        b.apply_edits(vec![
            (10..12, "\n\nimport java.util.List;\n\n".to_string()),
            (text.find('}').unwrap()..text.find('}').unwrap(), "/*end*/".to_string()),
        ]);
        assert!(b.text().contains("import java.util.List;\n\nclass A"));
        assert_eq!(&b.text()[b.cursor()..b.cursor() + 2], "x;", "caret follows its text");
        assert!(b.undo());
        assert_eq!(b.text(), text);
        assert!(!b.can_undo());
    }

    #[test]
    fn apply_edits_moves_caret_out_of_replaced_text_and_skips_overlaps() {
        let mut b = buf("abcdef", 3);
        b.apply_edits(vec![(2..4, "XY".into()), (3..5, "zz".into())]);
        assert_eq!(b.text(), "abXYef");
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn typing_replaces_selection() {
        let mut b = buf("hello world", 0);
        b.set_cursor(5, true);
        b.insert("X");
        assert_eq!(b.text(), "X world");
        assert_eq!(b.cursor(), 1);
    }

    #[test]
    fn shift_arrows_build_and_collapse_selection() {
        let mut b = buf("abcdef", 2);
        b.move_right(true);
        b.move_right(true);
        assert_eq!(b.selection(), Some(2..4));
        b.move_left(false);
        assert_eq!(b.cursor(), 2);
        assert_eq!(b.selection(), None);
    }

    #[test]
    fn typing_undoes_as_one_word() {
        let mut b = buf("", 0);
        for c in "int x".chars() {
            b.insert(&c.to_string());
        }
        assert_eq!(b.text(), "int x");
        b.undo();
        assert_eq!(b.text(), "int");
        b.undo();
        assert_eq!(b.text(), "");
        b.redo();
        assert_eq!(b.text(), "int");
    }

    #[test]
    fn backspaces_coalesce_and_undo_restores() {
        let mut b = buf("abcdef", 6);
        b.backspace();
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "abcdef");
        assert_eq!(b.cursor(), 6);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut b = buf("a", 1);
        b.insert("b");
        b.undo();
        b.insert("c");
        assert!(!b.redo());
        assert_eq!(b.text(), "ac");
    }

    #[test]
    fn enter_keeps_indent_and_opens_block() {
        let mut b = buf("    if (x) {", 12);
        b.insert_newline();
        assert_eq!(b.text(), "    if (x) {\n        ");
        assert_eq!(b.cursor(), b.text().len());
    }

    #[test]
    fn enter_between_braces_splits_them() {
        let mut b = buf("void f() {}", 10);
        b.insert_newline();
        assert_eq!(b.text(), "void f() {\n    \n}");
        assert_eq!(b.cursor(), "void f() {\n    ".len());
    }

    #[test]
    fn backspace_in_indent_removes_one_level() {
        let mut b = buf("        x", 8);
        b.backspace();
        assert_eq!(b.text(), "    x");
    }

    #[test]
    fn tab_indents_to_next_stop_and_block_indents_selection() {
        let mut b = buf("ab", 2);
        b.indent();
        assert_eq!(b.text(), "ab  ");

        let mut b = buf("a\nb\nc", 0);
        b.set_cursor(3, true);
        b.indent();
        assert_eq!(b.text(), "    a\n    b\nc");
        b.outdent();
        assert_eq!(b.text(), "a\nb\nc");
    }

    #[test]
    fn copy_and_cut_without_selection_take_the_line() {
        let mut b = buf("one\ntwo\nthree", 5);
        assert_eq!(b.copy_text(), "two\n");
        assert_eq!(b.cut(), "two\n");
        assert_eq!(b.text(), "one\nthree");
    }

    #[test]
    fn paste_normalizes_line_endings_and_tabs() {
        let mut b = buf("", 0);
        b.paste("a\r\n\tb");
        assert_eq!(b.text(), "a\n    b");
    }

    #[test]
    fn vertical_move_keeps_preferred_column() {
        let mut b = buf("abcdef\nx\nabcdef", 5);
        b.move_vertical(1, false);
        assert_eq!(b.cursor_line_col(), (1, 1));
        b.move_vertical(1, false);
        assert_eq!(b.cursor_line_col(), (2, 5));
    }

    #[test]
    fn smart_home_toggles_between_indent_and_column_zero() {
        let mut b = buf("    code", 8);
        b.move_line_start(false);
        assert_eq!(b.cursor(), 4);
        b.move_line_start(false);
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn word_movement() {
        let mut b = buf("int count = 3;", 0);
        b.move_word_right(false);
        assert_eq!(b.cursor(), 4);
        b.move_word_right(false);
        assert_eq!(b.cursor(), 10);
        b.move_word_left(false);
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn double_click_selects_word() {
        let mut b = buf("int count = 3;", 6);
        b.select_word_at(6);
        assert_eq!(b.selected_text(), Some("count"));
    }

    #[test]
    fn toggle_comment_round_trips() {
        let mut b = buf("    a();\n    b();", 0);
        b.set_cursor(12, true);
        b.toggle_line_comment();
        assert_eq!(b.text(), "    // a();\n    // b();");
        b.set_cursor(0, false);
        b.set_cursor(20, true);
        b.toggle_line_comment();
        assert_eq!(b.text(), "    a();\n    b();");
    }

    #[test]
    fn duplicate_line() {
        let mut b = buf("a\nb", 0);
        b.duplicate_lines();
        assert_eq!(b.text(), "a\na\nb");
        assert_eq!(b.cursor_line_col(), (1, 0));
    }

    #[test]
    fn multibyte_text_never_splits_a_char() {
        let mut b = buf("héllo", 0);
        b.move_right(false);
        b.move_right(false);
        assert_eq!(b.cursor(), 3);
        b.backspace();
        assert_eq!(b.text(), "hllo");
        b.set_cursor(2, false); // inside 'é' in the original — clamped safely
        assert!(b.text().is_char_boundary(b.cursor()));
    }
}
