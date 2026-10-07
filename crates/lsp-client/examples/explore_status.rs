//! Prints jdtls's progress/status messages while it starts, then after a
//! dependency is added to the project's pom.xml (restored afterwards).
//!
//! `cargo run -p rji-lsp-client --example explore_status -- <maven-project-dir> <java.exe>`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rji_lsp_client::{LspClient, discover_jdtls};

fn drain(client: &LspClient, secs: u64, label: &str) {
    println!("---- {label}");
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        match client.status_rx.try_recv() {
            Ok(status) => println!("{status:?}"),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = PathBuf::from(args.next().unwrap());
    let java = PathBuf::from(args.next().unwrap());
    let jdtls = discover_jdtls().unwrap();
    let data = std::env::temp_dir().join(format!("rji-explore-status-{}", std::process::id()));
    let mut client = LspClient::spawn(&java, &jdtls.server_dir, &project, &data)?;
    drain(&client, 20, "startup");

    let pom = project.join("pom.xml");
    let original = std::fs::read_to_string(&pom)?;
    let dep = "<dependency>\n            <groupId>com.h2database</groupId>\n            <artifactId>h2</artifactId>\n            <version>2.3.232</version>\n            <scope>test</scope>\n        </dependency>\n    </dependencies>";
    std::fs::write(&pom, original.replacen("</dependencies>", dep, 1))?;
    client.project_configuration_update(&pom)?;
    drain(&client, 40, "after adding h2 to pom.xml");

    std::fs::write(&pom, original)?;
    client.shutdown();
    std::fs::remove_dir_all(&data).ok();
    Ok(())
}
