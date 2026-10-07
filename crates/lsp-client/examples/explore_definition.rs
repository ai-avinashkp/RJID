//! Dumps jdtls's textDocument/definition responses (project + JDK class)
//! and java/classFileContents for a jdt:// result.
//!
//! `cargo run -p rji-lsp-client --example explore_definition -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::Duration;

use rji_lsp_client::{LspClient, discover_jdtls, path_to_uri};
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().unwrap());
    let java = PathBuf::from(args.next().unwrap());
    let jdtls = discover_jdtls().unwrap();
    let data = std::env::temp_dir().join(format!("rji-explore-def-{}", std::process::id()));
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data)?;
    let file = project.join("src/main/java/com/example/cli/Probe.java");
    let uri = path_to_uri(&file);
    let text = "package com.example.cli;\n\npublic class Probe {\n    void f() {\n        String s = App.greeting(\"x\");\n        System.out.println(s);\n    }\n}\n";
    std::fs::write(&file, text)?;
    client.did_open(&file, text)?;
    std::thread::sleep(Duration::from_secs(10));
    for (label, line, ch) in [("App.greeting", 4, 25), ("println", 5, 22), ("String", 4, 9)] {
        let r = client.request("textDocument/definition", json!({"textDocument":{"uri":uri},"position":{"line":line,"character":ch}}))?.recv_blocking()?;
        println!("==== {label}: {}", serde_json::to_string(&r["result"])?.chars().take(500).collect::<String>());
        if let Some(target) = r["result"].as_array().and_then(|a| a.first()).and_then(|l| l["uri"].as_str())
            && target.starts_with("jdt://")
        {
            let c = client.request("java/classFileContents", json!({"uri": target}))?.recv_blocking()?;
            let body = c["result"].as_str().unwrap_or("");
            println!("   classFileContents: {} chars; starts: {:?}", body.len(), body.chars().take(120).collect::<String>());
        }
    }
    client.shutdown();
    std::fs::remove_file(&file).ok();
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
