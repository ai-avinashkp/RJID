//! Safe JDK discovery: never panics, never assumes exactly one JDK is
//! installed. Probes `JAVA_HOME`, `PATH`, and a handful of well-known install
//! roots, then reports every candidate found so a future JDK picker can let
//! the user choose rather than the IDE silently guessing wrong.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JdkSource {
    JavaHome,
    Path,
    WellKnownDir,
}

#[derive(Debug, Clone)]
pub struct JdkCandidate {
    pub home: PathBuf,
    /// Best-effort `java -version` output; `None` if the version probe
    /// itself failed (the candidate is still reported).
    pub version: Option<String>,
    pub source: JdkSource,
}

/// Finds every plausible JDK install. Errors probing any single location are
/// swallowed (logged to stderr) rather than propagated — a missing/odd
/// install must never prevent the IDE from starting.
pub fn detect_jdks() -> Vec<JdkCandidate> {
    let mut candidates: Vec<JdkCandidate> = Vec::new();

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let home = PathBuf::from(java_home);
        if has_javac(&home) {
            push_unique(&mut candidates, home, JdkSource::JavaHome);
        }
    }

    if let Some(home) = find_on_path() {
        push_unique(&mut candidates, home, JdkSource::Path);
    }

    for root in well_known_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && has_javac(&path) {
                push_unique(&mut candidates, path, JdkSource::WellKnownDir);
            }
        }
    }

    for candidate in &mut candidates {
        candidate.version = probe_version(&candidate.home);
    }

    candidates
}

fn push_unique(candidates: &mut Vec<JdkCandidate>, home: PathBuf, source: JdkSource) {
    let canonical = std::fs::canonicalize(&home).unwrap_or(home.clone());
    let already_known = candidates.iter().any(|c| {
        std::fs::canonicalize(&c.home).unwrap_or_else(|_| c.home.clone()) == canonical
    });
    if !already_known {
        candidates.push(JdkCandidate {
            home,
            version: None,
            source,
        });
    }
}

fn javac_path(jdk_home: &Path) -> PathBuf {
    jdk_home
        .join("bin")
        .join(format!("javac{}", std::env::consts::EXE_SUFFIX))
}

fn has_javac(jdk_home: &Path) -> bool {
    javac_path(jdk_home).is_file()
}

/// Looks for `javac` on `PATH` and, if found, returns its JDK home
/// (the parent of `bin/`).
fn find_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exe_name = format!("javac{}", std::env::consts::EXE_SUFFIX);
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(&exe_name);
        if candidate.is_file() {
            return dir.parent().map(Path::to_path_buf);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn well_known_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\Java"),
        PathBuf::from(r"C:\Program Files\Eclipse Adoptium"),
        PathBuf::from(r"C:\Program Files\Microsoft"),
        PathBuf::from(r"C:\Program Files (x86)\Java"),
    ]
}

#[cfg(target_os = "macos")]
fn well_known_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/Library/Java/JavaVirtualMachines")]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn well_known_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/lib/jvm")]
}

fn probe_version(jdk_home: &Path) -> Option<String> {
    let output = Command::new(javac_path(jdk_home)).arg("-version").output();
    match output {
        Ok(output) => {
            // `javac -version` prints to stdout; older `java -version` prints
            // to stderr — check both so we don't miss it either way.
            let text = if !output.stdout.is_empty() {
                output.stdout
            } else {
                output.stderr
            };
            let text = String::from_utf8_lossy(&text).trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        }
        Err(_) => None,
    }
}
