//! What kind of Java project a folder is, and the Build / Run / Test tasks
//! that make sense for it.
//!
//! `detect_project` combines the build tool (Maven, Gradle, or none), the
//! frameworks declared in the build script (Spring Boot, Spring web,
//! JavaFX, Android), and a scan of the sources for entry points
//! (`main` methods, `@SpringBootApplication`, `extends Application`).
//! `project_tasks` turns that into shell commands for the terminal. A plain
//! Java folder without a build tool still gets Build/Run via `javac`/`java`.
//!
//! Commands are typed into the integrated terminal (PowerShell on
//! Windows), so `-D…` arguments are quoted: PowerShell would otherwise split
//! `-Dexec.mainClass=a.b.C` at the dots.

use std::path::{Path, PathBuf};

use crate::gradle::{GradleProject, detect_gradle_project};
use crate::maven::{MavenProject, detect_maven_project};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSystem {
    Maven,
    Gradle,
    /// Plain `.java` sources compiled with `javac`.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainKind {
    Plain,
    SpringBoot,
    JavaFx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainClass {
    /// Fully-qualified class name.
    pub class_name: String,
    pub path: PathBuf,
    pub kind: MainKind,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub build: BuildSystem,
    pub name: String,
    pub maven: Option<MavenProject>,
    pub gradle: Option<GradleProject>,
    pub spring_boot: bool,
    /// An embedded web server is on the classpath (`spring-boot-starter-web`,
    /// `-webmvc`, `-webflux`, `-jersey`). Without one, a Spring Boot app
    /// starts, runs its runners and exits; it never listens on a port.
    pub spring_web: bool,
    pub javafx: bool,
    pub android: bool,
    /// `server.port` from `application.properties`/`.yml`, else 8080 for
    /// web apps. `Some(0)` means a random port.
    pub server_port: Option<u16>,
    /// Entry points, best first (Spring Boot, JavaFX, then `App`/`Main`).
    pub main_classes: Vec<MainClass>,
    /// Sources root for a project without a build tool (`src` or `.`).
    pub plain_source_dir: Option<String>,
    /// Maven command: the project's wrapper if present, else `mvn`.
    pub maven_cmd: String,
}

impl ProjectInfo {
    /// Short label for the toolbar/status bar, e.g. "Spring Boot · Web".
    pub fn kind_label(&self) -> String {
        let framework = if self.android {
            "Android"
        } else if self.spring_boot && self.spring_web {
            "Spring Boot · Web"
        } else if self.spring_boot {
            "Spring Boot"
        } else if self.javafx {
            "JavaFX"
        } else {
            "Java"
        };
        match self.build {
            BuildSystem::Maven => format!("{framework} · Maven"),
            BuildSystem::Gradle => format!("{framework} · Gradle"),
            BuildSystem::None => framework.to_string(),
        }
    }

    pub fn primary_main(&self) -> Option<&MainClass> {
        self.main_classes.first()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskGroup {
    Build,
    Run,
    Test,
    Clean,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub label: String,
    /// Compact toolbar label.
    pub short: &'static str,
    pub command: String,
    pub group: TaskGroup,
}

/// Directories never scanned for sources.
const SKIP_DIRS: &[&str] = &[
    "target", "build", "out", "bin", "node_modules", ".git", ".gradle", ".idea", ".vscode",
    ".rji", ".mvn", ".settings",
];
/// Scan budget: big repos must not stall opening a folder.
const MAX_SCANNED_FILES: usize = 3000;
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Classifies `root`. Returns `None` only when the folder has neither a
/// build script nor any `.java` file (i.e. it isn't a Java project).
pub fn detect_project(root: &Path) -> Option<ProjectInfo> {
    let maven = detect_maven_project(root);
    let gradle = if maven.is_none() { detect_gradle_project(root) } else { None };
    let build_text = build_script_text(root);
    let build = if maven.is_some() {
        BuildSystem::Maven
    } else if gradle.is_some() {
        BuildSystem::Gradle
    } else {
        BuildSystem::None
    };

    let source_roots: Vec<PathBuf> = match build {
        BuildSystem::None => {
            let src = root.join("src");
            vec![if src.is_dir() { src } else { root.to_path_buf() }]
        }
        _ => vec![root.join("src").join("main").join("java"), root.join("src").join("main").join("kotlin")],
    };
    let mut main_classes = Vec::new();
    let mut budget = MAX_SCANNED_FILES;
    let mut any_java = false;
    for dir in &source_roots {
        scan_sources(dir, &mut main_classes, &mut budget, &mut any_java);
    }
    if build == BuildSystem::None && !any_java {
        return None;
    }
    main_classes.sort_by_key(|m| {
        let kind_rank = match m.kind {
            MainKind::SpringBoot => 0,
            MainKind::JavaFx => 1,
            MainKind::Plain => 2,
        };
        let simple = m.class_name.rsplit('.').next().unwrap_or("");
        let name_rank = if matches!(simple, "App" | "Main" | "Application") { 0 } else { 1 };
        (kind_rank, name_rank, m.class_name.len())
    });

    let spring_boot = build_text.contains("org.springframework.boot")
        || main_classes.iter().any(|m| m.kind == MainKind::SpringBoot);
    let spring_web = [
        "spring-boot-starter-web",
        "spring-boot-starter-webmvc",
        "spring-boot-starter-webflux",
        "spring-boot-starter-jersey",
    ]
    .iter()
    .any(|starter| build_text.contains(starter));
    let javafx = build_text.contains("org.openjfx")
        || main_classes.iter().any(|m| m.kind == MainKind::JavaFx);
    let android = build_text.contains("com.android.application") || build_text.contains("com.android.library");
    let server_port = if spring_web { Some(read_server_port(root).unwrap_or(8080)) } else { None };

    let name = maven
        .as_ref()
        .and_then(|m| m.artifact_id.clone())
        .or_else(|| gradle.as_ref().and_then(|g| g.project_name.clone()))
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    let maven_cmd = maven_command(root);
    let plain_source_dir = (build == BuildSystem::None)
        .then(|| if root.join("src").is_dir() { "src".to_string() } else { ".".to_string() });

    Some(ProjectInfo {
        root: root.to_path_buf(),
        build,
        name,
        maven,
        gradle,
        spring_boot,
        spring_web,
        javafx,
        android,
        server_port,
        main_classes,
        plain_source_dir,
        maven_cmd,
    })
}

/// `.\mvnw.cmd` / `./mvnw` when the project ships the Maven wrapper.
fn maven_command(root: &Path) -> String {
    if cfg!(windows) && root.join("mvnw.cmd").is_file() {
        ".\\mvnw.cmd".into()
    } else if !cfg!(windows) && root.join("mvnw").is_file() {
        "./mvnw".into()
    } else {
        "mvn".into()
    }
}

fn gradle_command(project: &GradleProject) -> String {
    if project.wrapper_command == "gradle" {
        project.wrapper_command.clone()
    } else if cfg!(windows) {
        format!(".\\{}", project.wrapper_command)
    } else {
        format!("./{}", project.wrapper_command)
    }
}

fn build_script_text(root: &Path) -> String {
    let mut text = String::new();
    for name in ["pom.xml", "build.gradle.kts", "build.gradle"] {
        if let Ok(s) = std::fs::read_to_string(root.join(name)) {
            text.push_str(&s);
        }
    }
    // Android modules usually live in `app/`.
    for name in ["app/build.gradle.kts", "app/build.gradle"] {
        if let Ok(s) = std::fs::read_to_string(root.join(name)) {
            text.push_str(&s);
        }
    }
    text
}

fn scan_sources(dir: &Path, found: &mut Vec<MainClass>, budget: &mut usize, any_java: &mut bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if *budget == 0 {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) && !name.starts_with('.') {
                scan_sources(&path, found, budget, any_java);
            }
        } else if name.ends_with(".java") {
            *budget -= 1;
            *any_java = true;
            if entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES)
                && let Ok(text) = std::fs::read_to_string(&path)
                && let Some(kind) = main_kind(&text)
            {
                let stem = name.trim_end_matches(".java");
                let class_name = match package_of(&text) {
                    Some(pkg) => format!("{pkg}.{stem}"),
                    None => stem.to_string(),
                };
                found.push(MainClass { class_name, path, kind });
            }
        }
    }
}

/// Whether a source file is an entry point, and which kind.
pub fn main_kind(text: &str) -> Option<MainKind> {
    let code = strip_comments(text);
    if code.contains("@SpringBootApplication") {
        return Some(MainKind::SpringBoot);
    }
    let has_main = code.contains("static void main(") || code.contains("static void main (");
    let fx = code.contains("extends Application")
        && (code.contains("javafx.application") || code.contains("javafx.stage"));
    if fx {
        return Some(MainKind::JavaFx);
    }
    has_main.then_some(MainKind::Plain)
}

pub fn package_of(text: &str) -> Option<String> {
    strip_comments(text).lines().find_map(|line| {
        let rest = line.trim().strip_prefix("package ")?;
        let pkg = rest.trim().trim_end_matches(';').trim();
        (!pkg.is_empty()).then(|| pkg.to_string())
    })
}

/// Removes `//` and `/* */` comments (string contents are left alone; a
/// `//` inside a string literal only ever hides the rest of that line,
/// which can't create a false entry point).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
        } else if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else {
            let ch = rest.chars().next().unwrap_or(' ');
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// `server.port` from `src/main/resources/application.properties` or
/// `application.yml`/`.yaml`.
fn read_server_port(root: &Path) -> Option<u16> {
    let resources = root.join("src").join("main").join("resources");
    if let Ok(text) = std::fs::read_to_string(resources.join("application.properties"))
        && let Some(port) = text.lines().find_map(|line| {
            let (key, value) = line.split_once(['=', ':'])?;
            (key.trim() == "server.port").then(|| value.trim().parse().ok()).flatten()
        })
    {
        return Some(port);
    }
    for name in ["application.yml", "application.yaml"] {
        if let Ok(text) = std::fs::read_to_string(resources.join(name))
            && let Some(port) = yaml_server_port(&text)
        {
            return Some(port);
        }
    }
    None
}

/// Finds `port:` nested directly under a top-level `server:` key.
fn yaml_server_port(text: &str) -> Option<u16> {
    let mut in_server = false;
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            in_server = line.trim_end() == "server:";
            if let Some(v) = line.strip_prefix("server.port:") {
                return v.trim().parse().ok();
            }
            continue;
        }
        if in_server && let Some(v) = line.trim().strip_prefix("port:") {
            return v.trim().trim_matches(['"', '\'']).parse().ok();
        }
    }
    None
}

/// Build/Run/Test/Clean commands for the project, best Run task first
/// among the Run group.
pub fn project_tasks(info: &ProjectInfo) -> Vec<Task> {
    let mut tasks = Vec::new();
    let mut push = |id: &str, label: String, short: &'static str, command: String, group: TaskGroup| {
        tasks.push(Task { id: id.to_string(), label, short, command, group });
    };
    match info.build {
        BuildSystem::Maven => {
            let mvn = &info.maven_cmd;
            let plugins = info.maven.as_ref();
            let boot_plugin = plugins.is_some_and(|m| m.has_spring_boot_plugin);
            let fx_plugin = plugins.is_some_and(|m| m.has_javafx_plugin);
            if boot_plugin && info.spring_boot {
                push("run-boot", "Run Spring Boot".into(), "Boot", format!("{mvn} spring-boot:run"), TaskGroup::Run);
            }
            if fx_plugin {
                push("run-fx", "Run JavaFX".into(), "FX", format!("{mvn} javafx:run"), TaskGroup::Run);
            }
            for (i, main) in info.main_classes.iter().take(5).enumerate() {
                let covered = (main.kind == MainKind::SpringBoot && boot_plugin)
                    || (main.kind == MainKind::JavaFx && fx_plugin);
                if covered {
                    continue;
                }
                push(
                    &format!("run-main-{i}"),
                    format!("Run {}", simple_name(&main.class_name)),
                    "Run",
                    format!("{mvn} compile exec:java \"-Dexec.mainClass={}\"", main.class_name),
                    TaskGroup::Run,
                );
            }
            push("build", "Build".into(), "Build", format!("{mvn} package -DskipTests"), TaskGroup::Build);
            push("test", "Test".into(), "Test", format!("{mvn} test"), TaskGroup::Test);
            push("clean", "Clean".into(), "Clean", format!("{mvn} clean"), TaskGroup::Clean);
        }
        BuildSystem::Gradle => {
            let Some(project) = info.gradle.as_ref() else { return tasks };
            let gradle = gradle_command(project);
            let text = build_script_text(&info.root);
            let application = ["id(\"application\")", "id 'application'", "id \"application\"", "plugin: 'application'"]
                .iter()
                .any(|p| text.contains(p))
                // Kotlin DSL shorthand: `plugins { application }`.
                || text.lines().any(|l| l.trim() == "application")
                || project.has_javafx_plugin;
            if project.has_spring_boot_plugin {
                push("run-boot", "Run Spring Boot".into(), "Boot", format!("{gradle} bootRun"), TaskGroup::Run);
            } else if application {
                let label = if info.javafx { "Run JavaFX" } else { "Run" };
                let short = if info.javafx { "FX" } else { "Run" };
                push("run-app", label.into(), short, format!("{gradle} run"), TaskGroup::Run);
            } else if let Some(main) = info.primary_main() {
                let classes = Path::new("build").join("classes").join("java").join("main");
                push(
                    "run-main-0",
                    format!("Run {}", simple_name(&main.class_name)),
                    "Run",
                    then(&format!("{gradle} classes"), &format!("java -cp {} {}", classes.display(), main.class_name)),
                    TaskGroup::Run,
                );
            }
            push("build", "Build".into(), "Build", format!("{gradle} build -x test"), TaskGroup::Build);
            push("test", "Test".into(), "Test", format!("{gradle} test"), TaskGroup::Test);
            push("clean", "Clean".into(), "Clean", format!("{gradle} clean"), TaskGroup::Clean);
        }
        BuildSystem::None => {
            let src = info.plain_source_dir.as_deref().unwrap_or(".");
            let compile = javac_all(src);
            for (i, main) in info.main_classes.iter().take(5).enumerate() {
                push(
                    &format!("run-main-{i}"),
                    format!("Run {}", simple_name(&main.class_name)),
                    "Run",
                    then(&compile, &format!("java -cp out {}", main.class_name)),
                    TaskGroup::Run,
                );
            }
            push("build", "Build".into(), "Build", compile, TaskGroup::Build);
            let clean = if cfg!(windows) {
                "Remove-Item -Recurse -Force out -ErrorAction SilentlyContinue".to_string()
            } else {
                "rm -rf out".to_string()
            };
            push("clean", "Clean".into(), "Clean", clean, TaskGroup::Clean);
        }
    }
    tasks
}

/// The run task the ▶ button uses when the project has no run config.
pub fn default_run_task(info: &ProjectInfo) -> Option<Task> {
    project_tasks(info).into_iter().find(|t| t.group == TaskGroup::Run)
}

fn simple_name(class_name: &str) -> &str {
    class_name.rsplit('.').next().unwrap_or(class_name)
}

/// `a`, then `b` only if `a` succeeded, in the terminal's shell.
fn then(a: &str, b: &str) -> String {
    if cfg!(windows) { format!("{a}; if ($?) {{ {b} }}") } else { format!("{a} && {b}") }
}

/// Compiles every `.java` file under `src` into `out/`.
fn javac_all(src: &str) -> String {
    if cfg!(windows) {
        format!("javac -d out (Get-ChildItem -Recurse -Filter *.java -Path {src} | ForEach-Object FullName)")
    } else {
        format!("javac -d out $(find {src} -name '*.java')")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rji-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn detects_entry_point_kinds() {
        assert_eq!(main_kind("@SpringBootApplication\npublic class A {}"), Some(MainKind::SpringBoot));
        assert_eq!(
            main_kind("import javafx.application.Application;\nclass A extends Application {}"),
            Some(MainKind::JavaFx)
        );
        assert_eq!(main_kind("class A { public static void main(String[] a) {} }"), Some(MainKind::Plain));
        assert_eq!(main_kind("// public static void main(\nclass A {}"), None);
        assert_eq!(package_of("/* x */\npackage com.ex.app;\n").as_deref(), Some("com.ex.app"));
    }

    #[test]
    fn spring_boot_web_maven_project() {
        let root = temp_dir("boot");
        write(
            &root,
            "pom.xml",
            "<project><artifactId>demo</artifactId><dependencies><dependency><artifactId>spring-boot-starter-webmvc</artifactId></dependency></dependencies>\
             <build><plugins><plugin><groupId>org.springframework.boot</groupId><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
        );
        write(&root, "src/main/java/com/ex/DemoApplication.java", "package com.ex;\n@SpringBootApplication\npublic class DemoApplication { public static void main(String[] a) {} }");
        write(&root, "src/main/resources/application.yml", "spring:\n  application:\n    name: demo\nserver:\n  port: 9090\n");
        let info = detect_project(&root).unwrap();
        assert!(info.spring_boot && info.spring_web);
        assert_eq!(info.server_port, Some(9090));
        assert_eq!(info.kind_label(), "Spring Boot · Web · Maven");
        let run = default_run_task(&info).unwrap();
        assert_eq!(run.command, "mvn spring-boot:run");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spring_boot_without_web_starter_has_no_port() {
        let root = temp_dir("boot-noweb");
        write(&root, "pom.xml", "<project><artifactId>x</artifactId><dependencies><dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter</artifactId></dependency></dependencies></project>");
        let info = detect_project(&root).unwrap();
        assert!(info.spring_boot && !info.spring_web);
        assert_eq!(info.server_port, None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn plain_maven_project_runs_main_via_exec() {
        let root = temp_dir("plain-mvn");
        write(&root, "pom.xml", "<project><artifactId>cli</artifactId></project>");
        write(&root, "src/main/java/com/ex/Tool.java", "package com.ex;\nclass Tool { static void main(String[] a) {} }");
        write(&root, "src/main/java/com/ex/App.java", "package com.ex;\npublic class App { public static void main(String[] a) {} }");
        let info = detect_project(&root).unwrap();
        assert_eq!(info.kind_label(), "Java · Maven");
        assert_eq!(info.primary_main().unwrap().class_name, "com.ex.App");
        let run = default_run_task(&info).unwrap();
        assert_eq!(run.command, "mvn compile exec:java \"-Dexec.mainClass=com.ex.App\"");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn folder_without_build_tool_uses_javac() {
        let root = temp_dir("plain");
        write(&root, "src/Main.java", "public class Main { public static void main(String[] a) {} }");
        let info = detect_project(&root).unwrap();
        assert_eq!(info.build, BuildSystem::None);
        let run = default_run_task(&info).unwrap();
        assert!(run.command.starts_with("javac -d out"), "{}", run.command);
        assert!(run.command.contains("java -cp out Main"), "{}", run.command);
        std::fs::remove_dir_all(&root).ok();
        let empty = temp_dir("empty");
        assert!(detect_project(&empty).is_none());
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn reads_properties_port() {
        let root = temp_dir("port");
        write(&root, "src/main/resources/application.properties", "spring.application.name=x\nserver.port = 8181\n");
        assert_eq!(read_server_port(&root), Some(8181));
        std::fs::remove_dir_all(&root).ok();
    }
}
