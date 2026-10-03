//! Generates a project from a template, then prints its detected tasks.
//!
//!   cargo run -p rji-project-java --example new_project -- <parent> <name> <console|spring|javafx> <maven|gradle|none> [java-release]

use rji_project_java::templates::{NewProjectSpec, Template, TemplateBuild, TemplateVersions, create_project};
use rji_project_java::{detect_project, project_tasks};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(args.len() >= 4, "usage: <parent> <name> <console|spring|javafx> <maven|gradle|none> [java-release]");
    let template = match args[2].as_str() {
        "spring" => Template::SpringBootWeb,
        "javafx" => Template::JavaFx,
        _ => Template::Console,
    };
    let build = match args[3].as_str() {
        "gradle" => TemplateBuild::Gradle,
        "none" => TemplateBuild::None,
        _ => TemplateBuild::Maven,
    };
    let java_release: u32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(21);
    let spec = NewProjectSpec {
        template,
        build,
        name: args[1].clone(),
        group: "com.example".into(),
        java_release,
        versions: TemplateVersions::fallback(java_release),
    };
    let created = create_project(std::path::Path::new(&args[0]), &spec)?;
    println!("created {}", created.root.display());
    let info = detect_project(&created.root).expect("detectable");
    println!("kind: {}  port: {:?}", info.kind_label(), info.server_port);
    for task in project_tasks(&info) {
        println!("{:?}\t{}\t{}", task.group, task.label, task.command);
    }
    Ok(())
}
