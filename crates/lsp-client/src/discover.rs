//! Finds an existing Eclipse JDT Language Server (jdtls) rather than
//! bundling one: either a standalone install pointed to by `JDTLS_HOME`
//! (download from https://download.eclipse.org/jdtls/), or the copy that
//! ships inside the "Language Support for Java" extension (`redhat.java`)
//! of VS Code, VS Code Insiders, VSCodium, or Cursor.
//!
//! Safe/fallible like `project_java::jdk_detect`: returns `None`, never an
//! error, when nothing is found — Java intelligence is then unavailable,
//! not a startup failure.

use std::path::{Path, PathBuf};

pub struct JdtlsInstall {
    pub server_dir: PathBuf,
}

/// Whether `dir` is a usable jdtls server folder: plugins with the Equinox
/// launcher, and the shared configuration for this OS.
pub fn is_server_dir(dir: &Path) -> bool {
    dir.join("plugins").is_dir() && shared_config_dir(dir).is_dir() && find_equinox_launcher(dir).is_some()
}

pub fn discover_jdtls() -> Option<JdtlsInstall> {
    discover_jdtls_with(None)
}

/// Like [`discover_jdtls`], also checking a copy the IDE downloaded itself
/// (`managed`), after `JDTLS_HOME` and before editor extensions.
pub fn discover_jdtls_with(managed: Option<&Path>) -> Option<JdtlsInstall> {
    if let Some(dir) = std::env::var_os("JDTLS_HOME").map(PathBuf::from)
        && is_server_dir(&dir)
    {
        return Some(JdtlsInstall { server_dir: dir });
    }
    if let Some(dir) = managed.filter(|d| is_server_dir(d)) {
        return Some(JdtlsInstall { server_dir: dir.to_path_buf() });
    }
    // Lets the first-run download flow be tried on a machine that has an
    // editor's copy (as a new user without one would see it).
    if std::env::var_os("RJID_NO_EDITOR_JDTLS").is_some() {
        return None;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    let mut candidates: Vec<PathBuf> = [".vscode", ".vscode-insiders", ".vscode-oss", ".cursor"]
        .iter()
        .filter_map(|editor| std::fs::read_dir(home.join(editor).join("extensions")).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("redhat.java-"))
        })
        .map(|path| path.join("server"))
        .filter(|server| is_server_dir(server))
        .collect();
    // Newest extension version wins (numeric compare: 1.10 > 1.9).
    candidates.sort_by_key(|server| extension_version(server));
    candidates.pop().map(|server_dir| JdtlsInstall { server_dir })
}

/// `…/redhat.java-1.56.0-win32-x64/server` -> `[1, 56, 0]`.
fn extension_version(server_dir: &Path) -> Vec<u64> {
    server_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("redhat.java-"))
        .map(|rest| {
            rest.split('-')
                .next()
                .unwrap_or("")
                .split('.')
                .map_while(|part| part.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The shared OSGi configuration directory jdtls needs, which differs by OS.
pub fn shared_config_dir(server_dir: &Path) -> PathBuf {
    let os = if cfg!(windows) {
        "config_win"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") && server_dir.join("config_mac_arm").is_dir() {
            "config_mac_arm"
        } else {
            "config_mac"
        }
    } else if cfg!(target_arch = "aarch64") && server_dir.join("config_linux_arm").is_dir() {
        "config_linux_arm"
    } else {
        "config_linux"
    };
    server_dir.join(os)
}

pub fn find_equinox_launcher(server_dir: &Path) -> Option<PathBuf> {
    let plugins_dir = server_dir.join("plugins");
    std::fs::read_dir(&plugins_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                name.starts_with("org.eclipse.equinox.launcher_") && name.ends_with(".jar")
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_extension_version_sorts_numerically() {
        let a = PathBuf::from("/x/redhat.java-1.9.0-win32-x64/server");
        let b = PathBuf::from("/x/redhat.java-1.10.0-win32-x64/server");
        assert!(extension_version(&b) > extension_version(&a));
    }
}
