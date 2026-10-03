//! Updating the IDE itself, securely:
//!
//! 1. A release *manifest* (version, notes, one binary per platform with its
//!    SHA-256 and size) is published together with an **Ed25519 signature**.
//!    The app only trusts a manifest that verifies against the public key
//!    compiled into it — a compromised server or mirror can't push a build.
//! 2. The binary is downloaded to a `.part` file with a hard size limit and
//!    hashed while streaming; it's only kept if size and SHA-256 match the
//!    signed manifest exactly.
//! 3. It's staged next to the executable as `<exe>.new` and swapped in at
//!    the next start (the running exe is renamed to `<exe>.old`, which
//!    Windows allows), with rollback if the swap fails half-way.
//!
//! Publishing a release: `rji-release keygen` once (keep the secret key
//! offline), then `rji-release hash <binary>` / `rji-release sign
//! <manifest.json> <secret.key>` per release, and build the app with
//! `RJI_UPDATE_URL` / `RJI_UPDATE_PUBKEY` set.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::version::Version;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// `<arch>-<os>`, e.g. `x86_64-windows` (see [`current_target`]).
    pub target: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the binary.
    pub sha256: String,
    pub size: u64,
}

/// What's published: the manifest's exact JSON text and its signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    pub manifest: String,
    /// Base64 Ed25519 signature over `manifest`'s UTF-8 bytes.
    pub signature: String,
}

pub fn current_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// New signing keypair: `(secret, public)`, both base64.
pub fn keygen() -> (String, String) {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    (B64.encode(key.to_bytes()), B64.encode(key.verifying_key().to_bytes()))
}

/// Ed25519-signs `text` (base64 secret key); returns the base64 signature.
pub fn sign_text(text: &str, secret_b64: &str) -> anyhow::Result<String> {
    let bytes: [u8; 32] = B64
        .decode(secret_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!("secret key must be 32 bytes"))?;
    Ok(B64.encode(SigningKey::from_bytes(&bytes).sign(text.as_bytes()).to_bytes()))
}

/// Checks an Ed25519 signature over `text` against a base64 public key.
pub fn verify_signed_text(text: &str, signature_b64: &str, public_b64: &str) -> anyhow::Result<()> {
    let key_bytes: [u8; 32] = B64
        .decode(public_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!("public key must be 32 bytes"))?;
    let key = VerifyingKey::from_bytes(&key_bytes).context("invalid public key")?;
    let sig_bytes: [u8; 64] = B64
        .decode(signature_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!("signature must be 64 bytes"))?;
    key.verify(text.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .map_err(|_| anyhow!("signature is invalid — refusing it"))
}

pub fn sign_manifest(manifest_json: &str, secret_b64: &str) -> anyhow::Result<SignedManifest> {
    // Reject a manifest that wouldn't parse back, before signing it.
    serde_json::from_str::<ReleaseManifest>(manifest_json).context("manifest is not valid")?;
    Ok(SignedManifest {
        manifest: manifest_json.to_string(),
        signature: sign_text(manifest_json, secret_b64)?,
    })
}

/// Verifies the signature *before* parsing anything inside the manifest.
pub fn verify_manifest(signed: &SignedManifest, public_b64: &str) -> anyhow::Result<ReleaseManifest> {
    verify_signed_text(&signed.manifest, &signed.signature, public_b64)
        .context("update manifest")?;
    serde_json::from_str(&signed.manifest).context("parsing release manifest")
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!("RJID/{} (updates)", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// GETs a signed document `{"<field>": "<text>", "signature": "…"}` and
/// returns `<text>` only if its signature verifies.
pub fn fetch_signed_text(url: &str, field: &str, public_b64: &str) -> anyhow::Result<String> {
    let body = agent(Duration::from_secs(15))
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()?;
    let doc: serde_json::Value = serde_json::from_str(&body).context("parsing signed document")?;
    let text = doc[field].as_str().ok_or_else(|| anyhow!("signed document has no `{field}`"))?;
    let signature = doc["signature"].as_str().ok_or_else(|| anyhow!("signed document has no signature"))?;
    verify_signed_text(text, signature, public_b64)?;
    Ok(text.to_string())
}

/// Fetches and verifies the release manifest.
pub fn fetch_manifest(url: &str, public_b64: &str) -> anyhow::Result<ReleaseManifest> {
    let text = fetch_signed_text(url, "manifest", public_b64).context("update manifest")?;
    serde_json::from_str(&text).context("parsing release manifest")
}

/// The asset for this platform, if the manifest is newer than `current`.
pub fn applicable_asset<'m>(manifest: &'m ReleaseManifest, current: &str) -> Option<&'m ReleaseAsset> {
    let newer = Version::parse(&manifest.version)? > Version::parse(current)?;
    let target = current_target();
    newer.then(|| manifest.assets.iter().find(|a| a.target == target)).flatten()
}

/// Where a downloaded update waits for the next start.
pub fn staged_path(exe: &Path) -> PathBuf {
    sibling(exe, ".new")
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Downloads `url` to `dest`, keeping it only if its size and SHA-256
/// match exactly (checked while streaming, with a hard size cap).
pub fn download_verified(url: &str, sha256: &str, size: u64, dest: &Path) -> anyhow::Result<()> {
    let part = sibling(dest, ".part");
    let result = (|| -> anyhow::Result<()> {
        let mut response = agent(Duration::from_secs(600))
            .get(url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        // One byte over the limit is enough to detect an oversized body.
        let mut reader = response.body_mut().with_config().limit(size + 1).reader();
        let mut file = std::fs::File::create(&part).context("creating download file")?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if total >= size => bail!("download is larger than the signed size ({err})"),
                Err(err) => return Err(err).context("downloading"),
            };
            total += n as u64;
            ensure!(total <= size, "download is larger than the signed size");
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
        }
        file.sync_all()?;
        ensure!(total == size, "download size {total} doesn't match the signed size {size}");
        ensure!(
            hex(&hasher.finalize()).eq_ignore_ascii_case(sha256.trim()),
            "downloaded file's checksum doesn't match the signed manifest — discarded"
        );
        Ok(())
    })();
    if let Err(err) = result {
        let _ = std::fs::remove_file(&part);
        return Err(err);
    }
    std::fs::rename(&part, dest).context("saving download")
}

/// Downloads `asset` and stages it as `<exe>.new`. Nothing is staged unless
/// size and SHA-256 match the signed manifest.
pub fn download_and_stage(asset: &ReleaseAsset, exe: &Path) -> anyhow::Result<PathBuf> {
    let staged = staged_path(exe);
    download_verified(&asset.url, &asset.sha256, asset.size, &staged)?;
    Ok(staged)
}

/// At startup: if an update is staged, swap it in. Returns `true` if the
/// executable on disk changed (the caller should relaunch). On failure the
/// original is restored.
pub fn apply_staged(exe: &Path) -> anyhow::Result<bool> {
    let staged = staged_path(exe);
    if !staged.is_file() {
        return Ok(false);
    }
    let old = sibling(exe, ".old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).context("moving current executable aside")?;
    if let Err(err) = std::fs::rename(&staged, exe) {
        // Roll back so the user is never left without a working binary.
        let _ = std::fs::rename(&old, exe);
        return Err(err).context("installing update (rolled back)");
    }
    Ok(true)
}

/// Removes the previous version left behind by a successful swap (it can't
/// be deleted while it's still the running process, so this runs later).
pub fn cleanup_previous(exe: &Path) {
    let _ = std::fs::remove_file(sibling(exe, ".old"));
}

pub fn sha256_file(path: &Path) -> anyhow::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let size = std::io::copy(&mut file, &mut hasher)?;
    Ok((hex(&hasher.finalize()), size))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn manifest_for(binary: &[u8], url: &str) -> String {
        let digest = hex(&Sha256::digest(binary));
        serde_json::to_string(&ReleaseManifest {
            version: "9.9.9".into(),
            notes: "test".into(),
            assets: vec![ReleaseAsset {
                target: current_target(),
                url: url.into(),
                sha256: digest,
                size: binary.len() as u64,
            }],
        })
        .unwrap()
    }

    #[test]
    fn signature_round_trip_and_tamper_detection() {
        let (secret, public) = keygen();
        let json = manifest_for(b"bin", "http://x/bin");
        let signed = sign_manifest(&json, &secret).unwrap();
        assert_eq!(verify_manifest(&signed, &public).unwrap().version, "9.9.9");

        let mut tampered = signed.clone();
        tampered.manifest = tampered.manifest.replace("9.9.9", "9.9.8");
        assert!(verify_manifest(&tampered, &public).is_err());

        let (_, other_public) = keygen();
        assert!(verify_manifest(&signed, &other_public).is_err());
    }

    #[test]
    fn only_newer_versions_for_this_platform_apply() {
        let manifest: ReleaseManifest = serde_json::from_str(&manifest_for(b"x", "u")).unwrap();
        assert!(applicable_asset(&manifest, "0.1.0").is_some());
        assert!(applicable_asset(&manifest, "9.9.9").is_none());
        let mut other = manifest.clone();
        other.assets[0].target = "sparc-plan9".into();
        assert!(applicable_asset(&other, "0.1.0").is_none());
    }

    #[test]
    fn end_to_end_fetch_download_stage_and_swap() {
        let (secret, public) = keygen();
        let binary = b"NEW-BINARY-CONTENT".repeat(1000);
        let good_url_path = "/rji-new".to_string();
        let bad_url_path = "/rji-tampered".to_string();
        let mut tampered = binary.clone();
        tampered[10] ^= 0xff;

        let base = serve_with_manifest(&secret, &binary, &tampered, &good_url_path, &bad_url_path);

        let manifest = fetch_manifest(&format!("{base}/manifest.json"), &public).unwrap();
        let asset = applicable_asset(&manifest, "0.1.0").unwrap().clone();

        let dir = std::env::temp_dir().join(format!("rji-selfupdate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rji.exe");
        std::fs::write(&exe, b"OLD-BINARY").unwrap();

        // A swapped-in tampered file is rejected and nothing is staged.
        let mut bad = asset.clone();
        bad.url = format!("{base}{bad_url_path}");
        assert!(download_and_stage(&bad, &exe).is_err());
        assert!(!staged_path(&exe).exists());

        // The genuine file stages, then swaps in on "restart".
        download_and_stage(&asset, &exe).unwrap();
        assert!(apply_staged(&exe).unwrap());
        assert_eq!(std::fs::read(&exe).unwrap(), binary);
        assert_eq!(std::fs::read(dir.join("rji.exe.old")).unwrap(), b"OLD-BINARY");
        assert!(!apply_staged(&exe).unwrap(), "nothing left to apply");
        cleanup_previous(&exe);
        assert!(!dir.join("rji.exe.old").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A tiny local HTTP server (no network needed) serving a signed
    /// manifest that points at `good`, plus a tampered copy at `bad_path`.
    fn serve_with_manifest(secret: &str, good: &[u8], bad: &[u8], good_path: &str, bad_path: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let json = manifest_for(good, &format!("{base}{good_path}"));
        let signed = serde_json::to_vec(&sign_manifest(&json, secret).unwrap()).unwrap();
        let routes = vec![
            ("/manifest.json".to_string(), signed),
            (good_path.to_string(), good.to_vec()),
            (bad_path.to_string(), bad.to_vec()),
        ];
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() <= 2 {
                        break;
                    }
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut stream = stream;
                let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone()).unwrap_or_default();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        base
    }
}
