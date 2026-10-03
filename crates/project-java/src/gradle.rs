//! Lightweight Gradle build-script detection — just enough to power a
//! Run/Build toolbar, mirroring `maven.rs`. Gradle build scripts are Groovy
//! or Kotlin DSL, not a simple markup format, so unlike the POM reader this
//! doesn't parse a structured tree — it works off targeted substring
//! heuristics, which is good enough for plugin-presence checks and a
//! project name.

use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct GradleProject {
    pub project_name: Option<String>,
    pub has_spring_boot_plugin: bool,
    pub has_javafx_plugin: bool,
    /// Whether `spring-boot-devtools` is declared as a dependency — see the
    /// identical field on `MavenProject` for why this is distinct from
    /// `has_spring_boot_plugin`.
    pub has_devtools: bool,
    /// The project's Gradle wrapper (`gradlew.bat` on Windows, `gradlew`
    /// elsewhere) if present in the workspace root, else a bare `gradle`
    /// on PATH.
    pub wrapper_command: String,
}

/// Reads `<workspace_root>/build.gradle[.kts]`, if present, and a sibling
/// `settings.gradle[.kts]` for the project name. Returns `None` (not an
/// error) when neither build script exists — mirrors
/// `detect_maven_project`'s "report what's found, never assume" posture. A
/// workspace has at most one build system in practice, so the caller tries
/// this after (or instead of) the Maven detector rather than merging them.
pub fn detect_gradle_project(workspace_root: &Path) -> Option<GradleProject> {
    let build_script = ["build.gradle.kts", "build.gradle"]
        .iter()
        .map(|name| workspace_root.join(name))
        .find(|p| p.exists())?;
    let text = std::fs::read_to_string(&build_script).unwrap_or_default();

    let settings_text = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .map(|name| workspace_root.join(name))
        .find_map(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();

    let project_name = extract_root_project_name(&settings_text).or_else(|| {
        workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });

    let wrapper = if cfg!(windows) { "gradlew.bat" } else { "gradlew" };
    let wrapper_command = if workspace_root.join(wrapper).exists() {
        wrapper.to_string()
    } else {
        "gradle".to_string()
    };

    Some(GradleProject {
        project_name,
        has_spring_boot_plugin: text.contains("org.springframework.boot"),
        has_javafx_plugin: text.contains("org.openjfx.javafxplugin"),
        has_devtools: text.contains("spring-boot-devtools"),
        wrapper_command,
    })
}

/// Pulls `rootProject.name = "foo"` (Kotlin DSL) or `rootProject.name = 'foo'`
/// / `rootProject.name 'foo'` (Groovy DSL) out of a settings script's text —
/// a plain string search rather than a real parser, which is all a project
/// display name needs.
fn extract_root_project_name(settings_text: &str) -> Option<String> {
    let idx = settings_text.find("rootProject.name")?;
    let after = &settings_text[idx + "rootProject.name".len()..];
    let quote_char = after.chars().find(|c| *c == '\'' || *c == '"')?;
    let start = after.find(quote_char)? + 1;
    let end = after[start..].find(quote_char)? + start;
    let name = after[start..end].trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_spring_boot_gradle_project() {
        let dir = std::env::temp_dir().join(format!(
            "rji-gradle-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.gradle.kts"),
            "rootProject.name = \"sample-gradle-app\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("build.gradle.kts"),
            "plugins {\n    id(\"java\")\n    id(\"org.springframework.boot\") version \"3.3.0\"\n}\n",
        )
        .unwrap();

        let project = detect_gradle_project(&dir).expect("should detect gradle project");
        assert_eq!(project.project_name.as_deref(), Some("sample-gradle-app"));
        assert!(project.has_spring_boot_plugin);
        assert!(!project.has_javafx_plugin);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn returns_none_without_a_build_script() {
        let dir = std::env::temp_dir().join(format!(
            "rji-gradle-test-empty-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(detect_gradle_project(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
