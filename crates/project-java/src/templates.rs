//! New-project templates: a console Java app, a Spring Boot web app, or a
//! JavaFX desktop app, built with Maven, Gradle (Kotlin DSL), or nothing
//! (plain `javac`, console only).
//!
//! Generated entirely offline from the text below. Library versions are
//! passed in (`TemplateVersions`) so the caller can look up the latest
//! stable releases first, with `TemplateVersions::fallback` when offline.
//! Gradle projects get no wrapper (its jar is a binary we don't ship), so
//! they need `gradle` on PATH, or `gradle wrapper` run once.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Console,
    SpringBootWeb,
    JavaFx,
}

impl Template {
    pub const ALL: [Template; 3] = [Template::Console, Template::SpringBootWeb, Template::JavaFx];

    pub fn label(self) -> &'static str {
        match self {
            Template::Console => "Java (console)",
            Template::SpringBootWeb => "Spring Boot (web)",
            Template::JavaFx => "JavaFX (desktop)",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Template::Console => "A command-line app with a main method and a unit test.",
            Template::SpringBootWeb => "A REST app on an embedded server, with a hello endpoint and DevTools reload.",
            Template::JavaFx => "A desktop window with a button and a label.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateBuild {
    Maven,
    Gradle,
    /// Plain sources compiled with `javac` (console template only).
    None,
}

impl TemplateBuild {
    pub fn label(self) -> &'static str {
        match self {
            TemplateBuild::Maven => "Maven",
            TemplateBuild::Gradle => "Gradle",
            TemplateBuild::None => "No build tool",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateVersions {
    pub spring_boot: String,
    pub javafx: String,
    pub junit: String,
}

impl TemplateVersions {
    /// Known-good versions for when Maven Central can't be reached.
    pub fn fallback(jdk_major: u32) -> Self {
        TemplateVersions {
            spring_boot: "4.1.1".into(),
            javafx: if jdk_major >= 25 { "25.0.1" } else { "21.0.5" }.into(),
            junit: if jdk_major >= 17 { "6.1.3" } else { "5.11.4" }.into(),
        }
    }
}

/// The JDK a JavaFX release needs: 21-22 need 17; from 23 on, two
/// releases behind (JavaFX 25 needs JDK 23).
pub fn javafx_min_jdk(javafx_major: u64) -> u64 {
    if javafx_major <= 22 { 17 } else { (javafx_major - 2).max(21) }
}

#[derive(Debug, Clone)]
pub struct NewProjectSpec {
    pub template: Template,
    pub build: TemplateBuild,
    /// Folder and artifact name, e.g. `demo-app`.
    pub name: String,
    /// Maven group / base package, e.g. `com.example`.
    pub group: String,
    /// `--release` level for javac (the detected JDK's major version).
    pub java_release: u32,
    pub versions: TemplateVersions,
}

#[derive(Debug, Clone)]
pub struct CreatedProject {
    pub root: PathBuf,
    /// The file to open first.
    pub main_file: PathBuf,
}

/// Checks a project name: letters, digits, `-`, `_`, `.`, starting with a
/// letter. Returns the problem, if any.
pub fn validate_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Enter a project name");
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return Some("The name must start with a letter");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return Some("Use only letters, digits, '-', '_' and '.'");
    }
    None
}

pub fn validate_group(group: &str) -> Option<&'static str> {
    if group.is_empty() {
        return Some("Enter a group (e.g. com.example)");
    }
    let ok = group.split('.').all(|part| {
        part.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    if ok { None } else { Some("The group must be a Java package name, e.g. com.example") }
}

/// `com.example` + `my-app` -> `com.example.myapp`.
pub fn package_name(group: &str, name: &str) -> String {
    let mut last: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if last.is_empty() || last.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        last = format!("app{last}");
    }
    if JAVA_KEYWORDS.contains(&last.as_str()) {
        last.push('_');
    }
    format!("{group}.{last}")
}

const JAVA_KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float",
    "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native",
    "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp",
    "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void",
    "volatile", "while", "true", "false", "null",
];

/// Writes the project into `parent/<name>`. Refuses to touch an existing,
/// non-empty folder.
pub fn create_project(parent: &Path, spec: &NewProjectSpec) -> anyhow::Result<CreatedProject> {
    if let Some(problem) = validate_name(&spec.name).or_else(|| validate_group(&spec.group)) {
        anyhow::bail!(problem);
    }
    anyhow::ensure!(
        !(spec.build == TemplateBuild::None && spec.template != Template::Console),
        "{} needs Maven or Gradle",
        spec.template.label()
    );
    let root = parent.join(&spec.name);
    if root.exists() && std::fs::read_dir(&root).map(|mut d| d.next().is_some()).unwrap_or(true) {
        anyhow::bail!("{} already exists and isn't empty", root.display());
    }
    let files = render_files(spec);
    for (rel, text) in &files {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, text)?;
    }
    let main_rel = files
        .iter()
        .map(|(rel, _)| rel)
        .find(|rel| rel.ends_with("App.java") || rel.ends_with("Application.java"))
        .cloned()
        .unwrap_or_default();
    Ok(CreatedProject { main_file: root.join(main_rel), root })
}

/// Every file of the project as (relative path, contents).
pub fn render_files(spec: &NewProjectSpec) -> Vec<(String, String)> {
    let pkg = package_name(&spec.group, &spec.name);
    let pkg_dir = pkg.replace('.', "/");
    let (main_dir, test_dir, resources) = match spec.build {
        TemplateBuild::None => ("src".to_string(), String::new(), String::new()),
        _ => ("src/main/java".to_string(), "src/test/java".to_string(), "src/main/resources".to_string()),
    };
    let mut files: Vec<(String, String)> = Vec::new();
    let class_name = if spec.template == Template::SpringBootWeb { "Application" } else { "App" };
    let main_class = format!("{pkg}.{class_name}");

    match spec.template {
        Template::Console => {
            files.push((format!("{main_dir}/{pkg_dir}/App.java"), console_app(&pkg, &spec.name)));
            if spec.build != TemplateBuild::None {
                files.push((format!("{test_dir}/{pkg_dir}/AppTest.java"), console_test(&pkg)));
            }
        }
        Template::SpringBootWeb => {
            files.push((format!("{main_dir}/{pkg_dir}/Application.java"), spring_app(&pkg)));
            files.push((format!("{main_dir}/{pkg_dir}/HelloController.java"), spring_controller(&pkg, &spec.name)));
            files.push((
                format!("{resources}/application.properties"),
                format!("spring.application.name={}\nserver.port=8080\n", spec.name),
            ));
            files.push((format!("{test_dir}/{pkg_dir}/ApplicationTests.java"), spring_test(&pkg)));
        }
        Template::JavaFx => {
            files.push((format!("{main_dir}/{pkg_dir}/App.java"), javafx_app(&pkg, &spec.name)));
        }
    }

    match spec.build {
        TemplateBuild::Maven => files.push(("pom.xml".into(), pom(spec, &main_class))),
        TemplateBuild::Gradle => {
            files.push(("build.gradle.kts".into(), gradle_build(spec, &main_class)));
            files.push(("settings.gradle.kts".into(), format!("rootProject.name = \"{}\"\n", spec.name)));
        }
        TemplateBuild::None => {}
    }
    files.push((".gitignore".into(), gitignore(spec.build)));
    files.push(("README.md".into(), readme(spec, &main_class)));
    files
}

fn console_app(pkg: &str, name: &str) -> String {
    format!(
        r#"package {pkg};

public class App {{

    public static String greeting(String who) {{
        return "Hello, " + who + "!";
    }}

    public static void main(String[] args) {{
        String who = args.length > 0 ? args[0] : "{name}";
        System.out.println(greeting(who));
    }}
}}
"#
    )
}

fn console_test(pkg: &str) -> String {
    format!(
        r#"package {pkg};

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class AppTest {{

    @Test
    void greetsByName() {{
        assertEquals("Hello, Ada!", App.greeting("Ada"));
    }}
}}
"#
    )
}

fn spring_app(pkg: &str) -> String {
    format!(
        r#"package {pkg};

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;

@SpringBootApplication
public class Application {{

    public static void main(String[] args) {{
        SpringApplication.run(Application.class, args);
    }}
}}
"#
    )
}

fn spring_controller(pkg: &str, name: &str) -> String {
    format!(
        r#"package {pkg};

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class HelloController {{

    @GetMapping("/")
    public String hello(@RequestParam(defaultValue = "world") String name) {{
        return "Hello, " + name + "! This is {name}.";
    }}
}}
"#
    )
}

fn spring_test(pkg: &str) -> String {
    format!(
        r#"package {pkg};

import org.junit.jupiter.api.Test;
import org.springframework.boot.test.context.SpringBootTest;

@SpringBootTest
class ApplicationTests {{

    @Test
    void contextLoads() {{
    }}
}}
"#
    )
}

fn javafx_app(pkg: &str, name: &str) -> String {
    format!(
        r#"package {pkg};

import javafx.application.Application;
import javafx.geometry.Insets;
import javafx.scene.Scene;
import javafx.scene.control.Button;
import javafx.scene.control.Label;
import javafx.scene.layout.VBox;
import javafx.stage.Stage;

public class App extends Application {{

    private int clicks = 0;

    @Override
    public void start(Stage stage) {{
        Label label = new Label("Hello from {name}!");
        Button button = new Button("Click me");
        button.setOnAction(event -> label.setText("Clicked " + (++clicks) + " time(s)"));

        VBox root = new VBox(12, label, button);
        root.setPadding(new Insets(24));

        stage.setTitle("{name}");
        stage.setScene(new Scene(root, 360, 200));
        stage.show();
    }}

    public static void main(String[] args) {{
        launch(args);
    }}
}}
"#
    )
}

fn pom(spec: &NewProjectSpec, main_class: &str) -> String {
    let NewProjectSpec { name, group, java_release, versions, .. } = spec;
    let header = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 https://maven.apache.org/xsd/maven-4.0.0.xsd">
    <modelVersion>4.0.0</modelVersion>
"#;
    match spec.template {
        Template::SpringBootWeb => {
            let web = spring_web_starter(&versions.spring_boot);
            format!(
                r#"{header}
    <parent>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-starter-parent</artifactId>
        <version>{boot}</version>
        <relativePath/>
    </parent>

    <groupId>{group}</groupId>
    <artifactId>{name}</artifactId>
    <version>0.0.1-SNAPSHOT</version>

    <properties>
        <java.version>{java_release}</java.version>
    </properties>

    <dependencies>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>{web}</artifactId>
        </dependency>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-devtools</artifactId>
            <scope>runtime</scope>
            <optional>true</optional>
        </dependency>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-starter-test</artifactId>
            <scope>test</scope>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.springframework.boot</groupId>
                <artifactId>spring-boot-maven-plugin</artifactId>
            </plugin>
        </plugins>
    </build>
</project>
"#,
                boot = versions.spring_boot,
            )
        }
        Template::JavaFx => format!(
            r#"{header}
    <groupId>{group}</groupId>
    <artifactId>{name}</artifactId>
    <version>1.0-SNAPSHOT</version>

    <properties>
        <maven.compiler.release>{java_release}</maven.compiler.release>
        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
        <javafx.version>{fx}</javafx.version>
    </properties>

    <dependencies>
        <dependency>
            <groupId>org.openjfx</groupId>
            <artifactId>javafx-controls</artifactId>
            <version>${{javafx.version}}</version>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-compiler-plugin</artifactId>
                <version>3.14.1</version>
            </plugin>
            <plugin>
                <groupId>org.openjfx</groupId>
                <artifactId>javafx-maven-plugin</artifactId>
                <version>0.0.8</version>
                <configuration>
                    <mainClass>{main_class}</mainClass>{fx_options}
                </configuration>
            </plugin>
        </plugins>
    </build>
</project>
"#,
            fx = versions.javafx,
            fx_options = if *java_release >= 22 {
                "
                    <options>
                        <option>--enable-native-access=javafx.graphics</option>
                    </options>"
            } else {
                ""
            },
        ),
        Template::Console => format!(
            r#"{header}
    <groupId>{group}</groupId>
    <artifactId>{name}</artifactId>
    <version>1.0-SNAPSHOT</version>

    <properties>
        <maven.compiler.release>{java_release}</maven.compiler.release>
        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>
        <exec.mainClass>{main_class}</exec.mainClass>
    </properties>

    <dependencies>
        <dependency>
            <groupId>org.junit.jupiter</groupId>
            <artifactId>junit-jupiter</artifactId>
            <version>{junit}</version>
            <scope>test</scope>
        </dependency>
    </dependencies>

    <build>
        <plugins>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-compiler-plugin</artifactId>
                <version>3.14.1</version>
            </plugin>
            <plugin>
                <groupId>org.apache.maven.plugins</groupId>
                <artifactId>maven-surefire-plugin</artifactId>
                <version>3.5.5</version>
            </plugin>
        </plugins>
    </build>
</project>
"#,
            junit = versions.junit,
        ),
    }
}

/// Spring Boot 4 renamed the servlet web starter to `-webmvc`.
fn spring_web_starter(boot_version: &str) -> &'static str {
    let major: u32 = boot_version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(4);
    if major >= 4 { "spring-boot-starter-webmvc" } else { "spring-boot-starter-web" }
}

fn gradle_build(spec: &NewProjectSpec, main_class: &str) -> String {
    let NewProjectSpec { group, java_release, versions, .. } = spec;
    let common_tail = format!(
        r#"
group = "{group}"
version = "1.0-SNAPSHOT"

repositories {{
    mavenCentral()
}}

tasks.withType<JavaCompile> {{
    options.release = {java_release}
    options.encoding = "UTF-8"
}}

tasks.withType<Test> {{
    useJUnitPlatform()
}}
"#
    );
    match spec.template {
        Template::SpringBootWeb => format!(
            r#"plugins {{
    java
    id("org.springframework.boot") version "{boot}"
    id("io.spring.dependency-management") version "1.1.7"
}}
{common_tail}
dependencies {{
    implementation("org.springframework.boot:{web}")
    developmentOnly("org.springframework.boot:spring-boot-devtools")
    testImplementation("org.springframework.boot:spring-boot-starter-test")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}}
"#,
            boot = versions.spring_boot,
            web = spring_web_starter(&versions.spring_boot),
        ),
        Template::JavaFx => format!(
            r#"plugins {{
    application
    id("org.openjfx.javafxplugin") version "0.1.0"
}}
{common_tail}
javafx {{
    version = "{fx}"
    modules = listOf("javafx.controls")
}}

application {{
    mainClass = "{main_class}"{fx_jvm}
}}
"#,
            fx = versions.javafx,
            fx_jvm = if *java_release >= 22 {
                "
    applicationDefaultJvmArgs = listOf(\"--enable-native-access=javafx.graphics\")"
            } else {
                ""
            },
        ),
        Template::Console => format!(
            r#"plugins {{
    application
}}
{common_tail}
dependencies {{
    testImplementation(platform("org.junit:junit-bom:{junit}"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}}

application {{
    mainClass = "{main_class}"
}}
"#,
            junit = versions.junit,
        ),
    }
}

fn gitignore(build: TemplateBuild) -> String {
    let mut text = String::from(".rji/\n.idea/\n.vscode/\n*.class\n");
    text.push_str(match build {
        TemplateBuild::Maven => "target/\n",
        TemplateBuild::Gradle => "build/\n.gradle/\n",
        TemplateBuild::None => "out/\n",
    });
    text
}

fn readme(spec: &NewProjectSpec, main_class: &str) -> String {
    let run = match (spec.build, spec.template) {
        (TemplateBuild::Maven, Template::SpringBootWeb) => "mvn spring-boot:run".to_string(),
        (TemplateBuild::Maven, Template::JavaFx) => "mvn javafx:run".to_string(),
        (TemplateBuild::Maven, Template::Console) => "mvn compile exec:java".to_string(),
        (TemplateBuild::Gradle, Template::SpringBootWeb) => "gradle bootRun".to_string(),
        (TemplateBuild::Gradle, _) => "gradle run".to_string(),
        (TemplateBuild::None, _) => format!("javac -d out <sources>  then  java -cp out {main_class}"),
    };
    let mut text = format!("# {}\n\n{}\n\nRun it with the ▶ Run button, or:\n\n    {run}\n", spec.name, spec.template.description());
    if spec.template == Template::SpringBootWeb {
        text.push_str("\nThen open http://localhost:8080/ (or `/?name=you`).\n");
    }
    if spec.build == TemplateBuild::Gradle {
        text.push_str("\nNeeds Gradle on PATH; run `gradle wrapper` once to pin a Gradle version for the project.\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{MainKind, default_run_task, detect_project};

    fn spec(template: Template, build: TemplateBuild) -> NewProjectSpec {
        NewProjectSpec {
            template,
            build,
            name: "demo-app".into(),
            group: "com.example".into(),
            java_release: 21,
            versions: TemplateVersions::fallback(21),
        }
    }

    #[test]
    fn names_and_packages() {
        assert_eq!(package_name("com.example", "demo-app"), "com.example.demoapp");
        assert_eq!(package_name("com.example", "123"), "com.example.app123");
        assert_eq!(package_name("com.example", "class"), "com.example.class_");
        assert!(validate_name("demo-app").is_none());
        assert!(validate_name("1demo").is_some());
        assert!(validate_name("my app").is_some());
        assert!(validate_group("com.example").is_none());
        assert!(validate_group("com..x").is_some());
    }

    #[test]
    fn every_template_creates_a_detectable_runnable_project() {
        let parent = std::env::temp_dir().join(format!("rji-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let cases = [
            (Template::Console, TemplateBuild::Maven, "exec:java", MainKind::Plain),
            (Template::Console, TemplateBuild::Gradle, " run", MainKind::Plain),
            (Template::Console, TemplateBuild::None, "java -cp out", MainKind::Plain),
            (Template::SpringBootWeb, TemplateBuild::Maven, "spring-boot:run", MainKind::SpringBoot),
            (Template::SpringBootWeb, TemplateBuild::Gradle, "bootRun", MainKind::SpringBoot),
            (Template::JavaFx, TemplateBuild::Maven, "javafx:run", MainKind::JavaFx),
            (Template::JavaFx, TemplateBuild::Gradle, " run", MainKind::JavaFx),
        ];
        for (i, (template, build, run_contains, kind)) in cases.into_iter().enumerate() {
            let mut s = spec(template, build);
            s.name = format!("demo{i}");
            let created = create_project(&parent, &s).unwrap();
            assert!(created.main_file.is_file(), "{:?}", created.main_file);
            let info = detect_project(&created.root).unwrap();
            assert_eq!(info.primary_main().map(|m| m.kind), Some(kind), "{template:?}/{build:?}");
            assert_eq!(info.spring_web, template == Template::SpringBootWeb);
            let run = default_run_task(&info).unwrap();
            assert!(run.command.contains(run_contains), "{template:?}/{build:?}: {}", run.command);
            // Never overwrites.
            assert!(create_project(&parent, &s).is_err());
        }
        assert!(create_project(&parent, &spec(Template::JavaFx, TemplateBuild::None)).is_err());
        std::fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn spring_boot_starter_follows_major_version() {
        assert_eq!(spring_web_starter("3.5.6"), "spring-boot-starter-web");
        assert_eq!(spring_web_starter("4.1.1"), "spring-boot-starter-webmvc");
        assert_eq!(javafx_min_jdk(21), 17);
        assert_eq!(javafx_min_jdk(25), 23);
    }
}
