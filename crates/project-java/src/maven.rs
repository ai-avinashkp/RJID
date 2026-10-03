//! Lightweight Maven `pom.xml` detection — just enough to power a Run/Build
//! toolbar and pick a sensible default goal, not a full POM model.

use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct MavenProject {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub has_spring_boot_plugin: bool,
    pub has_javafx_plugin: bool,
    /// Whether `spring-boot-devtools` is declared as a dependency — distinct
    /// from `has_spring_boot_plugin` (the Maven plugin that adds the `run`
    /// goal). DevTools is what actually restarts the app when it sees
    /// `target/classes` change, which is what the IDE's recompile-on-save
    /// hook depends on being present to have any effect.
    pub has_devtools: bool,
}

/// Reads `<workspace_root>/pom.xml`, if present, and extracts the project's
/// own coordinates plus a couple of plugin-presence heuristics used to offer
/// `spring-boot:run` / `javafx:run` in the UI. Returns `None` (not an error)
/// when there is no `pom.xml` or it can't be parsed — a missing/odd POM must
/// never block opening the folder in the IDE.
pub fn detect_maven_project(workspace_root: &Path) -> Option<MavenProject> {
    let pom_path = workspace_root.join("pom.xml");
    let text = std::fs::read_to_string(&pom_path).ok()?;
    let doc = roxmltree::Document::parse(&text).ok()?;
    let project_el = doc.root_element();

    let direct_child_text = |name: &str| -> Option<String> {
        project_el
            .children()
            .find(|n| n.is_element() && n.has_tag_name(name))
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    Some(MavenProject {
        group_id: direct_child_text("groupId"),
        artifact_id: direct_child_text("artifactId"),
        version: direct_child_text("version"),
        has_spring_boot_plugin: text.contains("spring-boot-maven-plugin"),
        has_javafx_plugin: text.contains("javafx-maven-plugin"),
        has_devtools: text.contains("spring-boot-devtools"),
    })
}
