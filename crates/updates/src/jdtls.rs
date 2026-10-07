//! Downloads the Eclipse JDT Language Server (jdtls) for users who don't
//! have one (no VS Code Java extension, no `JDTLS_HOME`).
//!
//! jdtls is EPL-2.0 and is fetched from the Eclipse Foundation's own
//! download server at the user's request, never bundled. Safety:
//! - only HTTPS URLs under `download.eclipse.org/jdtls/` are used;
//! - the archive must match the SHA-256 Eclipse publishes next to it
//!   (integrity; authenticity rests on HTTPS to eclipse.org);
//! - size is capped, and tar entries that would land outside the target
//!   folder are refused;
//! - it's unpacked into a staging folder, validated by the caller, and
//!   only then renamed into place and recorded in `current`, so a failed
//!   or interrupted install never replaces a working one.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use sha2::{Digest, Sha256};

const BASE: &str = "https://download.eclipse.org/jdtls";
/// The archive is ~50 MB; anything far bigger is not what we asked for.
const MAX_ARCHIVE: u64 = 300 * 1024 * 1024;
/// How many minor versions below the snapshot to look for a milestone.
const MILESTONE_PROBES: u64 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdtlsRelease {
    /// e.g. `1.61.0`
    pub version: String,
    /// e.g. `jdt-language-server-1.61.0-202609031315.tar.gz`
    pub file: String,
    pub url: String,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!("RJID/{} (jdtls download)", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

fn get_small(url: &str) -> anyhow::Result<String> {
    let mut response = agent(Duration::from_secs(20)).get(url).call().with_context(|| format!("GET {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .with_context(|| format!("reading {url}"))
}

/// Validates a `latest.txt` body: one archive name of the expected shape,
/// e.g. `jdt-language-server-1.61.0-202609031315.tar.gz`. Returns
/// (file name, version). Anything else (an HTML error page, a path) is
/// rejected.
pub fn parse_latest_txt(body: &str) -> Option<(String, String)> {
    let file = body.trim();
    let rest = file.strip_prefix("jdt-language-server-")?.strip_suffix(".tar.gz")?;
    let (version, stamp) = rest.rsplit_once('-')?;
    let version_ok = !version.is_empty() && version.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let stamp_ok = stamp.len() >= 8 && stamp.chars().all(|c| c.is_ascii_digit());
    (version_ok && stamp_ok).then(|| (file.to_string(), version.to_string()))
}

/// The newest stable (milestone) release: the snapshot channel names the
/// version under development; milestones are probed from there downward.
pub fn latest_milestone() -> anyhow::Result<JdtlsRelease> {
    let snapshot = get_small(&format!("{BASE}/snapshots/latest.txt"))?;
    let (_, dev_version) = parse_latest_txt(&snapshot).context("unexpected jdtls snapshot listing")?;
    let parts: Vec<u64> = dev_version.split('.').filter_map(|p| p.parse().ok()).collect();
    let (major, minor) = (parts.first().copied().unwrap_or(1), parts.get(1).copied().unwrap_or(0));
    for m in (minor.saturating_sub(MILESTONE_PROBES)..=minor).rev() {
        let version = format!("{major}.{m}.0");
        let Ok(body) = get_small(&format!("{BASE}/milestones/{version}/latest.txt")) else { continue };
        if let Some((file, found)) = parse_latest_txt(&body)
            && found == version
        {
            let url = format!("{BASE}/milestones/{version}/{file}");
            return Ok(JdtlsRelease { version, file, url });
        }
    }
    bail!("couldn't find a stable jdtls release on download.eclipse.org")
}

/// The SHA-256 Eclipse publishes as `<archive>.sha256`.
pub fn fetch_sha256(release: &JdtlsRelease) -> anyhow::Result<String> {
    let body = get_small(&format!("{}.sha256", release.url))?;
    let hash = body.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    ensure!(hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()), "unexpected checksum file");
    Ok(hash)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Streams `url` to `dest`, reporting (downloaded, total) bytes, and checks
/// the SHA-256. Leaves nothing behind on failure.
pub fn download_checked(url: &str, sha256: &str, dest: &Path, progress: &dyn Fn(u64, Option<u64>)) -> anyhow::Result<()> {
    ensure!(url.starts_with(&format!("{BASE}/")), "refusing to download from {url}");
    let result = (|| -> anyhow::Result<()> {
        let mut response = agent(Duration::from_secs(900)).get(url).call().with_context(|| format!("GET {url}"))?;
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(total) = total {
            ensure!(total <= MAX_ARCHIVE, "archive is unexpectedly large ({total} bytes)");
        }
        let mut reader = response.body_mut().with_config().limit(MAX_ARCHIVE + 1).reader();
        let mut file = std::fs::File::create(dest).context("creating download file")?;
        let mut hasher = Sha256::new();
        let mut done: u64 = 0;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).context("downloading")?;
            if n == 0 {
                break;
            }
            done += n as u64;
            ensure!(done <= MAX_ARCHIVE, "archive is unexpectedly large");
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            progress(done, total);
        }
        file.sync_all()?;
        if let Some(total) = total {
            ensure!(done == total, "download was cut short ({done} of {total} bytes)");
        }
        ensure!(
            hex(&hasher.finalize()).eq_ignore_ascii_case(sha256.trim()),
            "checksum mismatch — the download was discarded"
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// Unpacks a `.tar.gz` into `dest`, refusing entries that would escape it
/// (`..`, absolute paths) and anything but files, folders and symlinks
/// inside the tree.
pub fn extract_tar_gz(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    let file = std::fs::File::open(archive).context("opening archive")?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    for entry in tar.entries().context("reading archive")? {
        let mut entry = entry.context("reading archive entry")?;
        let path = entry.path().context("archive entry path")?.into_owned();
        ensure!(
            path.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)),
            "archive entry escapes its folder: {}",
            path.display()
        );
        // `unpack_in` also refuses anything that would land outside `dest`.
        ensure!(entry.unpack_in(dest).context("unpacking")?, "archive entry escapes its folder: {}", path.display());
    }
    Ok(())
}

/// The managed install recorded in `<root>/current`, if it still exists.
pub fn installed(root: &Path) -> Option<PathBuf> {
    let version = std::fs::read_to_string(root.join("current")).ok()?;
    let version = version.trim();
    ensure_plain_name(version).ok()?;
    let dir = root.join(version);
    dir.is_dir().then_some(dir)
}

fn ensure_plain_name(name: &str) -> anyhow::Result<()> {
    ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
        "bad version name {name:?}"
    );
    Ok(())
}

/// Downloads, verifies and installs `release` under `root`
/// (`<root>/<version>/`), then points `<root>/current` at it. `validate`
/// checks the unpacked folder really is a jdtls server before it's used.
pub fn install(
    release: &JdtlsRelease,
    root: &Path,
    validate: &dyn Fn(&Path) -> bool,
    progress: &dyn Fn(u64, Option<u64>),
) -> anyhow::Result<PathBuf> {
    ensure_plain_name(&release.version)?;
    std::fs::create_dir_all(root).context("creating the jdtls folder")?;
    let target = root.join(&release.version);
    if target.is_dir() && validate(&target) {
        write_current(root, &release.version)?;
        return Ok(target);
    }
    let sha256 = fetch_sha256(release)?;
    let archive = root.join(format!(".download-{}", release.file));
    download_checked(&release.url, &sha256, &archive, progress)?;

    let staging = root.join(format!(".staging-{}", release.version));
    let _ = std::fs::remove_dir_all(&staging);
    let unpacked = extract_tar_gz(&archive, &staging);
    let _ = std::fs::remove_file(&archive);
    if let Err(err) = unpacked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    if !validate(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("the downloaded archive doesn't contain a usable language server");
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).context("installing jdtls")?;
    write_current(root, &release.version)?;
    remove_old_versions(root, &release.version);
    Ok(target)
}

fn write_current(root: &Path, version: &str) -> anyhow::Result<()> {
    let tmp = root.join("current.tmp");
    std::fs::write(&tmp, version)?;
    std::fs::rename(&tmp, root.join("current")).context("recording the installed version")
}

/// Best-effort cleanup of older versions and leftovers.
fn remove_old_versions(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_version = name.chars().next().is_some_and(|c| c.is_ascii_digit());
        let is_leftover = name.starts_with(".staging-") || name.starts_with(".download-");
        if entry.path().is_dir() && name != keep && (is_version || is_leftover) {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if is_leftover {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_well_formed_listings() {
        assert_eq!(
            parse_latest_txt("jdt-language-server-1.61.0-202609031315.tar.gz\n"),
            Some(("jdt-language-server-1.61.0-202609031315.tar.gz".into(), "1.61.0".into()))
        );
        assert_eq!(parse_latest_txt("<!DOCTYPE html><html>Not Found"), None);
        assert_eq!(parse_latest_txt("jdt-language-server-1.61.0-202609031315.zip"), None);
        assert_eq!(parse_latest_txt("../jdt-language-server-1.61.0-202609031315.tar.gz"), None);
        assert_eq!(parse_latest_txt("jdt-language-server-1.x.0-202609031315.tar.gz"), None);
    }

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            // `append_data` would reject `..`; write the raw name to test the guard.
            let name = path.as_bytes();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extracts_and_refuses_escaping_entries() {
        let root = std::env::temp_dir().join(format!("rji-jdtls-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let good = root.join("good.tar.gz");
        std::fs::write(&good, tar_gz(&[("plugins/a.jar", b"jar"), ("config_win/config.ini", b"x")])).unwrap();
        let out = root.join("out");
        extract_tar_gz(&good, &out).unwrap();
        assert_eq!(std::fs::read(out.join("plugins/a.jar")).unwrap(), b"jar");

        let evil = root.join("evil.tar.gz");
        std::fs::write(&evil, tar_gz(&[("../escaped.txt", b"pwned")])).unwrap();
        assert!(extract_tar_gz(&evil, &root.join("out2")).is_err());
        assert!(!root.join("escaped.txt").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn current_pointer_round_trips_and_rejects_paths() {
        let root = std::env::temp_dir().join(format!("rji-jdtls-cur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("1.61.0")).unwrap();
        write_current(&root, "1.61.0").unwrap();
        assert_eq!(installed(&root), Some(root.join("1.61.0")));
        std::fs::write(root.join("current"), "../../etc").unwrap();
        assert_eq!(installed(&root), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuses_foreign_urls() {
        let dest = std::env::temp_dir().join("rji-jdtls-foreign");
        assert!(download_checked("https://example.com/x.tar.gz", "00", &dest, &|_, _| {}).is_err());
    }
}
