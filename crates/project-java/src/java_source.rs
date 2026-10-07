//! Creating Java source files and packages: the kinds of type the IDE can
//! create, their templates, and where packages live (source roots).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
    AbstractClass,
}

impl JavaKind {
    pub const ALL: [JavaKind; 6] = [
        JavaKind::Class,
        JavaKind::Interface,
        JavaKind::Enum,
        JavaKind::Record,
        JavaKind::Annotation,
        JavaKind::AbstractClass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            JavaKind::Class => "Class",
            JavaKind::Interface => "Interface",
            JavaKind::Enum => "Enum",
            JavaKind::Record => "Record",
            JavaKind::Annotation => "Annotation",
            JavaKind::AbstractClass => "Abstract Class",
        }
    }

    /// The type's source text.
    pub fn template(self, package: Option<&str>, name: &str) -> String {
        let header = package.filter(|p| !p.is_empty()).map(|p| format!("package {p};\n\n")).unwrap_or_default();
        let body = match self {
            JavaKind::Class => format!("public class {name} {{\n\n}}\n"),
            JavaKind::Interface => format!("public interface {name} {{\n\n}}\n"),
            JavaKind::Enum => format!("public enum {name} {{\n\n}}\n"),
            JavaKind::Record => format!("public record {name}() {{\n\n}}\n"),
            JavaKind::Annotation => format!(
                "import java.lang.annotation.ElementType;\nimport java.lang.annotation.Retention;\nimport java.lang.annotation.RetentionPolicy;\nimport java.lang.annotation.Target;\n\n@Retention(RetentionPolicy.RUNTIME)\n@Target(ElementType.TYPE)\npublic @interface {name} {{\n\n}}\n"
            ),
            JavaKind::AbstractClass => format!("public abstract class {name} {{\n\n}}\n"),
        };
        format!("{header}{body}")
    }
}

/// The source root containing `dir`: the nearest `src/main/java`,
/// `src/test/java` (Maven/Gradle) or `src` (plain projects) at or above it.
pub fn source_root_for(dir: &Path) -> Option<PathBuf> {
    let mut maven_like = None;
    let mut plain = None;
    for ancestor in dir.ancestors() {
        let name = ancestor.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let parent = ancestor.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
        let grand = ancestor.parent().and_then(Path::parent).and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
        if name == "java" && (parent == "main" || parent == "test") && grand == "src" {
            maven_like = Some(ancestor.to_path_buf());
            break;
        }
        if name == "src" && plain.is_none() {
            plain = Some(ancestor.to_path_buf());
        }
    }
    maven_like.or(plain)
}

/// The package a folder corresponds to: `…/src/main/java/com/example` ->
/// `com.example` (`Some("")` for the source root itself).
pub fn package_for_dir(dir: &Path) -> Option<String> {
    let root = source_root_for(dir)?;
    let relative = dir.strip_prefix(&root).ok()?;
    Some(
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// Java keywords and literals, which can't name a type or package part.
const RESERVED: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class", "const",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float",
    "for", "goto", "if", "implements", "import", "instanceof", "int", "interface", "long", "native",
    "new", "package", "private", "protected", "public", "return", "short", "static", "strictfp",
    "super", "switch", "synchronized", "this", "throw", "throws", "transient", "try", "void",
    "volatile", "while", "true", "false", "null", "record", "var", "yield",
];

pub fn is_identifier(part: &str) -> bool {
    let mut chars = part.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !RESERVED.contains(&part)
}

/// Creates a Java type in `dir`. `name` may be qualified relative to the
/// folder's package (`service.UserService` creates the `service`
/// subpackage too). Returns the new file.
pub fn create_java_type(dir: &Path, kind: JavaKind, name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_end_matches(".java");
    let parts: Vec<&str> = name.split('.').collect();
    if name.is_empty() || parts.iter().any(|p| !is_identifier(p)) {
        return Err("Enter a valid Java name, e.g. UserService or service.UserService".into());
    }
    let (type_name, sub_packages) = parts.split_last().ok_or("Enter a name")?;
    let mut target_dir = dir.to_path_buf();
    for part in sub_packages {
        target_dir.push(part);
    }
    let file = target_dir.join(format!("{type_name}.java"));
    if file.exists() {
        return Err(format!("{type_name}.java already exists here"));
    }
    let package = package_for_dir(&target_dir);
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    std::fs::write(&file, kind.template(package.as_deref(), type_name)).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Creates a package (folders) under the source root of `dir`.
pub fn create_package(dir: &Path, package: &str) -> Result<PathBuf, String> {
    let package = package.trim().trim_matches('.');
    if package.is_empty() || package.split('.').any(|p| !is_identifier(p)) {
        return Err("Enter a valid package name, e.g. com.example.service".into());
    }
    let root = source_root_for(dir).unwrap_or_else(|| dir.to_path_buf());
    let path = package.split('.').fold(root, |path, part| path.join(part));
    if path.is_dir() {
        return Err(format!("Package {package} already exists"));
    }
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_follow_the_source_root() {
        let dir = Path::new("/p/src/main/java/com/example");
        assert_eq!(source_root_for(dir), Some(PathBuf::from("/p/src/main/java")));
        assert_eq!(package_for_dir(dir).as_deref(), Some("com.example"));
        assert_eq!(package_for_dir(Path::new("/p/src/main/java")).as_deref(), Some(""));
        assert_eq!(package_for_dir(Path::new("/plain/src/util")).as_deref(), Some("util"));
        assert_eq!(package_for_dir(Path::new("/p/docs")), None);
    }

    #[test]
    fn templates_and_names() {
        assert_eq!(JavaKind::Record.template(Some("a.b"), "Point"), "package a.b;\n\npublic record Point() {\n\n}\n");
        assert_eq!(JavaKind::Interface.template(None, "Shape"), "public interface Shape {\n\n}\n");
        assert!(JavaKind::Annotation.template(None, "Audit").contains("public @interface Audit"));
        assert!(is_identifier("UserService") && !is_identifier("class") && !is_identifier("9x"));
    }

    #[test]
    fn creates_types_and_packages_on_disk() {
        let base = std::env::temp_dir().join(format!("rji-java-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let pkg = base.join("src/main/java/com/example");
        std::fs::create_dir_all(&pkg).unwrap();

        let file = create_java_type(&pkg, JavaKind::Enum, "service.Status").unwrap();
        assert_eq!(file, pkg.join("service").join("Status.java"));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("package com.example.service;") && text.contains("public enum Status"));
        assert!(create_java_type(&pkg, JavaKind::Class, "service.Status").is_err());
        assert!(create_java_type(&pkg, JavaKind::Class, "bad name").is_err());

        let created = create_package(&pkg, "com.example.repository").unwrap();
        assert_eq!(created, base.join("src/main/java/com/example/repository"));
        assert!(create_package(&pkg, "com.example.repository").is_err());
        std::fs::remove_dir_all(&base).ok();
    }
}
