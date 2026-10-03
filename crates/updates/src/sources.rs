//! Where "latest version" information comes from — official, key-free
//! endpoints only: Maven Central's `maven-metadata.xml` (Maven, Spring
//! Boot, JavaFX) and the Eclipse Adoptium API (JDKs). Every request has a
//! timeout; callers run these off the UI thread.

use std::time::Duration;

use anyhow::{Context, anyhow};
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(12);
/// Large enough for any metadata file, small enough to bound memory.
const MAX_BODY: u64 = 4 * 1024 * 1024;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(format!("RJID/{} (update check)", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn get(url: &str) -> anyhow::Result<String> {
    let mut response = agent().get(url).call().with_context(|| format!("GET {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()
        .with_context(|| format!("reading {url}"))
}

/// All published versions of a Maven Central artifact.
pub fn maven_central_versions(group: &str, artifact: &str) -> anyhow::Result<Vec<String>> {
    let url = format!(
        "https://repo1.maven.org/maven2/{}/{artifact}/maven-metadata.xml",
        group.replace('.', "/")
    );
    parse_maven_metadata(&get(&url)?)
}

pub fn parse_maven_metadata(xml: &str) -> anyhow::Result<Vec<String>> {
    let doc = roxmltree::Document::parse(xml).context("parsing maven-metadata.xml")?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("version"))
        .filter(|n| n.parent().is_some_and(|p| p.has_tag_name("versions")))
        .filter_map(|n| n.text().map(|t| t.trim().to_string()))
        .collect())
}

/// `(most recent LTS, most recent feature release)` JDK major versions.
pub fn adoptium_available_releases() -> anyhow::Result<(u64, u64)> {
    let json: Value = serde_json::from_str(&get("https://api.adoptium.net/v3/info/available_releases")?)?;
    parse_available_releases(&json)
}

pub fn parse_available_releases(json: &Value) -> anyhow::Result<(u64, u64)> {
    let lts = json["most_recent_lts"].as_u64().ok_or_else(|| anyhow!("no most_recent_lts"))?;
    let feature = json["most_recent_feature_release"].as_u64().unwrap_or(lts);
    Ok((lts, feature))
}

/// Latest GA Temurin JDK of one major version for this OS/CPU, as
/// `major.minor.security` (e.g. `21.0.5`).
pub fn adoptium_latest_for_major(major: u64) -> anyhow::Result<Option<String>> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };
    let url = format!(
        "https://api.adoptium.net/v3/assets/latest/{major}/hotspot?image_type=jdk&os={os}&architecture={arch}"
    );
    let json: Value = serde_json::from_str(&get(&url)?)?;
    Ok(parse_latest_assets(&json))
}

pub fn parse_latest_assets(json: &Value) -> Option<String> {
    let version = &json.as_array()?.first()?["version"];
    let (major, minor, security) = (
        version["major"].as_u64()?,
        version["minor"].as_u64().unwrap_or(0),
        version["security"].as_u64().unwrap_or(0),
    );
    Some(format!("{major}.{minor}.{security}"))
}

/// Download page for a Temurin JDK major version.
pub fn jdk_download_url(major: u64) -> String {
    format!("https://adoptium.net/temurin/releases/?version={major}")
}

pub const MAVEN_DOWNLOAD_URL: &str = "https://maven.apache.org/download.cgi";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_and_adoptium_shapes() {
        let xml = r#"<metadata><groupId>g</groupId><versioning><latest>3.4.0</latest>
            <versions><version>3.3.0</version><version>3.4.0</version></versions></versioning></metadata>"#;
        assert_eq!(parse_maven_metadata(xml).unwrap(), vec!["3.3.0", "3.4.0"]);

        let releases = serde_json::json!({"most_recent_lts": 25, "most_recent_feature_release": 25});
        assert_eq!(parse_available_releases(&releases).unwrap(), (25, 25));

        let assets = serde_json::json!([{ "version": { "major": 21, "minor": 0, "security": 5, "semver": "21.0.5+11" } }]);
        assert_eq!(parse_latest_assets(&assets).as_deref(), Some("21.0.5"));
        assert_eq!(parse_latest_assets(&serde_json::json!([])), None);
    }
}
