//! Text edits, workspace edits and code actions as the language server
//! sends them, parsed into plain structs the editor can apply.

use std::path::PathBuf;

use serde_json::Value;

use crate::client::uri_to_path;

/// A replacement in one document. Positions are 0-based lines and UTF-16
/// columns, as LSP defines them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub new_text: String,
}

/// All edits to one file. `create` is set when the server asked for the
/// file to be created (a "Create class …" quick fix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
    pub create: bool,
}

#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    /// The changes to make (already extracted from a
    /// `java.apply.workspaceEdit` command when the server sent one).
    pub edit: Vec<FileEdit>,
    /// The action as received, for requests that need it echoed back.
    pub raw: Value,
}

pub fn parse_text_edit(value: &Value) -> Option<TextEdit> {
    let pos = |p: &Value| -> Option<(u32, u32)> {
        Some((p.get("line")?.as_u64()? as u32, p.get("character")?.as_u64()? as u32))
    };
    let range = value.get("range")?;
    Some(TextEdit {
        start: pos(range.get("start")?)?,
        end: pos(range.get("end")?)?,
        new_text: value.get("newText")?.as_str()?.to_string(),
    })
}

pub fn parse_text_edits(value: Option<&Value>) -> Vec<TextEdit> {
    value
        .and_then(Value::as_array)
        .map(|edits| edits.iter().filter_map(parse_text_edit).collect())
        .unwrap_or_default()
}

/// Both shapes of `WorkspaceEdit`: `changes: {uri: [TextEdit]}` and
/// `documentChanges: [TextDocumentEdit | CreateFile | …]`.
pub fn parse_workspace_edit(value: &Value) -> Vec<FileEdit> {
    let mut files: Vec<FileEdit> = Vec::new();
    let mut add = |path: PathBuf, edits: Vec<TextEdit>, create: bool| {
        if let Some(existing) = files.iter_mut().find(|f| f.path == path) {
            existing.edits.extend(edits);
            existing.create |= create;
        } else {
            files.push(FileEdit { path, edits, create });
        }
    };
    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            if let Some(path) = uri_to_path(uri) {
                // A file the server writes in full that doesn't exist yet.
                let create = !path.exists();
                add(path, parse_text_edits(Some(edits)), create);
            }
        }
    }
    if let Some(changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            match change.get("kind").and_then(Value::as_str) {
                Some("create") => {
                    if let Some(path) = change.get("uri").and_then(Value::as_str).and_then(uri_to_path) {
                        add(path, Vec::new(), true);
                    }
                }
                Some(_) => {} // rename/delete file: not supported
                None => {
                    let uri = change.pointer("/textDocument/uri").and_then(Value::as_str);
                    if let Some(path) = uri.and_then(uri_to_path) {
                        add(path, parse_text_edits(change.get("edits")), false);
                    }
                }
            }
        }
    }
    files.retain(|f| f.create || !f.edits.is_empty());
    files
}

/// A `textDocument/codeAction` response: `CodeAction` literals and bare
/// `Command`s. Only actions that carry edits are returned (jdtls wraps
/// some in a `java.apply.workspaceEdit` command).
pub fn parse_code_actions(response: &Value) -> Vec<CodeAction> {
    let Some(items) = response.get("result").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let kind = item.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
            let edit = match item.get("edit") {
                Some(edit) => parse_workspace_edit(edit),
                None => {
                    let command = item.get("command").unwrap_or(item);
                    let name = command.get("command").and_then(Value::as_str)?;
                    if name != "java.apply.workspaceEdit" {
                        return None;
                    }
                    parse_workspace_edit(command.pointer("/arguments/0")?)
                }
            };
            (!edit.is_empty()).then(|| CodeAction { title, kind, edit, raw: item.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_workspace_edit_shapes_and_commands() {
        let file = std::env::temp_dir().join("rji-edits-test-Existing.java");
        std::fs::write(&file, "x").unwrap();
        let uri = crate::client::path_to_uri(&file);
        let edit = json!({ "range": { "start": { "line": 0, "character": 24 }, "end": { "line": 2, "character": 0 } },
                           "newText": "\n\nimport java.util.HashMap;\n\n" });
        let response = json!({ "result": [
            { "title": "Organize imports", "kind": "source.organizeImports", "edit": { "changes": { uri.clone(): [edit] } } },
            { "title": "Wrapped", "command": { "command": "java.apply.workspaceEdit", "arguments": [
                { "documentChanges": [ { "textDocument": { "uri": uri.clone(), "version": 3 }, "edits": [edit] } ] } ] } },
            { "title": "Needs a prompt", "command": { "command": "java.action.generateAccessorsPrompt" } },
        ] });
        let actions = parse_code_actions(&response);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].edit[0].edits[0].start, (0, 24));
        assert!(!actions[0].edit[0].create);
        assert_eq!(actions[1].title, "Wrapped");
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn edits_to_missing_files_are_creations() {
        let missing = std::env::temp_dir().join("rji-edits-test-Missing-Class.java");
        let _ = std::fs::remove_file(&missing);
        let uri = crate::client::path_to_uri(&missing);
        let edit = json!({ "changes": { uri: [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": "class X {}" } ] } });
        let files = parse_workspace_edit(&edit);
        assert_eq!(files.len(), 1);
        assert!(files[0].create);
    }
}
