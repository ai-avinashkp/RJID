//! Dumps raw jdtls responses for completion/resolve, quick-fix code actions
//! and organize-imports, to see which shapes the IDE has to handle.
//!
//! `cargo run -p rji-lsp-client --example explore_actions -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::Duration;

use rji_lsp_client::{LspClient, discover_jdtls, path_to_uri};
use serde_json::{Value, json};

fn show(label: &str, v: &Value) {
    let s = serde_json::to_string_pretty(v).unwrap();
    println!("==== {label} ({} bytes)\n{}\n", s.len(), &s[..s.len().min(3000)]);
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().unwrap());
    let java = PathBuf::from(args.next().unwrap());
    let jdtls = discover_jdtls().unwrap();
    let data = std::env::temp_dir().join(format!("rji-explore-{}", std::process::id()));
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data)?;
    let file = project.join("src/main/java/com/example/cli/Probe.java");
    let uri = path_to_uri(&file);
    let default_text = "package com.example.cli;\n\npublic class Probe {\n    void f() {\n        ArrayLi\n        HashMap<String, Integer> m = null;\n    }\n}\n";
    let probe = std::env::var("PROBE_TEXT").map(|t| t.replace("\\n", "\n"));
    let text = probe.as_deref().unwrap_or(default_text);
    client.did_open(&file, text)?;
    std::thread::sleep(Duration::from_secs(8));

    let comp = client.request("textDocument/completion", json!({"textDocument":{"uri":uri},"position":{"line":4,"character":15}}))?.recv_blocking()?;
    let items = comp.pointer("/result/items").and_then(Value::as_array).cloned().unwrap_or_default();
    let item = items.iter().find(|i| i["label"].as_str().is_some_and(|l| l.starts_with("ArrayList "))).cloned();
    if let Some(item) = item {
        show("completion item ArrayList", &item);
        let resolved = client.request("completionItem/resolve", item)?.recv_blocking()?;
        show("resolved", &resolved);
    } else {
        println!("labels: {:?}", items.iter().take(10).map(|i| i["label"].clone()).collect::<Vec<_>>());
    }

    // Diagnostics for the HashMap line, then quick fixes.
    let mut diags = Vec::new();
    for _ in 0..50 {
        if let Ok(p) = client.diagnostics_rx.try_recv() { if !p.diagnostics.is_empty() { diags = p.diagnostics; } }
        if !diags.is_empty() { break; }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("diags: {diags:?}");
    let d = diags.iter().find(|d| d.message.contains("HashMap")).cloned();
    if let Some(d) = d {
        let lsp_diag = d.raw.clone();
        let actions = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":lsp_diag["range"],"context":{"diagnostics":[lsp_diag],"only":["quickfix"]}}))?.recv_blocking()?;
        show("quickfix actions", &actions);
    }
    let org = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"context":{"diagnostics":[],"only":["source.organizeImports"]}}))?.recv_blocking()?;
    show("organize imports", &org);
    let src = client.request("textDocument/codeAction", json!({"textDocument":{"uri":uri},"range":{"start":{"line":3,"character":4},"end":{"line":3,"character":4}},"context":{"diagnostics":[],"only":["source"]}}))?.recv_blocking()?;
    show("source actions", &src);
    let fmt = client.request("textDocument/formatting", json!({"textDocument":{"uri":uri},"options":{"tabSize":4,"insertSpaces":true}}))?.recv_blocking()?;
    show("formatting", &fmt);
    client.shutdown();
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
