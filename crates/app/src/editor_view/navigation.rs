//! Go to Definition (F12 / Ctrl+Click). jdtls answers with a location in
//! the project (`file://`) or inside the JDK / a library (`jdt://`); this
//! file handles the request and same-file jumps, and asks the window to
//! open anything else (see `EditorEvent::OpenLocation`).

use gpui::Context;
use rji_lsp_client::path_to_uri;
use serde_json::{Value, json};

use super::{CodeEditorView, EditorEvent};

/// A place in some document: LSP URI plus 0-based line / UTF-16 column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

/// First target of a definition response. Accepts every shape the spec
/// allows: `Location`, `Location[]`, `LocationLink[]`, or null.
pub fn parse_definition(response: &Value) -> Option<Location> {
    let result = response.get("result")?;
    let first = match result {
        Value::Array(items) => items.first()?,
        Value::Object(_) => result,
        _ => return None,
    };
    let (uri, range) = if let Some(uri) = first.get("targetUri") {
        (uri, first.get("targetSelectionRange").or_else(|| first.get("targetRange"))?)
    } else {
        (first.get("uri")?, first.get("range")?)
    };
    Some(Location {
        uri: uri.as_str()?.to_string(),
        line: range.pointer("/start/line")?.as_u64()? as u32,
        character: range.pointer("/start/character")?.as_u64()? as u32,
    })
}

impl CodeEditorView {
    /// Moves the caret to an LSP position and centers it.
    pub fn reveal_position(&mut self, line: u32, character: u32) {
        let offset = self.offset_for_lsp((line, character));
        self.buffer.set_cursor(offset, false);
        self.completion = None;
        let line = (line as usize).min(self.buffer.line_count() - 1);
        self.scroll_handle.scroll_to_item_strict(line, gpui::ScrollStrategy::Center);
    }

    /// Asks jdtls where the symbol at `offset` is defined, then goes there.
    pub(super) fn go_to_definition(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.read_only {
            cx.emit(EditorEvent::Notice("Go to Definition isn't available inside library sources yet".into()));
            return;
        }
        if !self.is_java() {
            cx.emit(EditorEvent::Notice("Go to Definition works in Java files".into()));
            return;
        }
        let (line, character) = self.lsp_position(offset);
        let params = json!({
            "textDocument": { "uri": path_to_uri(&self.path) },
            "position": { "line": line, "character": character },
        });
        let request = self.lsp.borrow_mut().as_mut().and_then(|client| client.request("textDocument/definition", params).ok());
        let Some(rx) = request else {
            cx.emit(EditorEvent::Notice("The Java language server isn't running yet".into()));
            return;
        };
        let version = self.edit_version;
        let own_uri = path_to_uri(&self.path);
        cx.spawn(async move |this, cx| {
            let Ok(response) = rx.recv().await else { return };
            this.update(cx, |this, cx| {
                if this.edit_version != version {
                    return; // typed meanwhile: the answer may point at stale text
                }
                match parse_definition(&response) {
                    None => cx.emit(EditorEvent::Notice("No definition found here".into())),
                    Some(location) if location.uri == own_uri => {
                        this.reveal_position(location.line, location.character);
                        cx.notify();
                    }
                    Some(location) => cx.emit(EditorEvent::OpenLocation(location)),
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_definition_shapes() {
        let location = json!({ "result": [{ "uri": "file:///a/App.java", "range": { "start": { "line": 4, "character": 25 }, "end": { "line": 4, "character": 33 } } }] });
        assert_eq!(parse_definition(&location), Some(Location { uri: "file:///a/App.java".into(), line: 4, character: 25 }));
        let single = json!({ "result": { "uri": "jdt://contents/x", "range": { "start": { "line": 1, "character": 2 } } } });
        assert_eq!(parse_definition(&single).unwrap().uri, "jdt://contents/x");
        let link = json!({ "result": [{ "targetUri": "file:///b.java", "targetRange": { "start": { "line": 0, "character": 0 } }, "targetSelectionRange": { "start": { "line": 9, "character": 4 } } }] });
        assert_eq!(parse_definition(&link).unwrap().line, 9);
        assert_eq!(parse_definition(&json!({ "result": null })), None);
        assert_eq!(parse_definition(&json!({ "result": [] })), None);
    }
}

/// The name under the mouse while Ctrl (Cmd on macOS) is held: underlined
/// like a link once jdtls confirms it has a definition.
pub(super) struct CtrlLink {
    /// Document byte range of the name.
    pub range: std::ops::Range<usize>,
    /// Edit version the answer is for.
    version: u64,
    /// jdtls found a definition for it.
    pub valid: bool,
}

impl CodeEditorView {
    /// Called on mouse moves and Ctrl presses/releases with the pointer
    /// over `line`: shows, moves or hides the link underline.
    pub(super) fn update_ctrl_link(
        &mut self,
        line: usize,
        position: gpui::Point<gpui::Pixels>,
        ctrl: bool,
        cx: &mut Context<Self>,
    ) {
        let usable = ctrl && self.is_java() && !self.read_only && self.lsp.borrow().is_some();
        let word = usable.then(|| self.word_at_point(line, position)).flatten();
        let Some(range) = word else {
            self.clear_ctrl_link(cx);
            return;
        };
        if self.ctrl_link.as_ref().is_some_and(|l| l.range == range && l.version == self.edit_version) {
            return;
        }
        self.ctrl_link = Some(CtrlLink { range: range.clone(), version: self.edit_version, valid: false });
        self.ctrl_link_generation += 1;
        let generation = self.ctrl_link_generation;
        let (line, character) = self.lsp_position(range.start);
        let params = json!({
            "textDocument": { "uri": path_to_uri(&self.path) },
            "position": { "line": line, "character": character },
        });
        let Some(rx) = self.lsp.borrow_mut().as_mut().and_then(|c| c.request("textDocument/definition", params).ok()) else {
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let Ok(response) = rx.recv().await else { return };
            this.update(cx, |this, cx| {
                if this.ctrl_link_generation != generation {
                    return; // the pointer moved on
                }
                if let Some(link) = this.ctrl_link.as_mut() {
                    link.valid = parse_definition(&response).is_some();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn clear_ctrl_link(&mut self, cx: &mut Context<Self>) {
        if self.ctrl_link.take().is_some() {
            self.ctrl_link_generation += 1;
            cx.notify();
        }
    }

    /// The link underline's byte columns on `line`, if it's there.
    pub(super) fn ctrl_link_on_line(&self, line_range: std::ops::Range<usize>) -> Option<(usize, usize)> {
        let link = self.ctrl_link.as_ref().filter(|l| l.valid)?;
        (link.range.start >= line_range.start && link.range.end <= line_range.end)
            .then(|| (link.range.start - line_range.start, link.range.end - line_range.start))
    }

    /// Document range of the identifier under a window position on `line`.
    fn word_at_point(&self, line: usize, position: gpui::Point<gpui::Pixels>) -> Option<std::ops::Range<usize>> {
        let (shaped, bounds) = self.line_layouts.get(&line)?;
        let range = self.buffer.line_range(line);
        let text = &self.buffer.text()[range.clone()];
        let col = shaped.index_for_x(position.x - bounds.left()).filter(|c| *c < text.len())?;
        let word = super::hover::word_range(text, col);
        // Names only: not keywords-only punctuation runs or numbers.
        let is_name = !word.is_empty() && !text[word.clone()].starts_with(|c: char| c.is_ascii_digit());
        is_name.then(|| range.start + word.start..range.start + word.end)
    }
}
