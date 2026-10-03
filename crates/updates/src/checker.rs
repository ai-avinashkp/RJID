//! One update check: what's installed / pinned vs. the latest stable
//! release of each component. Blocking (network + `mvn -v`); run it on a
//! background thread.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::project::{Component, VersionSite, find_sites};
use crate::sources;
use crate::version::{Bump, Version, classify, latest_patch, latest_stable};

#[derive(Debug, Clone)]
pub struct UpdateItem {
    pub component: Component,
    pub current: String,
    pub latest: String,
    pub bump: Bump,
    /// Where the project pins this version (editable in place), if it does.
    pub site: Option<VersionSite>,
    /// Newest release on the current `major.minor` line, when that's newer
    /// than `current` — the only update ever applied automatically.
    pub safe_patch: Option<String>,
    /// For things the IDE never installs itself (JDK, system Maven).
    pub download_url: Option<String>,
}

impl UpdateItem {
    /// `"spring-boot:3.5.0"` — what "Ignore this version" records.
    pub fn ignore_key(&self) -> String {
        format!("{}:{}", self.component.key(), self.latest)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    pub items: Vec<UpdateItem>,
    /// Sources that couldn't be reached (offline, timeouts) — the check
    /// still reports everything else.
    pub errors: Vec<String>,
}

/// Versions already on this machine, as the IDE detected them.
#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// Raw `java -version`-style string (e.g. `java version "25.0.2" …`).
    pub jdk: Option<String>,
    pub maven: Option<String>,
}

/// `java version "21.0.2" 2024-01-16 LTS` / `javac 25.0.2` -> the version.
pub fn jdk_version_from_banner(banner: &str) -> Option<String> {
    let quoted = banner.split('"').nth(1).unwrap_or(banner);
    quoted
        .split_whitespace()
        .find_map(Version::parse)
        .map(|v| v.raw)
}

/// `mvn -v` -> `3.9.6`, or `None` if Maven isn't on PATH.
pub fn detect_system_maven() -> Option<String> {
    let program = if cfg!(windows) { "mvn.cmd" } else { "mvn" };
    let mut command = Command::new(program);
    command.arg("-v").stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().find(|l| l.contains("Apache Maven"))?;
    first
        .split_whitespace()
        .nth(2)
        .and_then(Version::parse)
        .map(|v| v.raw)
}

pub fn check(workspace: Option<&Path>, installed: &Installed) -> CheckReport {
    let mut report = CheckReport::default();
    let sites = workspace.map(find_sites).unwrap_or_default();

    // Project-pinned components (Maven Central).
    for site in &sites {
        let (group, artifact) = match site.component {
            Component::SpringBoot => ("org.springframework.boot", "spring-boot"),
            Component::JavaFx => ("org.openjfx", "javafx-controls"),
            Component::MavenWrapper => ("org.apache.maven", "apache-maven"),
            _ => continue,
        };
        match sources::maven_central_versions(group, artifact) {
            Ok(versions) => {
                let versions: Vec<&str> = versions.iter().map(String::as_str).collect();
                if let Some(item) = compare(site.component, &site.current, &versions, Some(site.clone()), None) {
                    report.items.push(item);
                }
            }
            Err(err) => report.errors.push(format!("{}: {err:#}", site.component.label())),
        }
    }

    // Machine-installed Maven (only when the project doesn't pin one).
    let has_wrapper = sites.iter().any(|s| s.component == Component::MavenWrapper);
    if !has_wrapper && let Some(current) = installed.maven.as_deref() {
        match sources::maven_central_versions("org.apache.maven", "apache-maven") {
            Ok(versions) => {
                let versions: Vec<&str> = versions.iter().map(String::as_str).collect();
                if let Some(item) = compare(
                    Component::Maven,
                    current,
                    &versions,
                    None,
                    Some(sources::MAVEN_DOWNLOAD_URL.to_string()),
                ) {
                    report.items.push(item);
                }
            }
            Err(err) => report.errors.push(format!("Maven: {err:#}")),
        }
    }

    // JDK: newest update of the installed major, else a newer LTS.
    if let Some(current) = installed.jdk.as_deref().and_then(jdk_version_from_banner)
        && let Some(current_v) = Version::parse(&current)
    {
        let major = current_v.major();
        let same_major = sources::adoptium_latest_for_major(major);
        let releases = sources::adoptium_available_releases();
        match (same_major, releases) {
            (Ok(latest_same), Ok((lts, _feature))) => {
                let mut candidates: Vec<String> = latest_same.into_iter().collect();
                if lts > major
                    && let Ok(Some(newest_lts)) = sources::adoptium_latest_for_major(lts)
                {
                    candidates.push(newest_lts);
                }
                let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
                if let Some(mut item) = compare(Component::Jdk, &current, &refs, None, None) {
                    let target = Version::parse(&item.latest).map_or(major, |v| v.major());
                    item.download_url = Some(sources::jdk_download_url(target));
                    report.items.push(item);
                }
            }
            (Err(err), _) | (_, Err(err)) => report.errors.push(format!("JDK: {err:#}")),
        }
    }
    report
}

/// Builds an item if a newer stable release exists.
fn compare(
    component: Component,
    current: &str,
    versions: &[&str],
    site: Option<VersionSite>,
    download_url: Option<String>,
) -> Option<UpdateItem> {
    let current_v = Version::parse(current)?;
    let latest = latest_stable(versions.iter().copied())?;
    let bump = classify(&current_v, &latest)?;
    let safe_patch = latest_patch(versions.iter().copied(), &current_v).map(|v| v.raw);
    Some(UpdateItem {
        component,
        current: current.to_string(),
        latest: latest.raw,
        bump,
        site,
        safe_patch,
        download_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jdk_banner() {
        assert_eq!(jdk_version_from_banner("java version \"25.0.2\" 2026-01-20 LTS").as_deref(), Some("25.0.2"));
        assert_eq!(jdk_version_from_banner("openjdk version \"21.0.5\" 2024-10-15").as_deref(), Some("21.0.5"));
        assert_eq!(jdk_version_from_banner("17.0.9").as_deref(), Some("17.0.9"));
        // What the IDE's own JDK detection reports.
        assert_eq!(jdk_version_from_banner("javac 25.0.2").as_deref(), Some("25.0.2"));
    }

    #[test]
    fn compare_offers_latest_and_safe_patch() {
        let versions = ["3.3.0", "3.3.5", "3.4.1", "3.5.0-RC1"];
        let item = compare(Component::SpringBoot, "3.3.0", &versions, None, None).unwrap();
        assert_eq!((item.latest.as_str(), item.bump), ("3.4.1", Bump::Minor));
        assert_eq!(item.safe_patch.as_deref(), Some("3.3.5"));
        assert!(compare(Component::SpringBoot, "3.4.1", &versions, None, None).is_none());
    }

    /// Live network check against the real sources. Run explicitly:
    /// `cargo test -p rji-updates -- --ignored live`
    #[test]
    #[ignore]
    fn live_sources() {
        let versions = sources::maven_central_versions("org.springframework.boot", "spring-boot").unwrap();
        assert!(latest_stable(versions.iter().map(String::as_str)).unwrap().major() >= 3);
        let (lts, _) = sources::adoptium_available_releases().unwrap();
        assert!(lts >= 21);
        assert!(sources::adoptium_latest_for_major(21).unwrap().unwrap().starts_with("21."));
        let report = check(None, &Installed { jdk: Some("java version \"21.0.1\"".into()), maven: None });
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let jdk = report.items.iter().find(|i| i.component == Component::Jdk).unwrap();
        println!("{jdk:?}");
        assert!(jdk.download_url.is_some());
    }
}
