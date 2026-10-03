//! End-to-end check of the LSP client against a real jdtls: start it on a
//! project, open a document, request completions mid-expression, and check
//! real semantic results come back (plus diagnostics for a broken file).
//! Needs a JDK and the VS Code Java extension (for jdtls), so it's an
//! example rather than a `#[test]`:
//!
//! `cargo run -p rji-lsp-client --example completion_smoke -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rji_lsp_client::{LspClient, discover_jdtls};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().expect("usage: <project-dir> <java.exe>"));
    let java = PathBuf::from(args.next().expect("usage: <project-dir> <java.exe>"));
    let jdtls = discover_jdtls().ok_or_else(|| anyhow::anyhow!("jdtls not found"))?;
    let data_dir = std::env::temp_dir().join(format!("rji-lsp-smoke-{}", std::process::id()));

    let started = Instant::now();
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data_dir)?;
    println!("jdtls initialized in {:.1}s", started.elapsed().as_secs_f32());

    let file = project.join("src/main/java/com/example/Probe.java");
    let text = "package com.example;\n\npublic class Probe {\n    void f() {\n        System.out.pri\n    }\n}\n";
    client.did_open(&file, text)?;

    // jdtls may answer empty while it's still importing the project, so
    // retry for a while before failing.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let rx = client.request_completion(&file, 4, 22)?;
        let items = rx.recv_blocking()?;
        let labels: Vec<&str> = items.iter().take(8).map(|i| i.label.as_str()).collect();
        println!("{} items, first: {labels:?}", items.len());
        if let Some(item) = items.iter().find(|i| i.label.starts_with("println")) {
            println!("found: {:?}", item);
            anyhow::ensure!(item.kind == "method", "println should be a method, got {}", item.kind);
            anyhow::ensure!(item.insert_text.starts_with("println"), "insert text {:?}", item.insert_text);
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "no `println` completion within 90s");
        std::thread::sleep(Duration::from_secs(3));
    }

    // Hover on `println` in a complete statement: signature + Javadoc.
    let hover_text = "package com.example;\n\npublic class Probe {\n    void f() {\n        System.out.println(\"x\");\n    }\n}\n";
    client.did_change(&file, 2, hover_text)?;
    let hover = client.request_hover(&file, 4, 21)?.recv_blocking()?;
    let hover = hover.ok_or_else(|| anyhow::anyhow!("no hover for println"))?;
    println!("hover:\n{}\n", hover.lines().take(6).collect::<Vec<_>>().join("\n"));
    anyhow::ensure!(hover.contains("println"), "hover should describe println");

    // The same document with an error must produce diagnostics.
    client.did_change(&file, 3, "package com.example;\nclass Probe { int x = }\n")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!remaining.is_zero(), "no diagnostics for the broken document");
        if let Ok(published) = client.diagnostics_rx.try_recv() {
            if published.file_path.ends_with("Probe.java") && !published.diagnostics.is_empty() {
                println!("diagnostics: {:?}", published.diagnostics);
                break;
            }
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    client.shutdown();
    std::fs::remove_dir_all(&data_dir).ok();
    println!("\nLSP SMOKE TEST PASSED: initialize, completion, and diagnostics all work against real jdtls.");
    Ok(())
}
