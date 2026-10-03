//! Runs a real update check on a project folder and prints the report:
//! `cargo run -p rji-updates --example check -- <project-dir>`
fn main() {
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from);
    let installed = rji_updates::Installed {
        jdk: Some("java version \"25.0.2\"".into()),
        maven: rji_updates::detect_system_maven(),
    };
    println!("system maven: {:?}", installed.maven);
    let report = rji_updates::check(dir.as_deref(), &installed);
    for item in &report.items {
        let pins = item.site.as_ref().map_or(0, |s| s.ranges.len());
        println!(
            "{:<13} {} -> {} ({:?}), safe patch: {:?}, pins in file: {pins}, download: {:?}",
            item.component.label(), item.current, item.latest, item.bump, item.safe_patch, item.download_url
        );
    }
    for err in &report.errors {
        println!("error: {err}");
    }
}
