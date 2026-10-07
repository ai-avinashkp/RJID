//! A minimal LSP client for jdtls: spawn the server, do the
//! `initialize`/`initialized` handshake, keep documents in sync
//! (`didOpen`/`didChange`/`didClose`, full-text sync), surface
//! `publishDiagnostics`, and request completions.
//!
//! Every response is routed by request id through a small pending-request
//! table, so a request can either block for its reply (the one-time
//! `initialize` handshake, done off the UI thread) or hand back a channel
//! the UI awaits without blocking (completion).

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::{Value, json};

use crate::discover::{find_equinox_launcher, shared_config_dir};
use crate::jsonrpc::{read_message, write_message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 0-based line number, matching the buffer's own line indexing.
    pub line: u32,
    /// Start column and end position (0-based; columns in UTF-16 code
    /// units, as LSP sends them).
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    /// The diagnostic exactly as the server sent it (code actions need it
    /// back verbatim, including its `code`).
    pub raw: Value,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct PublishDiagnostics {
    pub file_path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub detail: Option<String>,
    /// Short human-readable kind ("method", "class", ...).
    pub kind: &'static str,
    /// Extra edits to make when this item is accepted — for a class from
    /// another package, its `import`.
    pub additional_edits: Vec<crate::edits::TextEdit>,
}

type ResponseHandler = Box<dyn FnOnce(Value) + Send>;
type Pending = Arc<Mutex<HashMap<i64, ResponseHandler>>>;
type SharedStdin = Arc<Mutex<ChildStdin>>;

/// Most completion items worth showing — jdtls can return thousands for an
/// empty prefix; the popup only ever shows a scrolled handful anyway.
const MAX_COMPLETION_ITEMS: usize = 200;

pub struct LspClient {
    child: Child,
    stdin: SharedStdin,
    next_id: i64,
    pending: Pending,
    pub diagnostics_rx: Receiver<PublishDiagnostics>,
    /// Progress and status reports (project import, dependency downloads,
    /// indexing) — see [`ServerStatus`].
    pub status_rx: Receiver<ServerStatus>,
}

impl LspClient {
    /// Spawns jdtls and completes the `initialize`/`initialized` handshake
    /// before returning — call off the UI thread, since jdtls takes a few
    /// seconds to start.
    pub fn spawn(
        java_exe: &Path,
        server_dir: &Path,
        workspace_root: &Path,
        data_dir: &Path,
    ) -> anyhow::Result<Self> {
        let launcher = find_equinox_launcher(server_dir)
            .ok_or_else(|| anyhow!("no equinox launcher jar found under {}", server_dir.display()))?;
        let config_dir = shared_config_dir(server_dir);
        std::fs::create_dir_all(data_dir).context("creating jdtls workspace-data directory")?;

        let mut command = Command::new(java_exe);
        command
            .arg("-Declipse.application=org.eclipse.jdt.ls.core.id1")
            .arg("-Dosgi.bundles.defaultStartLevel=4")
            .arg("-Declipse.product=org.eclipse.jdt.ls.core.product")
            .arg("-Dosgi.checkConfiguration=true")
            .arg(format!(
                "-Dosgi.sharedConfiguration.area={}",
                config_dir.display()
            ))
            .arg("-Dosgi.sharedConfiguration.area.readOnly=true")
            .arg("-Dosgi.configuration.cascaded=true")
            .arg("-Xms1G")
            .arg("--add-modules=ALL-SYSTEM")
            .arg("--add-opens")
            .arg("java.base/java.util=ALL-UNNAMED")
            .arg("--add-opens")
            .arg("java.base/java.lang=ALL-UNNAMED")
            .arg("-jar")
            .arg(&launcher)
            .arg("-data")
            .arg(data_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_window(&mut command);

        let mut child = command.spawn().context("spawning jdtls (java -jar ...)")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("jdtls stdin not piped"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("jdtls stdout not piped"))?;

        let stdin: SharedStdin = Arc::new(Mutex::new(stdin));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (diagnostics_tx, diagnostics_rx) = unbounded();
        let (status_tx, status_rx) = unbounded();
        spawn_reader_thread(stdout, stdin.clone(), pending.clone(), diagnostics_tx, status_tx);

        let mut client = LspClient {
            child,
            stdin,
            next_id: 1,
            pending,
            diagnostics_rx,
            status_rx,
        };
        client.initialize(workspace_root)?;
        Ok(client)
    }

    /// Sends a request; `on_response` runs on the reader thread when the
    /// matching reply arrives.
    fn send_request_with(
        &mut self,
        method: &str,
        params: Value,
        on_response: ResponseHandler,
    ) -> anyhow::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.lock().unwrap().insert(id, on_response);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let result = write_message(&mut *self.stdin.lock().unwrap(), &message);
        if result.is_err() {
            self.pending.lock().unwrap().remove(&id);
        }
        result
    }

    fn send_notification(&mut self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        write_message(&mut *self.stdin.lock().unwrap(), &message)
    }

    fn initialize(&mut self, workspace_root: &Path) -> anyhow::Result<()> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": path_to_uri(workspace_root),
            // Lets definitions resolve into JDK / library classes (jdt:// URIs
            // whose source is fetched with `java/classFileContents`).
            "initializationOptions": {
                "extendedClientCapabilities": { "classFileContentsSupport": true }
            },
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": { "relatedInformation": false },
                    "completion": {
                        "completionItem": { "snippetSupport": false },
                        "contextSupport": false
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    // Code actions as literals (title + edit) rather than
                    // bare commands, so quick fixes carry their edits.
                    "codeAction": {
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                                             "refactor.rewrite", "source", "source.organizeImports"]
                            }
                        }
                    },
                    "formatting": {},
                    "rename": { "prepareSupport": false }
                },
                "workspace": {
                    "applyEdit": true,
                    "workspaceEdit": { "documentChanges": true }
                },
                // Lets jdtls report project import / download progress.
                "window": { "workDoneProgress": true }
            },
        });
        let (tx, rx) = bounded(1);
        self.send_request_with(
            "initialize",
            params,
            Box::new(move |value| {
                let _ = tx.send_blocking(value);
            }),
        )?;
        rx.recv_blocking()
            .context("jdtls closed its output before answering `initialize`")?;
        self.send_notification("initialized", json!({}))?;
        Ok(())
    }

    pub fn did_open(&mut self, path: &Path, text: &str) -> anyhow::Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": "java",
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    pub fn did_change(&mut self, path: &Path, version: i64, text: &str) -> anyhow::Result<()> {
        self.send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": [ { "text": text } ],
            }),
        )
    }

    /// Tells jdtls a build file (`pom.xml`, `build.gradle[.kts]`) changed, so
    /// it re-imports the project and downloads new dependencies. Progress
    /// arrives on `status_rx`.
    pub fn project_configuration_update(&mut self, build_file: &Path) -> anyhow::Result<()> {
        self.send_notification(
            "java/projectConfigurationUpdate",
            json!({ "uri": path_to_uri(build_file) }),
        )
    }

    pub fn did_close(&mut self, path: &Path) -> anyhow::Result<()> {
        self.send_notification(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(path) } }),
        )
    }

    /// Asks for completions at a 0-based `line`/`character` (a UTF-8 byte
    /// column is fine for the ASCII identifiers completion is used on).
    /// Returns immediately; the items arrive on the returned channel.
    pub fn request_completion(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Receiver<Vec<CompletionItem>>> {
        let (tx, rx) = bounded(1);
        self.send_request_with(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character },
            }),
            Box::new(move |value| {
                let _ = tx.send_blocking(parse_completion(&value));
            }),
        )?;
        Ok(rx)
    }

    /// Sends any request; the full response message (with `result` or
    /// `error`) arrives on the returned channel.
    pub fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Receiver<Value>> {
        let (tx, rx) = bounded(1);
        self.send_request_with(
            method,
            params,
            Box::new(move |value| {
                let _ = tx.send_blocking(value);
            }),
        )?;
        Ok(rx)
    }

    /// Asks for hover info (signature + Javadoc) at a 0-based `line` and
    /// UTF-16 `character`. The Markdown text (or `None` when there's
    /// nothing to show) arrives on the returned channel.
    pub fn request_hover(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> anyhow::Result<Receiver<Option<String>>> {
        let (tx, rx) = bounded(1);
        self.send_request_with(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character },
            }),
            Box::new(move |value| {
                let _ = tx.send_blocking(parse_hover(&value));
            }),
        )?;
        Ok(rx)
    }

    /// Stops jdtls now. Also runs on drop, but drop never happens if the
    /// process exits without unwinding, so the app calls this explicitly
    /// when its window closes.
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn spawn_reader_thread(
    stdout: std::process::ChildStdout,
    stdin: SharedStdin,
    pending: Pending,
    diagnostics_tx: Sender<PublishDiagnostics>,
    status_tx: Sender<ServerStatus>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(Some(value)) = read_message(&mut reader) {
            let id = value.get("id").and_then(Value::as_i64);
            match (value.get("method").and_then(Value::as_str), id) {
                // Server -> client request: jdtls waits for a reply, so
                // always answer. We implement none of these, so reply with
                // an empty result (one `null` per requested item for
                // `workspace/configuration`, which expects an array).
                (Some(method), Some(id)) => {
                    let result = if method == "workspace/configuration" {
                        let count = value
                            .pointer("/params/items")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; count])
                    } else {
                        Value::Null
                    };
                    let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    let _ = write_message(&mut *stdin.lock().unwrap(), &reply);
                }
                (Some("textDocument/publishDiagnostics"), None) => {
                    if let Some(parsed) = parse_publish_diagnostics(&value)
                        && diagnostics_tx.send_blocking(parsed).is_err()
                    {
                        break;
                    }
                }
                (Some(method), None) => {
                    if let Some(status) = parse_server_status(method, &value) {
                        let _ = status_tx.send_blocking(status);
                    }
                }
                (None, Some(id)) => {
                    let handler = pending.lock().unwrap().remove(&id);
                    if let Some(handler) = handler {
                        handler(value);
                    }
                }
                (None, None) => {}
            }
        }
        // jdtls went away: fail every waiter instead of leaving it hanging.
        pending.lock().unwrap().clear();
    });
}

fn parse_publish_diagnostics(value: &Value) -> Option<PublishDiagnostics> {
    let params = value.get("params")?;
    let uri = params.get("uri")?.as_str()?;
    let file_path = uri_to_path(uri)?;
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let range = entry.get("range")?;
            let pos = |which: &str, field: &str| range.get(which)?.get(field)?.as_u64().map(|v| v as u32);
            let line = pos("start", "line")?;
            let start_char = pos("start", "character").unwrap_or(0);
            let end_line = pos("end", "line").unwrap_or(line);
            let end_char = pos("end", "character").unwrap_or(start_char);
            let message = entry.get("message")?.as_str()?.to_string();
            let severity = match entry.get("severity").and_then(Value::as_u64).unwrap_or(1) {
                1 => DiagnosticSeverity::Error,
                2 => DiagnosticSeverity::Warning,
                3 => DiagnosticSeverity::Info,
                _ => DiagnosticSeverity::Hint,
            };
            Some(Diagnostic { line, start_char, end_line, end_char, severity, message, raw: entry.clone() })
        })
        .collect();
    Some(PublishDiagnostics { file_path, diagnostics })
}

/// Hover `contents` as Markdown. The spec allows a `MarkupContent`
/// (`{kind, value}`), a `MarkedString` (a string or `{language, value}`),
/// or an array of `MarkedString`s (what jdtls sends: the signature as a
/// `java` block, then the Javadoc).
pub fn parse_hover(response: &Value) -> Option<String> {
    fn marked(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Object(obj) => {
                let text = obj.get("value")?.as_str()?;
                match obj.get("language").and_then(Value::as_str) {
                    Some(lang) => Some(format!("```{lang}\n{text}\n```")),
                    None => Some(text.to_string()),
                }
            }
            _ => None,
        }
    }
    let contents = response.get("result")?.get("contents")?;
    let text = match contents {
        Value::Array(items) => items.iter().filter_map(marked).collect::<Vec<_>>().join("\n\n"),
        other => marked(other)?,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// What the server is busy with, for the status bar.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    /// `$/progress` (work-done progress): `token` identifies one task.
    Progress {
        token: String,
        title: Option<String>,
        message: Option<String>,
        percentage: Option<u32>,
        done: bool,
    },
    /// jdtls's own `language/status` (e.g. "Starting", "ServiceReady",
    /// "Message" with text like "Importing Maven project(s)").
    Status { kind: String, message: String },
}

/// `$/progress` and jdtls `language/status` notifications.
pub fn parse_server_status(method: &str, value: &Value) -> Option<ServerStatus> {
    let params = value.get("params")?;
    match method {
        "$/progress" => {
            let token = match params.get("token")? {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let progress = params.get("value")?;
            let text = |key: &str| progress.get(key).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty());
            Some(ServerStatus::Progress {
                token,
                title: text("title"),
                message: text("message"),
                percentage: progress.get("percentage").and_then(Value::as_u64).map(|p| p.min(100) as u32),
                done: progress.get("kind").and_then(Value::as_str) == Some("end"),
            })
        }
        "language/status" => Some(ServerStatus::Status {
            kind: params.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
            message: params.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        }),
        _ => None,
    }
}

/// Accepts both response shapes the spec allows: a bare item array or a
/// `CompletionList { items }`.
fn parse_completion(response: &Value) -> Vec<CompletionItem> {
    let result = response.get("result").unwrap_or(&Value::Null);
    let items = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array));
    let Some(items) = items else {
        return Vec::new();
    };
    let mut parsed: Vec<(String, CompletionItem)> = items
        .iter()
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_string();
            let insert_text = item
                .pointer("/textEdit/newText")
                .or_else(|| item.get("insertText"))
                .and_then(Value::as_str)
                .unwrap_or(&label)
                .to_string();
            let sort_text = item
                .get("sortText")
                .and_then(Value::as_str)
                .unwrap_or(&label)
                .to_string();
            Some((
                sort_text,
                CompletionItem {
                    detail: item.get("detail").and_then(Value::as_str).map(str::to_string),
                    additional_edits: crate::edits::parse_text_edits(item.get("additionalTextEdits")),
                    kind: completion_kind(item.get("kind").and_then(Value::as_u64).unwrap_or(1)),
                    label,
                    insert_text,
                },
            ))
        })
        .collect();
    // The server's own relevance order.
    parsed.sort_by(|a, b| a.0.cmp(&b.0));
    parsed.truncate(MAX_COMPLETION_ITEMS);
    parsed.into_iter().map(|(_, item)| item).collect()
}

fn completion_kind(kind: u64) -> &'static str {
    match kind {
        2 | 3 => "method",
        4 => "constructor",
        5 | 10 => "field",
        6 => "variable",
        7 | 22 => "class",
        8 => "interface",
        9 => "package",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        20 => "enum const",
        21 => "constant",
        _ => "text",
    }
}

/// jdtls is a console program; on Windows a GUI parent spawning it would
/// otherwise flash (or leave open) a console window.
fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Minimal `file://` URI encoding — handles plain local paths (spaces
/// included); not a general-purpose percent-encoder.
pub fn path_to_uri(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/").replace(' ', "%20");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let without_scheme = uri.strip_prefix("file:///").or_else(|| uri.strip_prefix("file://"))?;
    let decoded = without_scheme.replace("%20", " ").replace("%3A", ":").replace("%3a", ":");
    Some(PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hover_shapes() {
        let jdtls = json!({ "id": 1, "result": { "contents": [
            { "language": "java", "value": "void java.io.PrintStream.println(String x)" },
            "Prints a String and then terminates the line."
        ] } });
        assert_eq!(
            parse_hover(&jdtls).as_deref(),
            Some("```java
void java.io.PrintStream.println(String x)
```

Prints a String and then terminates the line.")
        );
        let markup = json!({ "result": { "contents": { "kind": "markdown", "value": "**int** count" } } });
        assert_eq!(parse_hover(&markup).as_deref(), Some("**int** count"));
        assert_eq!(parse_hover(&json!({ "result": null })), None);
        assert_eq!(parse_hover(&json!({ "result": { "contents": [] } })), None);
    }

    #[test]
    fn parses_completion_list_and_prefers_text_edit() {
        let response = json!({
            "id": 3,
            "result": {
                "isIncomplete": false,
                "items": [
                    { "label": "println(String x) : void", "kind": 2, "sortText": "b",
                      "textEdit": { "newText": "println", "range": {} } },
                    { "label": "print", "kind": 2, "sortText": "a", "insertText": "print" }
                ]
            }
        });
        let items = parse_completion(&response);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "print");
        assert_eq!(items[1].insert_text, "println");
        assert_eq!(items[1].kind, "method");
    }

    #[test]
    fn parses_bare_array_and_empty_results() {
        assert_eq!(parse_completion(&json!({ "result": [ { "label": "x" } ] })).len(), 1);
        assert!(parse_completion(&json!({ "result": null })).is_empty());
    }

    #[test]
    fn uri_round_trip_with_spaces_and_encoded_drive_colon() {
        let uri = path_to_uri(Path::new("C:\\My Projects\\App.java"));
        assert_eq!(uri, "file:///C:/My%20Projects/App.java");
        assert_eq!(
            uri_to_path("file:///c%3A/My%20Projects/App.java").unwrap(),
            PathBuf::from("c:/My Projects/App.java")
        );
    }
}
