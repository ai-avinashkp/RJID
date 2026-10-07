//! Dumps raw jdtls responses for Override/Implement and Delegate Methods,
//! and the plain "source" code actions on a small class.
//!
//! `cargo run -p rji-lsp-client --example explore_source -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::Duration;

use rji_lsp_client::{LspClient, discover_jdtls, path_to_uri};
use serde_json::{Value, json};

fn show(label: &str, v: &Value) {
    let s = serde_json::to_string_pretty(v).unwrap();
    println!("==== {label}\n{}\n", &s[..s.len().min(2400)]);
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().unwrap());
    let java = PathBuf::from(args.next().unwrap());
    let jdtls = discover_jdtls().unwrap();
    let data = std::env::temp_dir().join(format!("rji-explore-src-{}", std::process::id()));
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data)?;
    let file = project.join("src/main/java/com/example/cli/Shelf.java");
    let uri = path_to_uri(&file);
    let text = "package com.example.cli;\n\nimport java.util.ArrayList;\nimport java.util.List;\n\npublic class Shelf implements Comparable<Shelf> {\n    private String name;\n    private List<String> books = new ArrayList<>();\n\n    int size() {\n        return books.size();\n    }\n}\n";
    std::fs::write(&file, text)?;
    client.did_open(&file, text)?;
    std::thread::sleep(Duration::from_secs(8));
    let ctx = json!({"textDocument":{"uri":uri},"range":{"start":{"line":9,"character":4},"end":{"line":9,"character":4}},"context":{"diagnostics":[]}});

    let r = client.request("java/listOverridableMethods", ctx.clone())?.recv_blocking()?;
    show("listOverridableMethods", &r);
    let methods: Vec<Value> = r["result"]["methods"].as_array().cloned().unwrap_or_default();
    let pick: Vec<Value> = methods.iter().filter(|m| m["name"] == "compareTo" || m["name"] == "toString").cloned().collect();
    let r = client.request("java/addOverridableMethods", json!({"context": ctx, "overridableMethods": pick}))?.recv_blocking()?;
    show("addOverridableMethods", &r);

    let r = client.request("java/checkDelegateMethodsStatus", ctx.clone())?.recv_blocking()?;
    show("checkDelegateMethodsStatus", &r);
    let entry = r["result"]["delegateFields"].as_array().and_then(|f| f.first()).cloned();
    if let Some(field) = entry {
        let m = field["delegateMethods"].as_array().and_then(|m| m.iter().find(|m| m["name"] == "isEmpty")).cloned();
        if let Some(m) = m {
            let r = client.request("java/generateDelegateMethods", json!({"context": ctx, "delegateEntries": [{"field": field["field"], "delegateMethod": m}]}))?.recv_blocking()?;
            show("generateDelegateMethods", &r);
        }
    }
    let mut src = ctx.clone();
    src["context"]["only"] = json!(["source"]);
    let r = client.request("textDocument/codeAction", src)?.recv_blocking()?;
    let titles: Vec<_> = r["result"].as_array().map(|a| a.iter().map(|x| (x["title"].clone(), x["kind"].clone(), x.get("edit").is_some(), x["command"]["command"].clone())).collect()).unwrap_or_default();
    println!("==== source actions\n{titles:#?}");
    client.shutdown();
    std::fs::remove_file(&file).ok();
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
