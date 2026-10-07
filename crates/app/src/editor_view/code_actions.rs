//! Language-server code actions in the editor: applying edits, imports
//! (automatic and on demand), quick fixes, rename, refactorings, code
//! generation (constructor, getters/setters, toString, equals/hashCode),
//! formatting — plus the right-click menu and the small step-by-step
//! dialogs those actions use to ask what exactly to do.
//!
//! The generation requests (`java/checkConstructorsStatus`,
//! `java/generateAccessors`, …) are jdtls extensions, the same ones the VS
//! Code Java extension uses; their responses are plain `WorkspaceEdit`s.

use std::ops::Range;
use std::path::Path;

use async_channel::Receiver;
use gpui::{
    Context, Entity, Focusable, MouseButton, MouseDownEvent, Subscription, Window, deferred, div,
    prelude::*, px,
};
use rji_lsp_client::{FileEdit, TextEdit, parse_code_actions, parse_text_edits, parse_workspace_edit, path_to_uri};
use rji_theme::Theme;
use serde_json::{Value, json};

use super::hover::{byte_col, utf16_len, word_range};
use super::{CodeEditorView, EditorCommand, EditorEvent};
use crate::text_input::{TextInput, TextInputEvent};

/// A fix or refactoring the user can pick: a title and the edits it makes.
#[derive(Clone)]
pub(super) struct Fix {
    pub title: String,
    pub edits: Vec<FileEdit>,
}

pub(super) struct ContextMenu {
    /// Top-left, in the editor's own coordinates.
    x: f32,
    y: f32,
}

#[derive(Clone)]
struct PickRow {
    label: String,
    detail: String,
    checked: bool,
    raw: Value,
}

#[derive(Clone)]
struct AccessorRow {
    field: String,
    type_name: String,
    getter: Option<bool>,
    setter: Option<bool>,
    raw: Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Constructor,
    ToString,
    EqualsHashCode,
    /// Override/Implement Methods.
    Override,
    /// Delegate Methods (rows are field + method pairs).
    Delegate,
}

/// One entry of the Source Action list.
#[derive(Clone)]
enum SourceItem {
    /// Opens a generate dialog (or runs an editor command).
    Command { title: &'static str, shortcut: Option<&'static str>, command: EditorCommand },
    /// A ready-made source action from the language server.
    Edit(Fix),
}

impl SourceItem {
    fn title(&self) -> &str {
        match self {
            SourceItem::Command { title, .. } => title,
            SourceItem::Edit(fix) => &fix.title,
        }
    }
}

enum DialogBody {
    Loading,
    Fields { kind: PickKind, rows: Vec<PickRow>, extra: Value },
    Accessors(Vec<AccessorRow>),
    Fixes { fixes: Vec<Fix>, selected: usize },
    Source { items: Vec<SourceItem>, selected: usize },
    Rename { input: Entity<TextInput>, position: (u32, u32), _subscription: Subscription },
    Message(String),
}

pub(super) struct ActionDialog {
    title: String,
    subtitle: String,
    body: DialogBody,
    /// `CodeActionParams` for the caret the dialog was opened at.
    context: Value,
    /// Document version the dialog's data was computed for.
    version: u64,
    busy: bool,
}

/// Right-click menu entries: `None` is a separator.
fn menu_entries(java: bool) -> Vec<Option<(&'static str, Option<&'static str>, EditorCommand, bool)>> {
    use EditorCommand as C;
    vec![
        Some(("Go to Definition", Some("F12"), C::GoToDefinition, java)),
        None,
        Some(("Cut", Some("Ctrl+X"), C::Cut, true)),
        Some(("Copy", Some("Ctrl+C"), C::Copy, true)),
        Some(("Paste", Some("Ctrl+V"), C::Paste, true)),
        None,
        Some(("Quick Fix…", Some("Ctrl+."), C::QuickFix, java)),
        Some(("Rename Symbol…", Some("F2"), C::Rename, java)),
        Some(("Refactor…", Some("Ctrl+Shift+R"), C::Refactor, java)),
        Some(("Source Action…", Some("Shift+Alt+S"), C::SourceAction, java)),
        None,
        Some(("Organize Imports", Some("Shift+Alt+O"), C::OrganizeImports, java)),
        Some(("Add Missing Imports", None, C::AddMissingImports, java)),
        Some(("Format Document", Some("Shift+Alt+F"), C::FormatDocument, java)),
        Some(("Toggle Comment", Some("Ctrl+/"), C::ToggleComment, true)),
        Some(("Select All", Some("Ctrl+A"), C::SelectAll, true)),
    ]
}

/// jdtls indents generated code with tabs; this editor uses spaces.
fn normalize_new_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\t', &" ".repeat(rji_editor::INDENT))
}

/// Whether an edit only adds/reorders `import` lines (safe to apply
/// without asking).
fn is_import_only(edits: &[FileEdit], own_path: &Path, current_text: &str, offset_of: impl Fn((u32, u32)) -> usize) -> bool {
    !edits.is_empty()
        && edits.iter().all(|file| {
            same_file(&file.path, own_path)
                && file.edits.iter().all(|e| {
                    let start = offset_of(e.start);
                    let end = offset_of(e.end).max(start);
                    let removed = current_text.get(start..end).unwrap_or("x");
                    let lines_ok = |s: &str| {
                        s.lines().all(|l| {
                            let t = l.trim();
                            t.is_empty() || (t.starts_with("import ") && t.ends_with(';'))
                        })
                    };
                    lines_ok(&e.new_text) && lines_ok(removed)
                })
        })
}

pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => cfg!(windows) && a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

impl CodeEditorView {
    // ---- positions and edits ---------------------------------------------

    /// Document offset of an LSP position (0-based line, UTF-16 column).
    pub(super) fn offset_for_lsp(&self, (line, character): (u32, u32)) -> usize {
        let line = line as usize;
        if line >= self.buffer.line_count() {
            return self.buffer.text().len();
        }
        let range = self.buffer.line_range(line);
        range.start + byte_col(&self.buffer.text()[range.clone()], character)
    }

    pub(super) fn lsp_position(&self, offset: usize) -> (u32, u32) {
        let (line, col) = self.buffer.line_col(offset);
        let start = self.buffer.line_starts()[line];
        (line as u32, utf16_len(&self.buffer.text()[start..start + col]) as u32)
    }

    /// Applies edits to this document as one undo step.
    pub fn apply_text_edits(&mut self, edits: &[TextEdit]) {
        if edits.is_empty() {
            return;
        }
        let converted = edits
            .iter()
            .map(|e| (self.offset_for_lsp(e.start)..self.offset_for_lsp(e.end), normalize_new_text(&e.new_text)))
            .collect();
        self.buffer.apply_edits(converted);
        self.after_edit();
    }

    /// Edits to this file are applied here; edits to other files (rename,
    /// "create class") go to the window, which owns the other tabs.
    pub(super) fn apply_workspace_edit(&mut self, files: Vec<FileEdit>, cx: &mut Context<Self>) {
        let mut others = Vec::new();
        for file in files {
            if same_file(&file.path, &self.path) {
                self.apply_text_edits(&file.edits);
            } else {
                others.push(file);
            }
        }
        if !others.is_empty() {
            cx.emit(EditorEvent::ExternalEdits(others));
        }
        self.scroll_cursor_into_view();
        cx.notify();
    }

    fn notice(&self, message: impl Into<String>, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Notice(message.into()));
    }

    // ---- requests ----------------------------------------------------------

    fn lsp_request(&self, method: &str, params: Value) -> Option<Receiver<Value>> {
        if !self.is_java() {
            return None;
        }
        self.lsp.borrow_mut().as_mut()?.request(method, params).ok()
    }

    /// `CodeActionParams` for `range` (document offsets).
    fn action_params(&self, range: Range<usize>, diagnostics: Vec<Value>, only: Option<&[&str]>) -> Value {
        let (sl, sc) = self.lsp_position(range.start);
        let (el, ec) = self.lsp_position(range.end);
        let mut context = json!({ "diagnostics": diagnostics });
        if let Some(only) = only {
            context["only"] = json!(only);
        }
        json!({
            "textDocument": { "uri": path_to_uri(&self.path) },
            "range": { "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } },
            "context": context,
        })
    }

    fn caret_range(&self) -> Range<usize> {
        self.buffer.selection().unwrap_or(self.buffer.cursor()..self.buffer.cursor())
    }

    /// Awaits `rx` and hands the response to `then` (if the view is alive).
    fn on_response(
        &self,
        rx: Receiver<Value>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.recv().await {
                this.update(cx, |this, cx| then(this, value, cx)).ok();
            }
        })
        .detach();
    }

    fn require_lsp(&self, cx: &mut Context<Self>) -> bool {
        if !self.is_java() {
            self.notice("This works in Java files", cx);
            return false;
        }
        if self.lsp.borrow().is_none() {
            self.notice("The Java language server isn't running yet", cx);
            return false;
        }
        true
    }

    // ---- imports and formatting -------------------------------------------

    pub(super) fn organize_imports(&mut self, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        let params = self.action_params(0..0, Vec::new(), Some(&["source.organizeImports"]));
        let Some(rx) = self.lsp_request("textDocument/codeAction", params) else { return };
        let version = self.edit_version;
        self.on_response(rx, cx, move |this, value, cx| {
            if this.edit_version != version {
                return;
            }
            match parse_code_actions(&value).into_iter().next() {
                Some(action) => {
                    this.apply_workspace_edit(action.edit, cx);
                    this.notice("Imports organized", cx);
                }
                None => this.notice("Imports are already organized", cx),
            }
        });
    }

    /// Adds every import jdtls can resolve unambiguously. `auto` (on new
    /// diagnostics) applies only edits that touch import lines, and stays
    /// quiet when there is nothing to add.
    pub(super) fn add_missing_imports(&mut self, auto: bool, cx: &mut Context<Self>) {
        if auto && (!self.is_java() || self.lsp.borrow().is_none()) {
            return;
        }
        if !auto && !self.require_lsp(cx) {
            return;
        }
        let params = self.action_params(0..0, Vec::new(), Some(&["source"]));
        let Some(rx) = self.lsp_request("textDocument/codeAction", params) else { return };
        let version = self.edit_version;
        self.on_response(rx, cx, move |this, value, cx| {
            if this.edit_version != version {
                // Stale: let the next diagnostics try again.
                if auto {
                    this.last_auto_import.clear();
                }
                return;
            }
            let action = parse_code_actions(&value)
                .into_iter()
                .find(|a| a.title.to_lowercase().contains("missing imports"));
            let Some(action) = action else {
                if !auto {
                    this.notice("No missing imports that can be added automatically", cx);
                }
                return;
            };
            let safe = is_import_only(&action.edit, &this.path, this.buffer.text(), |p| this.offset_for_lsp(p));
            if auto && !safe {
                return;
            }
            let added: Vec<String> = action
                .edit
                .iter()
                .flat_map(|f| f.edits.iter())
                .flat_map(|e| e.new_text.lines().map(str::trim).filter(|l| l.starts_with("import ")).map(str::to_string).collect::<Vec<_>>())
                .filter(|line| !this.buffer.text().contains(line.as_str()))
                .collect();
            this.apply_workspace_edit(action.edit, cx);
            if !added.is_empty() {
                this.notice(format!("Added {}", added.join(" ").replace("import ", "").replace(';', "")), cx);
            }
        });
    }

    /// Called with new diagnostics: if some types can't be resolved, try
    /// adding their imports (once per distinct set of names).
    pub(super) fn maybe_auto_import(&mut self, cx: &mut Context<Self>) {
        if !self.auto_import {
            return;
        }
        let mut unresolved: Vec<&str> = self
            .diagnostic_list
            .iter()
            .filter(|d| d.message.ends_with("cannot be resolved to a type"))
            .map(|d| d.message.trim_end_matches(" cannot be resolved to a type"))
            .collect();
        unresolved.sort_unstable();
        unresolved.dedup();
        let key = unresolved.join(",");
        if unresolved.is_empty() || self.last_auto_import == key {
            if unresolved.is_empty() {
                self.last_auto_import.clear();
            }
            return;
        }
        self.last_auto_import = key;
        self.add_missing_imports(true, cx);
    }

    pub(super) fn format_document(&mut self, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        let params = json!({
            "textDocument": { "uri": path_to_uri(&self.path) },
            "options": { "tabSize": rji_editor::INDENT, "insertSpaces": true },
        });
        let Some(rx) = self.lsp_request("textDocument/formatting", params) else { return };
        let version = self.edit_version;
        self.on_response(rx, cx, move |this, value, cx| {
            if this.edit_version != version {
                return;
            }
            let edits = parse_text_edits(value.get("result"));
            if edits.is_empty() {
                this.notice("Nothing to format (or the file has syntax errors)", cx);
            } else {
                this.apply_text_edits(&edits);
                this.notice("Document formatted", cx);
                cx.notify();
            }
        });
    }

    // ---- quick fixes ---------------------------------------------------------

    /// Quick fixes for `diagnostics` (raw LSP JSON) at `range`: the server's
    /// own fixes, plus one "Import 'X' (package)" per class named X when a
    /// type can't be resolved (found via completion, so ambiguous names
    /// like `List` offer every candidate).
    pub(super) fn fetch_fixes(
        &mut self,
        range: Range<usize>,
        diagnostics: Vec<Value>,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Vec<Fix>, &mut Context<Self>) + 'static,
    ) {
        if self.is_pom() {
            // pom.xml: version fixes the IDE computes itself.
            let _ = range;
            return self.pom_version_fixes(&diagnostics, cx, then);
        }
        let params = self.action_params(range, diagnostics.clone(), Some(&["quickfix"]));
        let actions_rx = self.lsp_request("textDocument/codeAction", params);
        // Unresolved type names and where they end (completion position).
        let unresolved: Vec<(String, (u32, u32))> = diagnostics
            .iter()
            .filter(|d| d["message"].as_str().is_some_and(|m| m.ends_with("cannot be resolved to a type")))
            .filter_map(|d| {
                let name = d["message"].as_str()?.trim_end_matches(" cannot be resolved to a type").to_string();
                let end = (d.pointer("/range/end/line")?.as_u64()? as u32, d.pointer("/range/end/character")?.as_u64()? as u32);
                Some((name, end))
            })
            .collect();
        let completions: Vec<(String, Receiver<Vec<rji_lsp_client::CompletionItem>>)> = unresolved
            .into_iter()
            .filter_map(|(name, (line, ch))| {
                let rx = self.lsp.borrow_mut().as_mut()?.request_completion(&self.path, line, ch).ok()?;
                Some((name, rx))
            })
            .collect();
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let mut fixes: Vec<Fix> = Vec::new();
            for (name, rx) in completions {
                if let Ok(items) = rx.recv().await {
                    for item in items {
                        let package = item.label.strip_prefix(&format!("{name} - ")).map(str::to_string);
                        if let Some(package) = package.filter(|_| !item.additional_edits.is_empty()) {
                            fixes.push(Fix {
                                title: format!("Import '{name}' ({package})"),
                                edits: vec![FileEdit { path: path.clone(), edits: item.additional_edits, create: false }],
                            });
                        }
                    }
                }
            }
            if let Some(rx) = actions_rx
                && let Ok(value) = rx.recv().await
            {
                fixes.extend(parse_code_actions(&value).into_iter().map(|a| Fix { title: a.title, edits: a.edit }));
            }
            let mut seen = std::collections::HashSet::new();
            fixes.retain(|f| seen.insert(f.title.clone()));
            this.update(cx, |this, cx| then(this, fixes, cx)).ok();
        })
        .detach();
    }

    fn open_dialog(&mut self, title: &str, subtitle: &str, context: Value, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.completion = None;
        self.clear_hover();
        self.dialog = Some(ActionDialog {
            title: title.into(),
            subtitle: subtitle.into(),
            body: DialogBody::Loading,
            context,
            version: self.edit_version,
            busy: false,
        });
        cx.notify();
    }

    fn set_dialog_body(&mut self, body: DialogBody, cx: &mut Context<Self>) {
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.body = body;
        }
        cx.notify();
    }

    pub(super) fn quick_fix(&mut self, cx: &mut Context<Self>) {
        if !self.is_pom() && !self.require_lsp(cx) {
            return;
        }
        let (line, _) = self.buffer.cursor_line_col();
        let diags: Vec<Value> = self
            .diagnostic_list
            .iter()
            .filter(|d| d.line as usize <= line && line <= d.end_line as usize)
            .map(|d| d.raw.clone())
            .collect();
        let range = if diags.is_empty() {
            self.caret_range()
        } else {
            let line_range = self.buffer.line_range(line);
            line_range.start..line_range.end
        };
        let subtitle = if diags.is_empty() { "No problems on this line" } else { "Problems on this line" };
        self.open_dialog("Quick Fix", subtitle, Value::Null, cx);
        self.fetch_fixes(range, diags, cx, |this, fixes, cx| {
            let body = if fixes.is_empty() {
                DialogBody::Message("No quick fixes here.".into())
            } else {
                DialogBody::Fixes { fixes, selected: 0 }
            };
            this.set_dialog_body(body, cx);
        });
    }

    /// Source Action… (Shift+Alt+S / Alt+Insert): the built-in generators,
    /// in VS Code's order, then any other source actions jdtls offers at the
    /// caret (e.g. "Change modifiers to final where possible").
    pub(super) fn source_action(&mut self, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        use EditorCommand as C;
        let built_in = |title, shortcut, command| SourceItem::Command { title, shortcut, command };
        let mut items = vec![
            built_in("Organize Imports", Some("Shift+Alt+O"), C::OrganizeImports),
            built_in("Generate Getters…", None, C::GenerateGetters),
            built_in("Generate Setters…", None, C::GenerateSetters),
            built_in("Generate Getters and Setters…", None, C::GenerateAccessors),
            built_in("Generate Constructors…", None, C::GenerateConstructor),
            built_in("Generate hashCode() and equals()…", None, C::GenerateEqualsHashCode),
            built_in("Generate toString()…", None, C::GenerateToString),
            built_in("Override/Implement Methods…", None, C::GenerateOverrides),
            built_in("Generate Delegate Methods…", None, C::GenerateDelegates),
        ];
        let caret = self.caret_range();
        let params = self.action_params(caret, Vec::new(), Some(&["source"]));
        let rx = self.lsp_request("textDocument/codeAction", params);
        self.open_dialog("Source Action", "↑↓ to choose, Enter to run", Value::Null, cx);
        let Some(rx) = rx else {
            self.set_dialog_body(DialogBody::Source { items, selected: 0 }, cx);
            return;
        };
        self.on_response(rx, cx, move |this, value, cx| {
            // Skip what the built-in entries already cover (with a dialog).
            let covered = ["getter", "setter", "organize imports", "tostring", "hashcode", "constructor"];
            for action in parse_code_actions(&value) {
                let lower = action.title.to_lowercase();
                if covered.iter().any(|c| lower.contains(c)) {
                    continue;
                }
                items.push(SourceItem::Edit(Fix { title: action.title, edits: action.edit }));
            }
            this.set_dialog_body(DialogBody::Source { items, selected: 0 }, cx);
        });
    }

    fn run_source_item(&mut self, item: SourceItem, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        match item {
            SourceItem::Command { command, .. } => self.command(command, window, cx),
            SourceItem::Edit(fix) => self.apply_fix(fix, cx),
        }
    }

    pub(super) fn refactor(&mut self, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        let range = self.caret_range();
        let params = self.action_params(range.clone(), Vec::new(), Some(&["refactor"]));
        let Some(rx) = self.lsp_request("textDocument/codeAction", params) else { return };
        let subtitle = if range.is_empty() {
            "Tip: select an expression or statements for Extract refactorings"
        } else {
            "For the selection"
        };
        self.open_dialog("Refactor", subtitle, Value::Null, cx);
        self.on_response(rx, cx, |this, value, cx| {
            let fixes: Vec<Fix> = parse_code_actions(&value).into_iter().map(|a| Fix { title: a.title, edits: a.edit }).collect();
            let body = if fixes.is_empty() {
                DialogBody::Message("No refactorings available here.".into())
            } else {
                DialogBody::Fixes { fixes, selected: 0 }
            };
            this.set_dialog_body(body, cx);
        });
    }

    pub(super) fn apply_fix(&mut self, fix: Fix, cx: &mut Context<Self>) {
        self.dialog = None;
        self.clear_hover();
        self.apply_workspace_edit(fix.edits, cx);
    }

    // ---- rename --------------------------------------------------------------

    pub(super) fn rename_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        let cursor = self.buffer.cursor();
        let (line, col) = self.buffer.line_col(cursor);
        let line_text = self.buffer.line_text(line).to_string();
        let mut word = word_range(&line_text, col.min(line_text.len().saturating_sub(1)));
        if word.is_empty() && col > 0 {
            word = word_range(&line_text, col - 1);
        }
        if word.is_empty() {
            self.notice("Put the caret on a name to rename it", cx);
            return;
        }
        let name = line_text[word.clone()].to_string();
        let start = self.buffer.line_starts()[line] + word.start;
        let position = self.lsp_position(start);
        let theme = self.theme;
        let input = cx.new(|cx| {
            let mut input = TextInput::new("new name", theme, cx);
            input.set_text(&name, cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Submit { .. } => this.confirm_dialog(window, cx),
            TextInputEvent::Cancel => this.close_dialog(window, cx),
            TextInputEvent::Changed => {}
        });
        let focus = input.focus_handle(cx);
        self.open_dialog("Rename Symbol", &format!("Rename '{name}' everywhere it's used"), Value::Null, cx);
        self.set_dialog_body(DialogBody::Rename { input, position, _subscription: subscription }, cx);
        window.focus(&focus, cx);
    }

    // ---- code generation -----------------------------------------------------

    pub(super) fn generate(&mut self, command: EditorCommand, cx: &mut Context<Self>) {
        if !self.require_lsp(cx) {
            return;
        }
        let range = self.caret_range();
        let context = self.action_params(range.start..range.start, Vec::new(), None);
        let (title, method, accessor_kind) = match command {
            EditorCommand::GenerateConstructor => ("Generate Constructor", "java/checkConstructorsStatus", None),
            EditorCommand::GenerateAccessors => ("Generate Getters and Setters", "java/resolveUnimplementedAccessors", Some(2)),
            EditorCommand::GenerateGetters => ("Generate Getters", "java/resolveUnimplementedAccessors", Some(0)),
            EditorCommand::GenerateSetters => ("Generate Setters", "java/resolveUnimplementedAccessors", Some(1)),
            EditorCommand::GenerateToString => ("Generate toString()", "java/checkToStringStatus", None),
            EditorCommand::GenerateEqualsHashCode => ("Generate equals() and hashCode()", "java/checkHashCodeEqualsStatus", None),
            EditorCommand::GenerateOverrides => ("Override/Implement Methods", "java/listOverridableMethods", None),
            EditorCommand::GenerateDelegates => ("Generate Delegate Methods", "java/checkDelegateMethodsStatus", None),
            _ => return,
        };
        let mut params = context.clone();
        if let Some(kind) = accessor_kind {
            params["kind"] = json!(kind);
        }
        let Some(rx) = self.lsp_request(method, params) else { return };
        self.open_dialog(title, "Inserted at the caret, inside the class.", context, cx);
        self.on_response(rx, cx, move |this, value, cx| {
            if let Some(error) = value.get("error") {
                let message = error["message"].as_str().unwrap_or("failed");
                this.set_dialog_body(DialogBody::Message(format!("The language server couldn't do this here ({message}). Put the caret inside a class body.")), cx);
                return;
            }
            let result = value.get("result").cloned().unwrap_or(Value::Null);
            let body = match command {
                EditorCommand::GenerateConstructor => fields_body(PickKind::Constructor, &result, true),
                EditorCommand::GenerateToString => fields_body(PickKind::ToString, &result, false),
                EditorCommand::GenerateEqualsHashCode => fields_body(PickKind::EqualsHashCode, &result, true),
                EditorCommand::GenerateOverrides => overrides_body(&result),
                EditorCommand::GenerateDelegates => delegates_body(&result),
                _ => accessors_body(&result),
            };
            if let (Some(dialog), DialogBody::Fields { kind: PickKind::EqualsHashCode, .. }) = (this.dialog.as_mut(), &body)
                && result["existingMethods"].as_array().is_some_and(|m| !m.is_empty())
            {
                dialog.subtitle = "equals()/hashCode() already exist and will be regenerated.".into();
            }
            if let (Some(dialog), DialogBody::Fields { kind: PickKind::ToString, .. }) = (this.dialog.as_mut(), &body)
                && result["exists"].as_bool() == Some(true)
            {
                dialog.subtitle = "toString() already exists; generating adds another — remove the old one first.".into();
            }
            this.set_dialog_body(body, cx);
        });
    }

    // ---- dialog interaction ----------------------------------------------------

    pub(super) fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn toggle_row(&mut self, index: usize, column: u8, cx: &mut Context<Self>) {
        match self.dialog.as_mut().map(|d| &mut d.body) {
            Some(DialogBody::Fields { rows, .. }) => {
                if let Some(row) = rows.get_mut(index) {
                    row.checked = !row.checked;
                }
            }
            Some(DialogBody::Accessors(rows)) => {
                if let Some(row) = rows.get_mut(index) {
                    let cell = if column == 0 { &mut row.getter } else { &mut row.setter };
                    if let Some(value) = cell {
                        *value = !*value;
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn set_all(&mut self, value: bool, cx: &mut Context<Self>) {
        match self.dialog.as_mut().map(|d| &mut d.body) {
            Some(DialogBody::Fields { rows, .. }) => rows.iter_mut().for_each(|r| r.checked = value),
            Some(DialogBody::Accessors(rows)) => rows.iter_mut().for_each(|r| {
                if let Some(g) = r.getter.as_mut() {
                    *g = value;
                }
                if let Some(s) = r.setter.as_mut() {
                    *s = value;
                }
            }),
            _ => {}
        }
        cx.notify();
    }

    /// Keyboard handling while a dialog is open. Returns true if handled.
    pub(super) fn dialog_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.dialog.as_mut() else { return false };
        match key {
            "escape" => self.close_dialog(window, cx),
            "enter" => self.confirm_dialog(window, cx),
            "up" | "down" => {
                let list = match &mut dialog.body {
                    DialogBody::Fixes { fixes, selected } => Some((fixes.len(), selected)),
                    DialogBody::Source { items, selected } => Some((items.len(), selected)),
                    _ => None,
                };
                if let Some((len, selected)) = list {
                    let n = len.max(1);
                    *selected = if key == "down" { (*selected + 1) % n } else { (*selected + n - 1) % n };
                    cx.notify();
                }
            }
            _ => {}
        }
        true
    }

    /// OK / Enter: runs the dialog's action.
    pub(super) fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.as_mut() else { return };
        if dialog.busy {
            return;
        }
        let context = dialog.context.clone();
        let version = dialog.version;
        let request: Option<(&str, Value)> = match &dialog.body {
            DialogBody::Loading => None,
            DialogBody::Message(_) => {
                self.close_dialog(window, cx);
                return;
            }
            DialogBody::Fixes { fixes, selected } => {
                let fix = fixes.get(*selected).cloned();
                window.focus(&self.focus_handle, cx);
                if let Some(fix) = fix {
                    self.apply_fix(fix, cx);
                }
                return;
            }
            DialogBody::Rename { input, position, .. } => {
                let new_name = input.read(cx).text().trim().to_string();
                if new_name.is_empty() || !new_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                    dialog.subtitle = "Enter a valid Java identifier".into();
                    cx.notify();
                    return;
                }
                Some((
                    "textDocument/rename",
                    json!({
                        "textDocument": { "uri": path_to_uri(&self.path) },
                        "position": { "line": position.0, "character": position.1 },
                        "newName": new_name,
                    }),
                ))
            }
            DialogBody::Fields { kind, rows, extra } => {
                let chosen: Vec<Value> = rows.iter().filter(|r| r.checked).map(|r| r.raw.clone()).collect();
                match kind {
                    PickKind::Constructor => Some((
                        "java/generateConstructors",
                        json!({ "context": context, "constructors": extra, "fields": chosen }),
                    )),
                    PickKind::ToString => Some(("java/generateToString", json!({ "context": context, "fields": chosen }))),
                    PickKind::EqualsHashCode => {
                        if chosen.is_empty() {
                            dialog.subtitle = "Pick at least one field".into();
                            cx.notify();
                            return;
                        }
                        Some((
                            "java/generateHashCodeEquals",
                            json!({ "context": context, "fields": chosen, "regenerate": true }),
                        ))
                    }
                    PickKind::Override | PickKind::Delegate => {
                        if chosen.is_empty() {
                            dialog.subtitle = "Pick at least one method".into();
                            cx.notify();
                            return;
                        }
                        if *kind == PickKind::Override {
                            Some(("java/addOverridableMethods", json!({ "context": context, "overridableMethods": chosen })))
                        } else {
                            Some(("java/generateDelegateMethods", json!({ "context": context, "delegateEntries": chosen })))
                        }
                    }
                }
            }
            DialogBody::Source { items, selected } => {
                let item = items.get(*selected).cloned();
                self.dialog = None;
                if let Some(item) = item {
                    self.run_source_item(item, window, cx);
                }
                return;
            }
            DialogBody::Accessors(rows) => {
                let accessors: Vec<Value> = rows
                    .iter()
                    .filter(|r| r.getter == Some(true) || r.setter == Some(true))
                    .map(|r| {
                        let mut raw = r.raw.clone();
                        raw["generateGetter"] = json!(r.getter == Some(true));
                        raw["generateSetter"] = json!(r.setter == Some(true));
                        raw
                    })
                    .collect();
                if accessors.is_empty() {
                    dialog.subtitle = "Pick at least one getter or setter".into();
                    cx.notify();
                    return;
                }
                Some(("java/generateAccessors", json!({ "context": context, "accessors": accessors })))
            }
        };
        let Some((method, params)) = request else { return };
        let Some(rx) = self.lsp_request(method, params) else { return };
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.busy = true;
        }
        cx.notify();
        let is_rename = method == "textDocument/rename";
        self.on_response(rx, cx, move |this, value, cx| {
            if !is_rename && this.edit_version != version {
                this.set_dialog_body(DialogBody::Message("The file changed meanwhile — please try again.".into()), cx);
                if let Some(d) = this.dialog.as_mut() {
                    d.busy = false;
                }
                return;
            }
            if let Some(error) = value.get("error") {
                let message = error["message"].as_str().unwrap_or("failed").to_string();
                this.set_dialog_body(DialogBody::Message(message), cx);
                if let Some(d) = this.dialog.as_mut() {
                    d.busy = false;
                }
                return;
            }
            let edits = value.get("result").map(parse_workspace_edit).unwrap_or_default();
            this.dialog = None;
            if edits.is_empty() {
                this.notice("Nothing to change", cx);
            } else {
                let files = edits.len();
                this.apply_workspace_edit(edits, cx);
                if is_rename && files > 1 {
                    this.notice(format!("Renamed in {files} files"), cx);
                }
            }
            cx.notify();
        });
        window.focus(&self.focus_handle, cx);
    }

    // ---- context menu ------------------------------------------------------

    pub(super) fn open_context_menu(&mut self, line: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        self.completion = None;
        self.clear_hover();
        self.dialog = None;
        // Right-click outside the selection moves the caret there first.
        let offset = self.offset_at(line, event.position);
        if !self.buffer.selection().is_some_and(|s| s.contains(&offset)) {
            self.buffer.set_cursor(offset, false);
        }
        let origin = self.scroll_handle.0.borrow().base_handle.bounds().origin;
        self.context_menu = Some(ContextMenu {
            x: f32::from(event.position.x - origin.x),
            y: f32::from(event.position.y - origin.y),
        });
        cx.notify();
    }

    /// Alt+Insert / Shift+F10: the same menu, at the caret.
    pub(super) fn open_context_menu_at_caret(&mut self, cx: &mut Context<Self>) {
        let (line, _) = self.buffer.cursor_line_col();
        let offset = self.scroll_handle.0.borrow().base_handle.offset();
        self.context_menu = Some(ContextMenu {
            x: self.gutter_width + 40.0,
            y: (line as f32 + 1.0) * self.line_height() + f32::from(offset.y),
        });
        cx.notify();
    }

    pub(super) fn render_context_menu(&self, view_w: f32, view_h: f32, theme: Theme, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let menu = self.context_menu.as_ref()?;
        let java = self.is_java() && self.lsp.borrow().is_some();
        let entries = menu_entries(java);
        let z = self.zoom;
        let row_h = 26.0 * z;
        let width = 300.0 * z;
        let height = entries.iter().map(|e| if e.is_some() { row_h } else { 9.0 }).sum::<f32>() + 10.0;
        let left = menu.x.min(view_w - width - 6.0).max(4.0);
        let top = if menu.y + height > view_h { (menu.y - height).max(4.0) } else { menu.y };

        let rows = entries.into_iter().enumerate().map(|(i, entry)| match entry {
            None => div().my(px(4.)).h(px(1.)).bg(theme.border).into_any_element(),
            Some((label, shortcut, command, enabled)) => div()
                .id(("editor-ctx", i))
                .h(px(row_h))
                .px_3()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_4()
                .rounded_sm()
                .text_color(if enabled { theme.foreground } else { theme.foreground_muted.opacity(0.5) })
                .when(enabled, |row| {
                    row.cursor_pointer()
                        .hover(|s| s.bg(theme.accent.opacity(0.2)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.context_menu = None;
                                this.command(command, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                })
                .child(label)
                .children(shortcut.map(|s| div().text_size(px(11. * z)).text_color(theme.foreground_muted).child(s)))
                .into_any_element(),
        });

        Some(
            deferred(
                div()
                    .id("editor-context-menu")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .p_1()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .shadow_lg()
                    .text_size(px(13. * z))
                    .font_family(".SystemUIFont")
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.context_menu = None;
                        cx.notify();
                    }))
                    .children(rows),
            )
            .with_priority(2),
        )
    }

    // ---- dialog rendering ------------------------------------------------------

    pub(super) fn render_dialog(&self, view_w: f32, theme: Theme, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let dialog = self.dialog.as_ref()?;
        let z = self.zoom;
        let width = (460.0 * z).min(view_w - 24.0).max(260.0);
        let checkbox = |checked: Option<bool>| {
            let (glyph, color) = match checked {
                Some(true) => ("☑", theme.accent),
                Some(false) => ("☐", theme.foreground_muted),
                None => ("–", theme.foreground_muted.opacity(0.5)),
            };
            div().w(px(18. * z)).flex_shrink_0().text_color(color).child(glyph)
        };
        let small_button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.5 * z))
                .text_color(theme.accent)
                .hover(|s| s.bg(theme.accent.opacity(0.14)))
                .child(label)
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if primary { theme.accent } else { theme.border })
                .text_color(if primary { theme.accent } else { theme.foreground })
                .hover(|s| s.bg(theme.accent.opacity(0.15)))
                .child(label)
        };

        let mut body = div().flex().flex_col().gap_1().max_h(px(320. * z)).id("dialog-body").overflow_y_scroll();
        let mut show_ok = true;
        let mut select_all = false;
        match &dialog.body {
            DialogBody::Loading => {
                show_ok = false;
                body = body.child(div().text_color(theme.foreground_muted).child("Asking the language server…"));
            }
            DialogBody::Message(text) => {
                body = body.child(div().text_color(theme.foreground).child(text.clone()));
            }
            DialogBody::Rename { input, .. } => {
                body = body.child(input.clone());
            }
            DialogBody::Fields { rows, .. } => {
                select_all = !rows.is_empty();
                if rows.is_empty() {
                    body = body.child(div().text_color(theme.foreground_muted).child("No fields — the result will use none."));
                }
                for (i, row) in rows.iter().enumerate() {
                    body = body.child(
                        div()
                            .id(("dialog-row", i))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.accent.opacity(0.1)))
                            .child(checkbox(Some(row.checked)))
                            .child(div().text_color(theme.foreground).child(row.label.clone()))
                            .child(div().text_size(px(11.5 * z)).text_color(theme.foreground_muted).child(row.detail.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_row(i, 0, cx))),
                    );
                }
            }
            DialogBody::Accessors(rows) => {
                select_all = !rows.is_empty();
                if rows.is_empty() {
                    show_ok = false;
                    body = body.child(div().text_color(theme.foreground_muted).child("Every field already has its getter/setter."));
                } else {
                    body = body.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .text_size(px(11. * z))
                            .text_color(theme.foreground_muted)
                            .child(div().w(px(60. * z)).child("get"))
                            .child(div().w(px(60. * z)).child("set"))
                            .child("field"),
                    );
                }
                for (i, row) in rows.iter().enumerate() {
                    let cell = |id: (&'static str, usize), value: Option<bool>, column: u8| {
                        div()
                            .id(id)
                            .w(px(60. * z))
                            .when(value.is_some(), |c| {
                                c.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.toggle_row(i, column, cx)))
                            })
                            .child(checkbox(value))
                    };
                    body = body.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .child(cell(("acc-get", i), row.getter, 0))
                            .child(cell(("acc-set", i), row.setter, 1))
                            .child(div().text_color(theme.foreground).child(row.field.clone()))
                            .child(div().text_size(px(11.5 * z)).text_color(theme.foreground_muted).child(row.type_name.clone())),
                    );
                }
            }
            DialogBody::Fixes { fixes, selected } => {
                for (i, fix) in fixes.iter().enumerate() {
                    let fix_clone = fix.clone();
                    body = body.child(
                        div()
                            .id(("dialog-fix", i))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(theme.foreground)
                            .when(i == *selected, |r| r.bg(theme.accent.opacity(0.25)))
                            .hover(|s| s.bg(theme.accent.opacity(0.14)))
                            .child(format!("💡 {}", fix.title))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.focus_handle, cx);
                                this.apply_fix(fix_clone.clone(), cx);
                            })),
                    );
                }
            }
            DialogBody::Source { items, selected } => {
                for (i, item) in items.iter().enumerate() {
                    let item_clone = item.clone();
                    let shortcut = match item {
                        SourceItem::Command { shortcut, .. } => *shortcut,
                        SourceItem::Edit(_) => None,
                    };
                    body = body.child(
                        div()
                            .id(("dialog-source", i))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_4()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(theme.foreground)
                            .when(i == *selected, |r| r.bg(theme.accent.opacity(0.25)))
                            .hover(|s| s.bg(theme.accent.opacity(0.14)))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(div().text_color(theme.foreground_muted).child("▤"))
                                    .child(item.title().to_string()),
                            )
                            .children(shortcut.map(|s| div().text_size(px(11. * z)).text_color(theme.foreground_muted).child(s)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.dialog = None;
                                this.run_source_item(item_clone.clone(), window, cx);
                            })),
                    );
                }
            }
        }

        let ok_label = match &dialog.body {
            DialogBody::Message(_) => "Close",
            DialogBody::Fixes { .. } => "Apply",
            DialogBody::Source { .. } => "Run",
            DialogBody::Rename { .. } => "Rename",
            _ if dialog.busy => "Working…",
            _ => "Generate",
        };
        Some(
            deferred(
                div()
                    .id("action-dialog")
                    .absolute()
                    .top(px(24.))
                    .left(px(((view_w - width) / 2.0).max(8.0)))
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.accent.opacity(0.5))
                    .rounded_lg()
                    .shadow_lg()
                    .text_size(px(13. * z))
                    .font_family(".SystemUIFont")
                    .text_color(theme.foreground)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_size(px(15. * z)).child(dialog.title.clone()))
                    .child(div().text_size(px(11.5 * z)).text_color(theme.foreground_muted).child(dialog.subtitle.clone()))
                    .when(select_all, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_1()
                                .child(small_button("dialog-all", "Select all").on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))))
                                .child(small_button("dialog-none", "Select none").on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx)))),
                        )
                    })
                    .child(body)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(button("dialog-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))))
                            .when(show_ok && !matches!(dialog.body, DialogBody::Message(_)), |row| {
                                row.child(button("dialog-ok", ok_label, true).on_click(cx.listener(|this, _, window, cx| this.confirm_dialog(window, cx))))
                            }),
                    ),
            )
            .with_priority(3),
        )
    }

    pub(super) fn has_overlay(&self) -> bool {
        self.dialog.is_some() || self.context_menu.is_some()
    }
}

/// Field picker for constructor / toString / equals+hashCode. `fields_only`
/// hides the methods jdtls offers for toString.
fn fields_body(kind: PickKind, result: &Value, fields_only: bool) -> DialogBody {
    let rows: Vec<PickRow> = result["fields"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter(|f| !fields_only || f["isField"].as_bool() != Some(false))
                .filter(|f| kind != PickKind::ToString || f["isField"].as_bool() == Some(true))
                .map(|f| PickRow {
                    label: f["name"].as_str().unwrap_or("?").to_string(),
                    detail: f["type"].as_str().unwrap_or("").to_string(),
                    // Constructors start with every field ticked (the
                    // server's own preselection is just the field at the
                    // caret); the others follow the server.
                    checked: kind == PickKind::Constructor || f["isSelected"].as_bool().unwrap_or(true) || kind == PickKind::EqualsHashCode,
                    raw: f.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    // Constructors: delegate to the superclass's first constructor (the
    // no-arg one for a plain class).
    let extra = result["constructors"].as_array().and_then(|c| c.first().cloned()).map(|c| json!([c])).unwrap_or(json!([]));
    DialogBody::Fields { kind, rows, extra }
}

/// `name(String, int)` from a jdtls method entry.
fn method_signature(method: &Value) -> String {
    let params: Vec<&str> = method["parameters"]
        .as_array()
        .map(|p| p.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    format!("{}({})", method["name"].as_str().unwrap_or("?"), params.join(", "))
}

/// Override/Implement: every overridable method, grouped by the type that
/// declares it; abstract ones the class must implement start ticked.
fn overrides_body(result: &Value) -> DialogBody {
    let mut rows: Vec<PickRow> = result["methods"]
        .as_array()
        .map(|methods| {
            methods
                .iter()
                .map(|m| {
                    let unimplemented = m["unimplemented"].as_bool() == Some(true);
                    PickRow {
                        label: method_signature(m),
                        detail: format!(
                            "{}{}",
                            m["declaringClass"].as_str().unwrap_or(""),
                            if unimplemented { " · must implement" } else { "" }
                        ),
                        checked: unimplemented,
                        raw: m.clone(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    // Must-implement first, then by declaring type.
    rows.sort_by(|a, b| b.checked.cmp(&a.checked).then(a.detail.cmp(&b.detail)));
    DialogBody::Fields { kind: PickKind::Override, rows, extra: Value::Null }
}

/// Delegate Methods: one row per (field, method) pair.
fn delegates_body(result: &Value) -> DialogBody {
    let mut rows = Vec::new();
    for entry in result["delegateFields"].as_array().into_iter().flatten() {
        let field = &entry["field"];
        let field_name = field["name"].as_str().unwrap_or("?");
        for method in entry["delegateMethods"].as_array().into_iter().flatten() {
            rows.push(PickRow {
                label: format!("{field_name}.{}", method_signature(method)),
                detail: field["type"].as_str().unwrap_or("").to_string(),
                checked: false,
                raw: json!({ "field": field, "delegateMethod": method }),
            });
        }
    }
    DialogBody::Fields { kind: PickKind::Delegate, rows, extra: Value::Null }
}

fn accessors_body(result: &Value) -> DialogBody {
    let rows = result
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|a| AccessorRow {
                    field: a["fieldName"].as_str().unwrap_or("?").to_string(),
                    type_name: format!(
                        "{}{}",
                        if a["isStatic"].as_bool() == Some(true) { "static " } else { "" },
                        a["typeName"].as_str().unwrap_or("")
                    ),
                    getter: a["generateGetter"].as_bool().filter(|g| *g).map(|_| true),
                    setter: a["generateSetter"].as_bool().filter(|s| *s).map(|_| true),
                    raw: a.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    DialogBody::Accessors(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_only_detection() {
        let text = "package a;\n\nclass A {}\n";
        let path = Path::new("A.java");
        let offset = |(line, ch): (u32, u32)| {
            let starts = [0usize, 11, 12];
            starts[line as usize] + ch as usize
        };
        let import = vec![FileEdit {
            path: path.into(),
            edits: vec![TextEdit { start: (0, 10), end: (2, 0), new_text: "\n\nimport java.util.HashMap;\n\n".into() }],
            create: false,
        }];
        assert!(is_import_only(&import, path, text, offset));
        let code = vec![FileEdit {
            path: path.into(),
            edits: vec![TextEdit { start: (2, 0), end: (2, 0), new_text: "int x;".into() }],
            create: false,
        }];
        assert!(!is_import_only(&code, path, text, offset));
    }

    #[test]
    fn tabs_become_spaces() {
        assert_eq!(normalize_new_text("a\r\n\tb\t\tc"), "a\n    b        c");
    }
}
