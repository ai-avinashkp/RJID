//! A single open file's editor view. All text-editing rules (caret,
//! selection, undo/redo, auto-indent, ...) live in `rji_editor::TextBuffer`,
//! which is unit-tested without a window; this view only maps keyboard and
//! mouse input onto buffer operations and paints the result.
//!
//! Scope notes:
//! - Text entry uses raw `on_key_down` + `Keystroke::key_char`, not GPUI's
//!   IME (`EntityInputHandler`) machinery — so no CJK-style composition
//!   yet. Latin typing, navigation, and editing all work.
//! - Lines render through `uniform_list`, which only builds rows scrolled
//!   into view (a 7k-line `Cargo.lock` lagged without it). Virtualization
//!   needs fixed-height rows, so long lines scroll horizontally, not wrap.
//! - Each visible line is a custom `Element` (`CodeLineElement`) that
//!   shapes its real glyph run, so caret/selection positions and click
//!   hit-testing use the real shaping instead of an estimated char width.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, Entity, FocusHandle, Focusable,
    GlobalElementId, KeyDownEvent, LayoutId, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, Rgba,
    ScrollStrategy, ShapedLine, SharedString, Style, TextRun, UniformListScrollHandle, Window,
    div, fill, point, prelude::*, px, size, uniform_list,
};
use rji_editor::{Language, LineState, TextBuffer, Token, TokenKind, highlight_line, scan_line_state};
use rji_lsp_client::{Diagnostic, DiagnosticSeverity};
use rji_theme::Theme;

mod code_actions;
mod find;
mod hover;
mod navigation;
pub use navigation::Location;
mod pom_fixes;

use find::{FindBar, FindMode};

use crate::completion::{CompletionItem, CompletionMenu, MenuPlacement, render_completion_menu};
use crate::devtools_hook::SharedDevtoolsHook;
use crate::lsp_shared::SharedLspClient;
use crate::scrollbar::{scroll_thumb, scroll_thumb_horizontal};

pub const LINE_HEIGHT: f32 = 20.0;

/// What the window's status bar shows about the active editor.
pub struct EditorStatus {
    pub line: usize,
    pub column: usize,
    pub selected_chars: usize,
    pub language: &'static str,
    pub line_ending: &'static str,
    pub errors: usize,
    pub warnings: usize,
    /// The diagnostic message on the caret's line, if any.
    pub line_message: Option<(DiagnosticSeverity, String)>,
}

pub struct CodeEditorView {
    pub path: PathBuf,
    buffer: TextBuffer,
    language: Language,
    /// Highlighter state at the start of each line (inside a block comment,
    /// text block, ...), recomputed once per edit — not per frame.
    line_states: Vec<LineState>,
    /// Content as last loaded/saved — `dirty` is derived by comparing
    /// against it, so undoing back to the saved state clears the marker.
    saved_text: String,
    /// The file used Windows line endings on disk. The buffer always works
    /// in `\n`; saving converts back so files keep their original style.
    crlf: bool,
    pub dirty: bool,
    pub theme: Theme,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    lsp: SharedLspClient,
    devtools: SharedDevtoolsHook,
    doc_version: i64,
    /// 0-based line -> most severe diagnostic (and its message) on it.
    diagnostics: HashMap<u32, (DiagnosticSeverity, String)>,
    /// Every diagnostic with its range (for underlines and hover).
    diagnostic_list: Vec<Diagnostic>,
    hover: Option<hover::HoverState>,
    /// Target the pointer is resting on, waiting for the hover delay.
    hover_pending: Option<hover::HoverTarget>,
    /// Bumped on every pointer move / dismissal; a delayed hover only
    /// shows if nothing happened since it was scheduled.
    hover_generation: u64,
    /// Right-click menu, when open.
    context_menu: Option<code_actions::ContextMenu>,
    /// A step-by-step action dialog (generate, rename, quick fix, ...).
    dialog: Option<code_actions::ActionDialog>,
    /// Add unambiguous imports automatically when types can't be resolved.
    pub auto_import: bool,
    /// Unresolved names the last automatic import attempt was for.
    last_auto_import: String,
    /// Missing-version problems the last automatic pom fix was for.
    last_pom_fix: String,
    /// 0-based lines with a breakpoint, toggled by clicking the gutter.
    /// `RootView` reads this when starting a debug session.
    pub breakpoints: std::collections::BTreeSet<u32>,
    /// 0-based line the debugger reports the program is suspended at.
    pub paused_line: Option<u32>,
    /// Each visible line's latest shaping + paint bounds, recorded by
    /// `CodeLineElement::paint` and read back to hit-test mouse positions.
    line_layouts: HashMap<usize, (ShapedLine, Bounds<Pixels>)>,
    /// Advance width of one monospace character, measured from real
    /// shaping — used to keep the caret horizontally in view.
    char_width: Pixels,
    gutter_width: f32,
    /// Pane width the rows were last laid out for (see `render`).
    laid_out_width: f32,
    /// Bumped on every edit; lets async work (plugins) detect that the
    /// document changed underneath it.
    edit_version: u64,
    /// True between a left-button press on the text and its release, so
    /// mouse moves extend the selection (drag-select).
    drag_selecting: bool,
    completion: Option<CompletionMenu>,
    find: Option<FindBar>,
    /// UI zoom factor (Ctrl+=/-), set by `RootView`; row heights scale
    /// with it so zoomed text never overflows its line.
    pub zoom: f32,
    /// Library / JDK source opened by Go to Definition: viewable, not editable.
    pub read_only: bool,
    /// Code font (Theme & Fonts); sizes are before UI zoom.
    pub font_family: gpui::SharedString,
    pub font_size: f32,
    /// Ctrl+hover link underline (see `navigation`).
    ctrl_link: Option<navigation::CtrlLink>,
    ctrl_link_generation: u64,
    /// Line and position of the last mouse move over the text, so pressing
    /// Ctrl without moving can show the link.
    last_pointer: Option<(usize, Point<Pixels>)>,
}

impl CodeEditorView {
    pub fn new(
        path: PathBuf,
        content: String,
        theme: Theme,
        lsp: SharedLspClient,
        devtools: SharedDevtoolsHook,
        cx: &mut Context<Self>,
    ) -> Self {
        // Normalize Windows line endings on load: the buffer, line math, and
        // rendering all assume `\n`. `save` restores `\r\n` for such files.
        let crlf = content.contains("\r\n");
        let content = content.replace("\r\n", "\n");
        let language = Language::for_extension(
            &path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase(),
        );
        let mut view = CodeEditorView {
            path,
            language,
            line_states: Vec::new(),
            crlf,
            saved_text: content.clone(),
            buffer: TextBuffer::new(content),
            dirty: false,
            theme,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            lsp,
            devtools,
            doc_version: 1,
            diagnostics: HashMap::new(),
            diagnostic_list: Vec::new(),
            hover: None,
            hover_pending: None,
            hover_generation: 0,
            context_menu: None,
            dialog: None,
            auto_import: true,
            last_auto_import: String::new(),
            last_pom_fix: String::new(),
            breakpoints: std::collections::BTreeSet::new(),
            paused_line: None,
            line_layouts: HashMap::new(),
            char_width: px(8.),
            gutter_width: 48.,
            laid_out_width: 0.,
            edit_version: 0,
            drag_selecting: false,
            completion: None,
            find: None,
            zoom: 1.0,
            read_only: false,
            font_family: crate::fonts::MONO.into(),
            font_size: rji_settings::DEFAULT_EDITOR_FONT_SIZE,
            ctrl_link: None,
            ctrl_link_generation: 0,
            last_pointer: None,
        };
        view.recompute_line_states();
        view
    }

    fn recompute_line_states(&mut self) {
        let mut state = LineState::Normal;
        self.line_states.clear();
        self.line_states.reserve(self.buffer.line_count());
        for line in 0..self.buffer.line_count() {
            self.line_states.push(state);
            state = scan_line_state(self.language, self.buffer.line_text(line), state);
        }
    }

    /// Row height: ~1.43× the font size (20 px at the default 14 px).
    fn line_height(&self) -> f32 {
        (self.font_size * LINE_HEIGHT / rji_settings::DEFAULT_EDITOR_FONT_SIZE).round() * self.zoom
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string())
    }

    pub fn is_java(&self) -> bool {
        self.path.extension().and_then(|e| e.to_str()) == Some("java")
    }

    pub fn language(&self) -> &'static str {
        match self.path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "java" => "Java",
            "xml" => "XML",
            "gradle" | "kts" => "Gradle",
            "properties" => "Properties",
            "json" => "JSON",
            "md" => "Markdown",
            "yml" | "yaml" => "YAML",
            "toml" => "TOML",
            "rs" => "Rust",
            _ => "Plain Text",
        }
    }

    pub fn status(&self) -> EditorStatus {
        let (line, byte_col) = self.buffer.cursor_line_col();
        let line_start = self.buffer.line_starts()[line];
        let column = self.buffer.text()[line_start..line_start + byte_col]
            .chars()
            .count();
        let count = |wanted: DiagnosticSeverity| {
            self.diagnostics
                .values()
                .filter(|(s, _)| *s == wanted)
                .count()
        };
        EditorStatus {
            line: line + 1,
            column: column + 1,
            selected_chars: self
                .buffer
                .selected_text()
                .map_or(0, |t| t.chars().count()),
            language: self.language(),
            line_ending: if self.crlf { "CRLF" } else { "LF" },
            errors: count(DiagnosticSeverity::Error),
            warnings: count(DiagnosticSeverity::Warning),
            line_message: self.diagnostics.get(&(line as u32)).cloned(),
        }
    }

    /// Best-effort fully-qualified class name: the file stem, prefixed with
    /// any `package ...;` declaration near the top. Used as the debugger's
    /// `ClassPrepare` filter; a wrong guess only means that file's
    /// breakpoints never resolve.
    pub fn fully_qualified_class_name(&self) -> Option<String> {
        crate::debug_session::java_class_name(&self.path, self.buffer.text())
    }

    /// Replaces the breakpoint set (restoring breakpoints when a file is
    /// reopened). Doesn't emit `BreakpointsChanged` — the caller already
    /// knows.
    pub fn set_breakpoints(&mut self, lines: std::collections::BTreeSet<u32>) {
        self.breakpoints = lines;
    }

    /// Moves the caret to the start of a 0-based line and scrolls it to the
    /// middle of the view — used when the debugger stops somewhere.
    /// What a plugin command operates on: the selection, or the whole
    /// document if nothing is selected. Returns `(text, is_selection,
    /// range, edit_version)`.
    pub fn plugin_target(&self) -> (String, bool, std::ops::Range<usize>, u64) {
        match self.buffer.selection() {
            Some(range) => (self.buffer.text()[range.clone()].to_string(), true, range, self.edit_version),
            None => (self.buffer.text().to_string(), false, 0..self.buffer.text().len(), self.edit_version),
        }
    }

    /// Applies a plugin's result as one undoable edit — unless the document
    /// changed while the plugin ran (then nothing is touched).
    pub fn apply_plugin_replace(&mut self, range: std::ops::Range<usize>, text: &str, version: u64) -> bool {
        if version != self.edit_version || range.end > self.buffer.text().len() {
            return false;
        }
        self.buffer.replace_range(range.clone(), text);
        self.buffer.select_range(range.start..range.start + text.len());
        self.after_edit();
        true
    }

    /// Re-reads the file after something else changed it (e.g. an applied
    /// update). Refuses if there are unsaved edits, so nothing is lost; the
    /// reload itself is one undoable step.
    pub fn reload_from_disk(&mut self) -> bool {
        if self.dirty {
            return false;
        }
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        let content = content.replace("\r\n", "\n");
        if content == self.buffer.text() {
            return true;
        }
        let cursor = self.buffer.cursor().min(content.len());
        self.buffer.replace_all(&content);
        self.buffer.set_cursor(cursor, false);
        self.saved_text = content;
        self.after_edit();
        true
    }

    pub fn reveal_line(&mut self, line: u32) {
        let line = (line as usize).min(self.buffer.line_count() - 1);
        let start = self.buffer.line_starts()[line];
        let indent = self
            .buffer
            .line_text(line)
            .bytes()
            .take_while(|b| *b == b' ')
            .count();
        self.buffer.set_cursor(start + indent, false);
        self.completion = None;
        self.scroll_handle.scroll_to_item_strict(line, ScrollStrategy::Center);
    }

    /// Replaces this file's diagnostics with the LSP's latest report,
    /// keeping the most severe per line (the gutter shows one tint).
    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        self.diagnostics.clear();
        self.diagnostic_list = diagnostics.clone();
        for diagnostic in diagnostics {
            let entry = self
                .diagnostics
                .entry(diagnostic.line)
                .or_insert((diagnostic.severity, diagnostic.message.clone()));
            if severity_rank(diagnostic.severity) < severity_rank(entry.0) {
                *entry = (diagnostic.severity, diagnostic.message);
            }
        }
        self.maybe_auto_import(cx);
        self.maybe_fix_pom_versions(cx);
    }

    /// Call after every buffer mutation: refreshes the dirty flag and
    /// tells the language server (best-effort — an LSP hiccup must never
    /// break editing).
    fn after_edit(&mut self) {
        if self.read_only {
            // Every edit path ends here: put the text back, keep the caret.
            let cursor = self.buffer.cursor().min(self.saved_text.len());
            self.buffer = TextBuffer::new(self.saved_text.clone());
            self.buffer.set_cursor(cursor, false);
            self.dirty = false;
            self.recompute_line_states();
            return;
        }
        self.edit_version += 1;
        self.clear_hover();
        self.ctrl_link = None;
        self.dirty = self.buffer.text() != self.saved_text;
        self.recompute_line_states();
        self.recompute_matches();
        if !self.is_java() {
            return;
        }
        self.doc_version += 1;
        if let Some(client) = self.lsp.borrow_mut().as_mut() {
            let _ = client.did_change(&self.path, self.doc_version, self.buffer.text());
        }
    }

    /// Never panics on an I/O error — the dirty marker just stays set so
    /// the failure is visible instead of silently losing the save.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let on_disk = if self.crlf {
            std::borrow::Cow::Owned(self.buffer.text().replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(self.buffer.text())
        };
        if std::fs::write(&self.path, on_disk.as_bytes()).is_ok() {
            self.saved_text = self.buffer.text().to_string();
            self.dirty = false;
            if self.is_java()
                && let Some(hook) = self.devtools.borrow().as_ref()
            {
                hook.trigger();
            }
            cx.emit(EditorEvent::Saved);
        }
    }

    fn toggle_breakpoint(&mut self, line: usize, cx: &mut Context<Self>) {
        let line = line as u32;
        if !self.breakpoints.remove(&line) {
            self.breakpoints.insert(line);
        }
        cx.emit(EditorEvent::BreakpointsChanged);
        cx.notify();
    }

    /// Maps a window-space mouse position on `line` to a document offset,
    /// using that line's last real shaping. Clamped to the line's real
    /// length: a blank line shapes a placeholder space so it has a
    /// clickable shape, and must not resolve past its own end.
    fn offset_at(&self, line: usize, position: Point<Pixels>) -> usize {
        let line = line.min(self.buffer.line_count() - 1);
        let range = self.buffer.line_range(line);
        let line_len = range.end - range.start;
        let col = self
            .line_layouts
            .get(&line)
            .map(|(shaped, bounds)| {
                if position.x < bounds.left() {
                    0
                } else {
                    shaped.closest_index_for_x(position.x - bounds.left())
                }
            })
            .unwrap_or(line_len)
            .min(line_len);
        range.start + col
    }

    fn visible_line_count(&self) -> usize {
        let height = self.scroll_handle.0.borrow().base_handle.bounds().size.height;
        ((f32::from(height) / self.line_height()) as usize).saturating_sub(1).max(1)
    }

    /// Scrolls just enough to keep the caret visible, vertically (whole
    /// lines) and horizontally (by measured character width).
    fn scroll_cursor_into_view(&self) {
        let (line, byte_col) = self.buffer.cursor_line_col();
        self.scroll_handle.scroll_to_item(line, ScrollStrategy::Nearest);

        let state = self.scroll_handle.0.borrow();
        let handle = &state.base_handle;
        let viewport_w = f32::from(handle.bounds().size.width) - self.gutter_width;
        if viewport_w <= 0.0 {
            return;
        }
        let line_start = self.buffer.line_starts()[line];
        let chars = self.buffer.text()[line_start..line_start + byte_col]
            .chars()
            .count();
        let caret_x = chars as f32 * f32::from(self.char_width);
        let margin = f32::from(self.char_width) * 4.0;
        let mut offset = handle.offset();
        let scrolled = -f32::from(offset.x);
        if caret_x + margin > scrolled + viewport_w {
            offset.x = px(-(caret_x + margin - viewport_w));
        } else if caret_x < scrolled + margin {
            offset.x = px(-(caret_x - margin).max(0.0));
        } else {
            return;
        }
        handle.set_offset(offset);
    }

    // ---- completion -------------------------------------------------------

    /// Byte range of the identifier being typed immediately before the
    /// caret — what a chosen completion replaces.
    fn word_prefix_range(&self) -> Range<usize> {
        let cursor = self.buffer.cursor();
        let bytes = self.buffer.text().as_bytes();
        let mut start = cursor;
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
        {
            start -= 1;
        }
        start..cursor
    }

    /// Opens (or refreshes) the completion menu for the word at the caret.
    /// `explicit` (Ctrl+Space) opens even with an empty prefix; otherwise
    /// it needs at least one typed identifier character, or a `.` trigger.
    fn update_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let prefix_range = self.word_prefix_range();
        let prefix = self.buffer.text()[prefix_range.clone()].to_string();
        let after_dot = prefix_range.start > 0
            && self.buffer.text().as_bytes()[prefix_range.start - 1] == b'.';
        if !explicit && prefix.is_empty() && !after_dot {
            self.completion = None;
            return;
        }

        // Already showing a list for this same word: just re-filter it
        // locally instead of re-querying the server on every keystroke.
        if let Some(menu) = self.completion.as_mut()
            && menu.replace_start == prefix_range.start
        {
            menu.set_filter(&prefix);
            if menu.is_empty() && !menu.loading {
                self.completion = None;
            }
            return;
        }

        let mut menu = CompletionMenu::new(prefix_range.start, &prefix);
        let (line, col) = self.buffer.cursor_line_col();
        let lsp_request = if self.is_java() {
            self.lsp
                .borrow_mut()
                .as_mut()
                .and_then(|client| client.request_completion(&self.path, line as u32, col as u32).ok())
        } else {
            None
        };

        if let Some(rx) = lsp_request {
            menu.loading = true;
            menu.set_items(crate::completion::fallback_items(
                self.buffer.text(),
                &prefix,
                !after_dot && self.is_java(),
            ));
            let replace_start = menu.replace_start;
            cx.spawn(async move |this, cx| {
                let Ok(items) = rx.recv().await else {
                    return;
                };
                this.update(cx, |this, cx| {
                    if let Some(menu) = this.completion.as_mut()
                        && menu.replace_start == replace_start
                    {
                        menu.loading = false;
                        menu.merge_lsp_items(
                            items
                                .into_iter()
                                .map(|i| CompletionItem {
                                    label: i.label,
                                    insert_text: i.insert_text,
                                    detail: i.detail,
                                    kind: i.kind,
                                    additional_edits: i.additional_edits,
                                })
                                .collect(),
                        );
                        if menu.is_empty() {
                            this.completion = None;
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        } else {
            menu.set_items(crate::completion::fallback_items(
                self.buffer.text(),
                &prefix,
                !after_dot && self.is_java(),
            ));
            if menu.is_empty() {
                self.completion = None;
                return;
            }
        }
        self.completion = Some(menu);
    }

    fn accept_completion(&mut self) -> bool {
        let Some(menu) = self.completion.take() else {
            return false;
        };
        let Some(item) = menu.selected_item() else {
            return false;
        };
        let range = menu.replace_start..self.buffer.cursor();
        // Multi-line snippets are written at column 0; indent their
        // continuation lines to match the line they're inserted on.
        let (line, _) = self.buffer.cursor_line_col();
        let indent: String = self
            .buffer
            .line_text(line)
            .chars()
            .take_while(|c| *c == ' ')
            .collect();
        let text = item.insert_text.replace('\n', &format!("\n{indent}"));
        // Snippet-style items mark where the caret should land with `$0`.
        if let Some(caret) = text.find("$0") {
            let clean = text.replacen("$0", "", 1);
            self.buffer.replace_range(range.clone(), &clean);
            self.buffer.set_cursor(range.start + caret, false);
        } else {
            self.buffer.replace_range(range, &text);
        }
        self.after_edit();
        // A class from another package brings its `import` along (edits
        // above the insertion point, so the positions are still valid).
        if !item.additional_edits.is_empty() {
            let edits = item.additional_edits.clone();
            self.apply_text_edits(&edits);
        }
        true
    }

    /// Mouse pick from the popup.
    pub fn pick_completion(&mut self, visible_index: usize, cx: &mut Context<Self>) {
        if let Some(menu) = self.completion.as_mut() {
            menu.select(visible_index);
        }
        if self.accept_completion() {
            self.scroll_cursor_into_view();
        }
        cx.notify();
    }

    // ---- commands (shared by keyboard shortcuts and the menu bar) --------

    pub fn command(&mut self, command: EditorCommand, window: &mut Window, cx: &mut Context<Self>) {
        let mut edited = false;
        match command {
            EditorCommand::Undo => edited = self.buffer.undo(),
            EditorCommand::Redo => edited = self.buffer.redo(),
            EditorCommand::Cut => {
                let cut = self.buffer.cut();
                cx.write_to_clipboard(ClipboardItem::new_string(cut));
                edited = true;
            }
            EditorCommand::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.copy_text()));
            }
            EditorCommand::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.buffer.paste(&text);
                    edited = true;
                }
            }
            EditorCommand::SelectAll => self.buffer.select_all(),
            EditorCommand::SelectLine => {
                let (line, _) = self.buffer.cursor_line_col();
                self.buffer.select_line(line);
            }
            EditorCommand::ToggleComment => {
                self.buffer.toggle_line_comment();
                edited = true;
            }
            EditorCommand::DuplicateLine => {
                self.buffer.duplicate_lines();
                edited = true;
            }
            EditorCommand::Indent => {
                self.buffer.indent();
                edited = true;
            }
            EditorCommand::Outdent => {
                self.buffer.outdent();
                edited = true;
            }
            EditorCommand::Completion => {
                window.focus(&self.focus_handle, cx);
                self.update_completion(true, cx);
                return cx.notify();
            }
            EditorCommand::Find => return self.open_find(FindMode::Find, false, window, cx),
            EditorCommand::Replace => return self.open_find(FindMode::Find, true, window, cx),
            EditorCommand::GoToLine => return self.open_find(FindMode::GoToLine, false, window, cx),
            EditorCommand::GoToDefinition => {
                let offset = self.buffer.cursor();
                return self.go_to_definition(offset, cx);
            }
            EditorCommand::QuickFix => return self.quick_fix(cx),
            EditorCommand::Rename => return self.rename_symbol(window, cx),
            EditorCommand::Refactor => return self.refactor(cx),
            EditorCommand::OrganizeImports => return self.organize_imports(cx),
            EditorCommand::AddMissingImports => return self.add_missing_imports(false, cx),
            EditorCommand::FormatDocument => return self.format_document(cx),
            EditorCommand::GenerateConstructor
            | EditorCommand::GenerateAccessors
            | EditorCommand::GenerateGetters
            | EditorCommand::GenerateSetters
            | EditorCommand::GenerateToString
            | EditorCommand::GenerateEqualsHashCode
            | EditorCommand::GenerateOverrides
            | EditorCommand::GenerateDelegates => return self.generate(command, cx),
            EditorCommand::SourceAction => return self.source_action(cx),
            EditorCommand::ContextMenu => return self.open_context_menu_at_caret(cx),
            EditorCommand::FindNext | EditorCommand::FindPrevious => {
                if self.find.as_ref().is_none_or(|bar| bar.mode != FindMode::Find) {
                    return self.open_find(FindMode::Find, false, window, cx);
                }
                self.find_step(command == EditorCommand::FindNext, window, cx);
                return;
            }
        }
        if edited {
            self.after_edit();
        }
        self.completion = None;
        self.scroll_cursor_into_view();
        cx.notify();
    }

    // ---- input -----------------------------------------------------------

    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.hover.is_some() || self.hover_pending.is_some() {
            self.clear_hover();
            cx.notify();
        }
        let m = &keystroke.modifiers;
        let ctrl = m.control || m.platform;
        let shift = m.shift;
        let key = keystroke.key.as_str();
        let alt = m.alt;

        // An open dialog or menu takes the keyboard (typing must never
        // reach the document underneath).
        if self.dialog.is_some() {
            if self.dialog_key(key, window, cx) {
                cx.stop_propagation();
            }
            return;
        }
        if self.context_menu.is_some() {
            self.context_menu = None;
            cx.notify();
            if key == "escape" {
                cx.stop_propagation();
                return;
            }
        }

        // Shortcuts that work anywhere in the editor, including while the
        // find bar's input has focus (its unhandled keys bubble up here).
        let global = match key {
            "f" if ctrl && !shift => Some(EditorCommand::Find),
            "h" if ctrl => Some(EditorCommand::Replace),
            "g" if ctrl => Some(EditorCommand::GoToLine),
            "f3" if shift => Some(EditorCommand::FindPrevious),
            "f3" => Some(EditorCommand::FindNext),
            "f12" if !ctrl && !shift => Some(EditorCommand::GoToDefinition),
            "." if ctrl => Some(EditorCommand::QuickFix),
            "f2" if !ctrl && !alt => Some(EditorCommand::Rename),
            "o" if shift && alt => Some(EditorCommand::OrganizeImports),
            "f" if shift && alt => Some(EditorCommand::FormatDocument),
            "r" if ctrl && shift => Some(EditorCommand::Refactor),
            "insert" if alt => Some(EditorCommand::SourceAction),
            "s" if shift && alt => Some(EditorCommand::SourceAction),
            "f10" if shift => Some(EditorCommand::ContextMenu),
            _ => None,
        };
        if let Some(command) = global {
            self.command(command, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.find.is_some() && key == "escape" {
            self.close_find(window, cx);
            cx.stop_propagation();
            return;
        }
        // Everything else only applies when the text itself has focus —
        // not while typing in the find bar.
        if !self.focus_handle.is_focused(window) {
            return;
        }

        // While the completion menu is open, it owns navigation keys.
        if self.completion.is_some() {
            match key {
                "up" => {
                    if let Some(menu) = self.completion.as_mut() {
                        menu.select_prev();
                    }
                    cx.stop_propagation();
                    return cx.notify();
                }
                "down" => {
                    if let Some(menu) = self.completion.as_mut() {
                        menu.select_next();
                    }
                    cx.stop_propagation();
                    return cx.notify();
                }
                "enter" | "tab" if !shift && !ctrl => {
                    if self.accept_completion() {
                        self.scroll_cursor_into_view();
                        cx.stop_propagation();
                        return cx.notify();
                    }
                }
                "escape" => {
                    self.completion = None;
                    cx.stop_propagation();
                    return cx.notify();
                }
                _ => {}
            }
        }

        let mut edited = false;
        let mut typed_word_char = false;
        let mut keep_completion = false;
        match key {
            "left" if ctrl => self.buffer.move_word_left(shift),
            "right" if ctrl => self.buffer.move_word_right(shift),
            "left" => self.buffer.move_left(shift),
            "right" => self.buffer.move_right(shift),
            "up" => self.buffer.move_vertical(-1, shift),
            "down" => self.buffer.move_vertical(1, shift),
            "home" if ctrl => self.buffer.move_doc_start(shift),
            "end" if ctrl => self.buffer.move_doc_end(shift),
            "home" => self.buffer.move_line_start(shift),
            "end" => self.buffer.move_line_end(shift),
            "pageup" => self
                .buffer
                .move_vertical(-(self.visible_line_count() as i64), shift),
            "pagedown" => self
                .buffer
                .move_vertical(self.visible_line_count() as i64, shift),
            "backspace" if ctrl => {
                self.buffer.delete_word_back();
                edited = true;
            }
            "backspace" => {
                self.buffer.backspace();
                edited = true;
                keep_completion = true;
            }
            "delete" => {
                self.buffer.delete_forward();
                edited = true;
            }
            "enter" => {
                self.buffer.insert_newline();
                edited = true;
            }
            "tab" if ctrl => return, // Ctrl+Tab switches tabs (RootView)
            "tab" if shift => {
                self.buffer.outdent();
                edited = true;
            }
            "tab" => {
                self.buffer.indent();
                edited = true;
            }
            "escape" => {
                let cursor = self.buffer.cursor();
                self.buffer.set_cursor(cursor, false);
            }
            "s" if ctrl && !shift => self.save(cx), // Ctrl+Shift+S (save all) is RootView's
            k if ctrl => {
                let command = match (k, shift) {
                    ("space", _) => EditorCommand::Completion,
                    ("a", _) => EditorCommand::SelectAll,
                    ("c", _) => EditorCommand::Copy,
                    ("x", _) => EditorCommand::Cut,
                    ("v", _) => EditorCommand::Paste,
                    ("z", true) | ("y", _) => EditorCommand::Redo,
                    ("z", false) => EditorCommand::Undo,
                    ("d", _) => EditorCommand::DuplicateLine,
                    ("/", _) => EditorCommand::ToggleComment,
                    ("l", _) => EditorCommand::SelectLine,
                    // Not ours (Ctrl+O, zoom, Ctrl+W, ...) — RootView's.
                    _ => return,
                };
                self.command(command, window, cx);
                cx.stop_propagation();
                return;
            }
            _ => {
                if ctrl || m.alt || m.function {
                    // Not ours (e.g. Ctrl+O / zoom) — let RootView handle it.
                    return;
                }
                let Some(ch) = keystroke.key_char.as_ref() else {
                    return;
                };
                self.buffer.insert(ch);
                edited = true;
                typed_word_char = ch
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            }
        }

        if edited {
            self.after_edit();
        }
        // Suggest-as-you-type only in code; Ctrl+Space works in any file.
        let auto_complete = self.language == Language::Java;
        if (typed_word_char && auto_complete) || (keep_completion && self.completion.is_some()) {
            self.update_completion(false, cx);
        } else {
            self.completion = None;
        }
        self.scroll_cursor_into_view();
        cx.stop_propagation();
        cx.notify();
    }

    fn on_line_mouse_down(
        &mut self,
        line: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.completion = None;
        self.clear_hover();
        let offset = self.offset_at(line, event.position);
        // Ctrl+Click (Cmd+Click on macOS): Go to Definition.
        if (event.modifiers.control || event.modifiers.platform) && event.click_count == 1 && !event.modifiers.shift {
            self.buffer.set_cursor(offset, false);
            self.go_to_definition(offset, cx);
            cx.notify();
            return;
        }
        match event.click_count {
            2 => self.buffer.select_word_at(offset),
            3 => self.buffer.select_line(line),
            _ => {
                self.buffer.set_cursor(offset, event.modifiers.shift);
                self.drag_selecting = true;
            }
        }
        cx.notify();
    }

    fn on_line_mouse_move(&mut self, line: usize, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.drag_selecting {
            if event.pressed_button.is_none() {
                self.last_pointer = Some((line, event.position));
                let ctrl = event.modifiers.control || event.modifiers.platform;
                self.update_ctrl_link(line, event.position, ctrl, cx);
                self.on_text_hover(line, event.position, cx);
            }
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            // Button released somewhere we didn't see (e.g. outside the
            // window) — stop extending.
            self.drag_selecting = false;
            return;
        }
        let offset = self.offset_at(line, event.position);
        if offset != self.buffer.cursor() {
            self.buffer.set_cursor(offset, true);
            cx.notify();
        }
    }
}

/// Editor actions invokable from the keyboard or the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    SelectLine,
    ToggleComment,
    DuplicateLine,
    Indent,
    Outdent,
    Completion,
    Find,
    Replace,
    FindNext,
    FindPrevious,
    GoToLine,
    QuickFix,
    Rename,
    Refactor,
    OrganizeImports,
    AddMissingImports,
    FormatDocument,
    GenerateConstructor,
    GenerateAccessors,
    GenerateGetters,
    GenerateSetters,
    GenerateToString,
    GenerateEqualsHashCode,
    GenerateOverrides,
    GenerateDelegates,
    /// The Source Action list (generate, organize imports, ...).
    SourceAction,
    GoToDefinition,
    /// The right-click menu, at the caret (Alt+Insert, Shift+F10).
    ContextMenu,
}

/// Things `RootView` needs to hear about from an editor.
pub enum EditorEvent {
    /// A gutter click added or removed a breakpoint.
    BreakpointsChanged,
    /// The file was written to disk.
    Saved,
    /// A short message for the status area.
    Notice(String),
    /// Language-server edits to other files (rename, "create class").
    ExternalEdits(Vec<rji_lsp_client::FileEdit>),
    /// Go to Definition landed in another file (project or library).
    OpenLocation(Location),
}

impl gpui::EventEmitter<EditorEvent> for CodeEditorView {}

impl Focusable for CodeEditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CodeEditorView {
    /// One row: gutter (line number, diagnostic tint, breakpoint dot) plus
    /// the shaped, clickable code content.
    fn render_line(
        &self,
        line_idx: usize,
        gutter_width: f32,
        viewport_w: f32,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let line_h = self.line_height();
        let zoom = self.zoom;
        let range = self.buffer.line_range(line_idx);
        let line_text = &self.buffer.text()[range.clone()];
        let (cursor_line, cursor_col) = self.buffer.cursor_line_col();
        let start_state = self
            .line_states
            .get(line_idx)
            .copied()
            .unwrap_or_default();
        let (tokens, _) = highlight_line(self.language, line_text, start_state);
        let selection = self.buffer.selection();

        // Selected part of this line, as byte columns within it, plus
        // whether the selection continues past the line's end (so the
        // newline itself reads as selected).
        let line_selection = selection.as_ref().and_then(|sel| {
            let line_end_incl_newline = range.end + 1;
            if sel.end <= range.start || sel.start >= line_end_incl_newline {
                return None;
            }
            let start = sel.start.max(range.start) - range.start;
            let end = sel.end.min(range.end) - range.start;
            Some((start, end, sel.end > range.end))
        });

        let gutter_color = self
            .diagnostics
            .get(&(line_idx as u32))
            .map(|(severity, _)| match severity {
                DiagnosticSeverity::Error => theme.error,
                DiagnosticSeverity::Warning => theme.warning,
                DiagnosticSeverity::Info | DiagnosticSeverity::Hint => theme.accent,
            })
            .unwrap_or(if line_idx == cursor_line {
                theme.foreground
            } else {
                theme.foreground_muted
            });

        let has_breakpoint = self.breakpoints.contains(&(line_idx as u32));
        let is_paused_here = self.paused_line == Some(line_idx as u32);
        let is_cursor_line = line_idx == cursor_line;

        let gutter = div()
            .id(("gutter", line_idx))
            .relative()
            .flex_shrink_0()
            .w(px(gutter_width))
            .h(px(line_h))
            .pr_2()
            .flex()
            .justify_end()
            .cursor_pointer()
            .text_color(gutter_color)
            .child((line_idx + 1).to_string())
            // Self-drawn breakpoint marker in the gutter's left padding.
            .when(has_breakpoint, |s| {
                s.child(
                    div()
                        .absolute()
                        .left(px(4. * zoom))
                        .top(px((line_h - 10. * zoom) / 2.))
                        .w(px(10. * zoom))
                        .h(px(10. * zoom))
                        .rounded_full()
                        .bg(theme.error),
                )
            })
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button.is_none() {
                    this.on_gutter_hover(line_idx, cx);
                }
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.toggle_breakpoint(line_idx, cx);
                    // Keep the row's own handler (caret placement) from
                    // also firing for this click.
                    cx.stop_propagation();
                }),
            );

        let code_element = CodeLineElement {
            editor: cx.entity(),
            line_idx,
            tokens,
            is_cursor_line,
            cursor_byte_col: cursor_col.min(line_text.len()),
            selection: line_selection,
            matches: self.match_spans_on_line(range.clone()),
            underlines: self.diagnostic_underlines(line_idx, line_text, theme),
            link: self.ctrl_link_on_line(range.clone()),
            line_height: line_h,
            theme,
        };

        div()
            .id(line_idx)
            .flex()
            .flex_row()
            .h(px(line_h))
            .when(is_cursor_line && selection.is_none(), |s| {
                s.bg(theme.foreground.opacity(0.05))
            })
            .when(is_paused_here, |s| s.bg(theme.warning.opacity(0.22)))
            // `uniform_list`'s "Unconstrained" sizing lets each row take its
            // own natural width. Flooring it at the viewport width keeps a
            // short line clickable (and highlightable) across the whole
            // visible pane; longer rows still grow for horizontal scroll.
            .min_w(px(viewport_w))
            .child(gutter)
            // Trailing padding: breathing room past the last character when
            // scrolled fully right, and it counts toward the row's measured
            // width so the scrollbar covers it too.
            .child(
                div()
                    .flex_1()
                    .h(px(line_h))
                    .pr(px(48.))
                    .child(code_element),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.on_line_mouse_down(line_idx, event, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_context_menu(line_idx, event, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                this.on_line_mouse_move(line_idx, event, cx);
            }))
    }
}

impl Render for CodeEditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let line_count = self.buffer.line_count();
        // Wide enough for the largest line number plus the breakpoint dot,
        // so it doesn't resize while scrolling.
        let gutter_width = ((line_count.to_string().len().max(3) as f32) * 8.5 + 26.0) * self.zoom;
        self.gutter_width = gutter_width;
        // `uniform_list` measures ONE representative row to size the whole
        // list horizontally (item 0 by default). Point it at the longest
        // line, or a short first line would disable horizontal scrolling.
        // Byte length is a fine proxy with a monospace font.
        let widest_line_idx = (0..line_count)
            .max_by_key(|&line| {
                let r = self.buffer.line_range(line);
                r.end - r.start
            })
            .unwrap_or(0);

        let (viewport_w, viewport_h) = {
            let state = self.scroll_handle.0.borrow();
            let bounds = state.base_handle.bounds();
            (f32::from(bounds.size.width), f32::from(bounds.size.height))
        };
        // The pane's width is only known from the previous frame's layout;
        // after a resize, render once more so row widths (the clickable /
        // highlighted area) match the new width instead of lagging a frame.
        if (viewport_w - self.laid_out_width).abs() > 0.5 {
            self.laid_out_width = viewport_w;
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let (thumb, thumb_h) = {
            let state = self.scroll_handle.0.borrow();
            (
                scroll_thumb(&state.base_handle, viewport_h, theme.accent),
                scroll_thumb_horizontal(&state.base_handle, viewport_w, theme.accent),
            )
        };

        let find_bar = self.render_find_bar(cx);
        let this_link_valid = self.ctrl_link.as_ref().is_some_and(|l| l.valid);
        let hover = self.render_hover(viewport_w, viewport_h, theme, cx);
        let context_menu = self.render_context_menu(viewport_w, viewport_h, theme, cx);
        let dialog = self.render_dialog(viewport_w, theme, cx);
        let menu = self.completion.as_ref().map(|menu| {
            // Anchor the popup just under the caret, in this view's own
            // coordinates (scroll offset already applied).
            let (line, col) = self.buffer.cursor_line_col();
            let offset = self.scroll_handle.0.borrow().base_handle.offset();
            let line_start = self.buffer.line_starts()[line];
            let chars = self.buffer.text()[line_start..line_start + col].chars().count();
            let x = gutter_width + chars as f32 * f32::from(self.char_width) + f32::from(offset.x);
            let line_top = line as f32 * self.line_height() + f32::from(offset.y);
            let place = MenuPlacement {
                caret_x: x.max(gutter_width),
                line_top,
                line_bottom: line_top + self.line_height(),
                min_x: gutter_width,
                view_w: viewport_w,
                view_h: viewport_h,
                zoom: self.zoom,
            };
            render_completion_menu(menu, place, theme, cx)
        });

        div()
            .id("editor-scroll")
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("CodeEditor")
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                    this.drag_selecting = false;
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.last_pointer = None;
                    this.clear_ctrl_link(cx);
                }
                if !*hovered && (this.hover.is_some() || this.hover_pending.is_some()) {
                    this.clear_hover();
                    cx.notify();
                }
            }))
            // Pressing / releasing Ctrl without moving the mouse.
            .on_modifiers_changed(cx.listener(|this, event: &gpui::ModifiersChangedEvent, _window, cx| {
                let ctrl = event.modifiers.control || event.modifiers.platform;
                match this.last_pointer {
                    Some((line, position)) => this.update_ctrl_link(line, position, ctrl, cx),
                    None => this.clear_ctrl_link(cx),
                }
            }))
            // A hand over a navigable name while Ctrl is held.
            .when(this_link_valid, |d| d.cursor_pointer())
            .on_scroll_wheel(cx.listener(|this, _: &gpui::ScrollWheelEvent, _window, cx| {
                if this.hover.is_some() || this.hover_pending.is_some() {
                    this.clear_hover();
                    cx.notify();
                }
            }))
            .size_full()
            .min_w_0()
            .font_family(self.font_family.clone())
            .text_size(px(self.font_size * self.zoom))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                uniform_list(
                    "editor-lines",
                    line_count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|line_idx| {
                                this.render_line(line_idx, gutter_width, viewport_w, theme, cx)
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .with_width_from_item(Some(widest_line_idx))
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .children(thumb)
            .children(thumb_h)
            .children(hover)
            .children(menu)
            .children(find_bar)
            .children(context_menu)
            .children(dialog)
    }
}

/// Lower rank = more severe, so a plain `<` comparison picks the worse one.
fn severity_rank(severity: DiagnosticSeverity) -> u8 {
    match severity {
        DiagnosticSeverity::Error => 0,
        DiagnosticSeverity::Warning => 1,
        DiagnosticSeverity::Info => 2,
        DiagnosticSeverity::Hint => 3,
    }
}

fn color_for_kind(kind: TokenKind, theme: Theme) -> Rgba {
    match kind {
        TokenKind::Keyword => theme.syntax.keyword,
        TokenKind::String => theme.syntax.string,
        TokenKind::Comment => theme.syntax.comment,
        TokenKind::Number => theme.syntax.number,
        TokenKind::Type => theme.syntax.type_name,
        TokenKind::Plain => theme.syntax.punctuation,
    }
}

/// One line's syntax-highlighted text as a custom `Element`, so it can
/// shape its real glyph run: that gives exact caret/selection x-positions
/// (`x_for_index`) and exact click hit-testing (`closest_index_for_x`).
struct CodeLineElement {
    editor: Entity<CodeEditorView>,
    line_idx: usize,
    tokens: Vec<Token>,
    is_cursor_line: bool,
    cursor_byte_col: usize,
    /// Selected byte columns on this line, and whether the selection runs
    /// past the line's end (its newline is selected too).
    selection: Option<(usize, usize, bool)>,
    /// Find-bar matches on this line: `(start, end, is_current)` columns.
    matches: Vec<(usize, usize, bool)>,
    /// Diagnostic ranges on this line: `(start, end)` byte columns + color.
    underlines: Vec<(usize, usize, Rgba)>,
    /// Ctrl+hover link underline: `(start, end)` byte columns.
    link: Option<(usize, usize)>,
    line_height: f32,
    theme: Theme,
}

struct LinePaint {
    matches: Vec<PaintQuad>,
    selection: Option<PaintQuad>,
    caret: Option<PaintQuad>,
}

impl IntoElement for CodeLineElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CodeLineElement {
    /// The shaped text, computed once in `request_layout` (so its real
    /// width can be reported) and reused in `prepaint`/`paint`.
    type RequestLayoutState = ShapedLine;
    type PrepaintState = LinePaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text_style = window.text_style();
        let font = text_style.font();

        let mut full_text = String::new();
        let mut runs: Vec<TextRun> = Vec::new();
        for token in &self.tokens {
            if token.text.is_empty() {
                continue;
            }
            runs.push(TextRun {
                len: token.text.len(),
                font: font.clone(),
                color: color_for_kind(token.kind, self.theme).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            full_text.push_str(&token.text);
        }
        if runs.is_empty() {
            // An empty line still needs a paintable, clickable shape.
            full_text.push(' ');
            runs.push(TextRun {
                len: 1,
                font,
                color: self.theme.foreground.into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }

        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let shared_text: SharedString = full_text.into();
        let shaped = window
            .text_system()
            .shape_line(shared_text, font_size, &runs, None);

        // Report the real shaped width (not "100% of parent") so long lines
        // make the list horizontally scrollable.
        let mut style = Style::default();
        style.size.width = shaped.width.into();
        style.size.height = px(self.line_height).into();
        (window.request_layout(style, [], cx), shaped)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let height = bounds.bottom() - bounds.top();
        let selection = self.selection.map(|(start, end, past_eol)| {
            let x1 = shaped.x_for_index(start);
            let mut x2 = shaped.x_for_index(end);
            if past_eol {
                // Show the selected newline as a small extra block.
                x2 += px(7.);
            }
            fill(
                Bounds::new(point(bounds.left() + x1, bounds.top()), size(x2 - x1, height)),
                self.theme.accent.opacity(0.30),
            )
        });
        let caret = self.is_cursor_line.then(|| {
            let x = shaped.x_for_index(self.cursor_byte_col);
            fill(
                Bounds::new(point(bounds.left() + x, bounds.top()), size(px(2.), height)),
                self.theme.accent,
            )
        });
        let matches = self
            .matches
            .iter()
            .map(|&(start, end, current)| {
                let x1 = shaped.x_for_index(start);
                let x2 = shaped.x_for_index(end);
                let color = if current {
                    self.theme.warning.opacity(0.55)
                } else {
                    self.theme.warning.opacity(0.22)
                };
                fill(
                    Bounds::new(point(bounds.left() + x1, bounds.top()), size(x2 - x1, height)),
                    color,
                )
            })
            .collect();
        LinePaint {
            matches,
            selection,
            caret,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Match and selection highlights go under the text, the caret over.
        for quad in prepaint.matches.drain(..) {
            window.paint_quad(quad);
        }
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        shaped
            .paint(
                bounds.origin,
                px(self.line_height),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
        if let Some(caret) = prepaint.caret.take() {
            window.paint_quad(caret);
        }
        // Ctrl+hover: a solid link underline under the navigable name.
        if let Some((start, end)) = self.link {
            let x0 = bounds.left() + shaped.x_for_index(start);
            let x1 = bounds.left() + shaped.x_for_index(end.min(shaped.len));
            let y = bounds.top() + px(self.line_height - 2.5);
            window.paint_quad(fill(Bounds::new(point(x0, y), size(x1 - x0, px(1.5))), self.theme.accent));
        }
        // Squiggly underline under each diagnostic range.
        let step = 2.0_f32;
        let base = f32::from(bounds.top()) + self.line_height - 3.0;
        for &(start, end, color) in &self.underlines {
            let x0 = f32::from(bounds.left() + shaped.x_for_index(start));
            let x1 = f32::from(bounds.left() + shaped.x_for_index(end.min(shaped.len)));
            let mut x = x0;
            let mut up = false;
            while x < x1.max(x0 + 4.0) {
                let y = if up { base - 1.5 } else { base };
                window.paint_quad(fill(Bounds::new(point(px(x), px(y)), size(px(step), px(1.5))), color));
                x += step;
                up = !up;
            }
        }

        let line_idx = self.line_idx;
        let stored = shaped.clone();
        let text_len = shaped.len;
        let width = shaped.width;
        self.editor.update(cx, |editor, _cx| {
            editor.line_layouts.insert(line_idx, (stored, bounds));
            if text_len > 1 {
                editor.char_width = width / text_len as f32;
            }
        });
    }
}
