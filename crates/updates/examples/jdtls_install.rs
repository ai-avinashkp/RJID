//! End-to-end: finds the latest stable jdtls, downloads + verifies +
//! installs it into a temp folder, then starts it on a project.
//!
//! `cargo run -p rji-updates --example jdtls_install -- <project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().expect("project dir"));
    let java = PathBuf::from(args.next().expect("java.exe"));
    let root = std::env::temp_dir().join(format!("rji-jdtls-e2e-{}", std::process::id()));

    let release = rji_updates::jdtls::latest_milestone()?;
    println!("latest stable: {} ({})", release.version, release.url);
    let started = Instant::now();
    let last = std::cell::Cell::new(0u64);
    let dir = rji_updates::jdtls::install(&release, &root, &rji_lsp_client::is_server_dir, &|done, total| {
        let mb = done / (5 * 1024 * 1024);
        if mb != last.get() {
            last.set(mb);
            println!("  {} MB / {:?}", done / (1024 * 1024), total.map(|t| t / (1024 * 1024)));
        }
    })?;
    println!("installed to {} in {:.1}s", dir.display(), started.elapsed().as_secs_f32());
    assert_eq!(rji_updates::jdtls::installed(&root), Some(dir.clone()));

    let found = rji_lsp_client::discover_jdtls_with(Some(&dir)).expect("discoverable");
    let data = root.join("data");
    let mut client = rji_lsp_client::LspClient::spawn(&java, &found.server_dir, &project, &data)?;
    println!("jdtls started from the downloaded copy: OK");
    client.shutdown();
    std::fs::remove_dir_all(&root).ok();
    Ok(())
}
