//! Dumps raw jdtls responses for the java/* generate requests and refactor
//! code actions on a small class.
//!
//! `cargo run -p rji-lsp-client --example explore_generate -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::Duration;

use rji_lsp_client::{LspClient, discover_jdtls, path_to_uri};
use serde_json::{Value, json};

fn show(label: &str, v: &Value) {
    let s = serde_json::to_string_pretty(v).unwrap();
    println!("==== {label}\n{}\n", &s[..s.len().min(2500)]);
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().unwrap());
    let java = PathBuf::from(args.next().unwrap());
    let jdtls = discover_jdtls().unwrap();
    let data = std::env::temp_dir().join(format!("rji-explore-gen-{}", std::process::id()));
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data)?;
    let file = project.join("src/main/java/com/example/cli/Person.java");
    let uri = path_to_uri(&file);
    let text = "package com.example.cli;\n\npublic class Person {\n    private String name;\n    private int age;\n    private static final int MAX = 3;\n\n    int twice() {\n        return age * 2 + 1;\n    }\n}\n";
    client.did_open(&file, text)?;
    std::thread::sleep(Duration::from_secs(8));
    let ctx = json!({"textDocument":{"uri":uri},"range":{"start":{"line":4,"character":4},"end":{"line":4,"character":4}},"context":{"diagnostics":[]}});

    for method in ["java/checkToStringStatus", "java/checkHashCodeEqualsStatus"] {
        let r = client.request(method, ctx.clone())?.recv_blocking()?;
        show(method, &r);
    }
    std::fs::write(&file, text)?;
    let status = client.request("java/checkConstructorsStatus", ctx.clone())?.recv_blocking()?;
    let fields = status["result"]["fields"].clone();
    let constructors = status["result"]["constructors"].clone();
    let r = client.request("java/generateConstructors", json!({"context":ctx,"constructors":constructors,"fields":fields}))?.recv_blocking()?;
    show("generateConstructors", &r);
    for kind in [json!(2), json!("both"), json!("BOTH")] {
        let mut flat = ctx.clone();
        flat["kind"] = kind.clone();
        let r = client.request("java/resolveUnimplementedAccessors", flat)?.recv_blocking()?;
        show(&format!("resolveUnimplementedAccessors kind={kind}"), &r);
    }
    let mut flat = ctx.clone();
    flat["kind"] = json!(2);
    let acc = client.request("java/resolveUnimplementedAccessors", flat)?.recv_blocking()?;
    let accessors = acc.get("result").cloned().unwrap_or(json!([]));
    let r = client.request("java/generateAccessors", json!({"context":ctx,"accessors":accessors}))?.recv_blocking()?;
    show("generateAccessors", &r);
    let r = client.request("java/generateToString", json!({"context":ctx,"fields":fields}))?.recv_blocking()?;
    show("generateToString", &r);
    let r = client.request("java/generateHashCodeEquals", json!({"context":ctx,"fields":fields,"regenerate":false}))?.recv_blocking()?;
    show("generateHashCodeEquals", &r);
    // Refactorings on a selected expression `age * 2`.
    let sel = json!({"textDocument":{"uri":uri},"range":{"start":{"line":8,"character":15},"end":{"line":8,"character":22}},"context":{"diagnostics":[],"only":["refactor"]}});
    let r = client.request("textDocument/codeAction", sel)?.recv_blocking()?;
    let titles: Vec<_> = r["result"].as_array().map(|a| a.iter().map(|x| (x["title"].clone(), x["kind"].clone(), x.get("edit").is_some(), x["command"]["command"].clone())).collect()).unwrap_or_default();
    println!("==== refactor actions on selection\n{titles:#?}");
    let r = client.request("textDocument/rename", json!({"textDocument":{"uri":uri},"position":{"line":4,"character":17},"newName":"years"}))?.recv_blocking()?;
    show("rename age->years", &r);
    client.shutdown();
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
