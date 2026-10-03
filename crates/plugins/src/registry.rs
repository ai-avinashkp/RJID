//! The plugin registry: a signed index of published plugins, used to
//! install new plugins and update installed ones.
//!
//! The index is published as `{"index": "<json>", "signature": "<ed25519>"}`
//! (same signing scheme and `rji-release` tooling as IDE updates). Every
//! plugin file listed in it carries a SHA-256 and size, verified while
//! downloading. A downloaded plugin is loaded and validated (manifest, API
//! version, no imports) *before* it replaces the installed copy, and the
//! swap rolls back on failure — so an update can never leave a plugin
//! half-installed.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::host::{Plugin, valid_id};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryIndex {
    pub plugins: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// `plugin.json`, the `.wasm` module, and anything else in the folder.
    pub files: Vec<RegistryFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryFile {
    /// Plain file name inside the plugin folder.
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// Fetches the index and verifies its signature before parsing it.
pub fn fetch_index(url: &str, public_key_b64: &str) -> anyhow::Result<RegistryIndex> {
    let text = rji_updates::fetch_signed_text(url, "index", public_key_b64).context("plugin registry")?;
    serde_json::from_str(&text).context("parsing plugin registry index")
}

/// Registry entries that are newer than what's installed.
pub fn updates_available<'a>(index: &'a RegistryIndex, installed: &[&Plugin]) -> Vec<&'a RegistryEntry> {
    index
        .plugins
        .iter()
        .filter(|entry| {
            installed.iter().any(|p| {
                p.manifest.id == entry.id
                    && rji_updates::Version::parse(&entry.version) > rji_updates::Version::parse(&p.manifest.version)
            })
        })
        .collect()
}

/// Downloads, verifies, and validates a plugin, then installs it as
/// `<plugins_dir>/<id>` (replacing an older copy atomically).
pub fn install(entry: &RegistryEntry, plugins_dir: &Path) -> anyhow::Result<PathBuf> {
    ensure!(valid_id(&entry.id), "registry entry has an invalid id `{}`", entry.id);
    for file in &entry.files {
        ensure!(
            !file.name.is_empty() && !file.name.contains(['/', '\\']) && !file.name.starts_with('.'),
            "registry entry `{}` has an unsafe file name `{}`",
            entry.id,
            file.name
        );
    }
    ensure!(
        entry.files.iter().any(|f| f.name == "plugin.json"),
        "registry entry `{}` has no plugin.json",
        entry.id
    );
    std::fs::create_dir_all(plugins_dir)?;

    let staging = plugins_dir.join(format!("{}.new", entry.id));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let staged = (|| -> anyhow::Result<()> {
        for file in &entry.files {
            rji_updates::download_verified(&file.url, &file.sha256, file.size, &staging.join(&file.name))
                .with_context(|| format!("{} / {}", entry.id, file.name))?;
        }
        // Validate before it can replace anything.
        let plugin = Plugin::load(&staging)?;
        ensure!(plugin.manifest.id == entry.id, "plugin.json id doesn't match the registry");
        ensure!(plugin.manifest.version == entry.version, "plugin.json version doesn't match the registry");
        Ok(())
    })();
    if let Err(err) = staged {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    swap_in(&staging, &plugins_dir.join(&entry.id))
}

/// Copies a local plugin folder (one containing `plugin.json`) into the
/// plugins directory, after validating it.
pub fn install_from_folder(source: &Path, plugins_dir: &Path) -> anyhow::Result<PathBuf> {
    let plugin = Plugin::load(source).context("not a valid plugin folder")?;
    let id = plugin.manifest.id.clone();
    std::fs::create_dir_all(plugins_dir)?;
    let staging = plugins_dir.join(format!("{id}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    // Only the manifest and the module are needed at runtime.
    for name in ["plugin.json", plugin.manifest.wasm.as_str()] {
        std::fs::copy(source.join(name), staging.join(name)).with_context(|| format!("copying {name}"))?;
    }
    swap_in(&staging, &plugins_dir.join(&id))
}

pub fn uninstall(id: &str, plugins_dir: &Path) -> anyhow::Result<()> {
    ensure!(valid_id(id), "invalid plugin id");
    let dir = plugins_dir.join(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(())
}

/// Replaces `target` with `staging`, keeping the old copy until the new
/// one is in place (and restoring it if that fails).
fn swap_in(staging: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let old = target.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    let had_old = target.exists();
    if had_old {
        std::fs::rename(target, &old).context("moving the installed version aside")?;
    }
    if let Err(err) = std::fs::rename(staging, target) {
        if had_old {
            let _ = std::fs::rename(&old, target);
        }
        let _ = std::fs::remove_dir_all(staging);
        bail!("installing plugin failed (previous version restored): {err}");
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    const WAT: &str = r#"(module
        (memory (export "memory") 1)
        (data (i32.const 1024) "{\"replace\":\"HELLO\"}")
        (func (export "alloc") (param i32) (result i32) (i32.const 2048))
        (func (export "run") (param i32 i32) (result i64)
            (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 19))))"#;

    fn manifest_json(version: &str) -> Vec<u8> {
        format!(
            r#"{{"id":"hello","name":"Hello","version":"{version}","api_version":1,"commands":[{{"id":"go","title":"Go"}}]}}"#
        )
        .into_bytes()
    }

    fn sha(bytes: &[u8]) -> String {
        let path = std::env::temp_dir().join(format!("rji-sha-{}-{}", std::process::id(), bytes.len()));
        std::fs::write(&path, bytes).unwrap();
        let (hash, _) = rji_updates::sha256_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        hash
    }

    /// Serves `routes` from an already-bound listener (local HTTP, no network).
    fn serve(listener: TcpListener, routes: Vec<(String, Vec<u8>)>) {
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap() <= 2 {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone()).unwrap_or_default();
                let mut stream = stream;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
    }

    #[test]
    fn signed_registry_install_update_and_tamper() {
        let (secret, public) = rji_updates::keygen();
        let wasm = wat::parse_str(WAT).unwrap();
        let (v1, v2) = (manifest_json("1.0.0"), manifest_json("1.1.0"));
        let mut tampered = wasm.clone();
        *tampered.last_mut().unwrap() ^= 1;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let file = |name: &str, path: &str, bytes: &[u8]| RegistryFile {
            name: name.into(),
            url: format!("{base}{path}"),
            sha256: sha(bytes),
            size: bytes.len() as u64,
        };
        let entry = |version: &str, manifest_path: &str, manifest: &[u8]| RegistryEntry {
            id: "hello".into(),
            name: "Hello".into(),
            version: version.into(),
            description: String::new(),
            files: vec![file("plugin.json", manifest_path, manifest), file("plugin.wasm", "/plugin.wasm", &wasm)],
        };
        let (e1, e2) = (entry("1.0.0", "/v1.json", &v1), entry("1.1.0", "/v2.json", &v2));
        let index_text = serde_json::to_string(&RegistryIndex { plugins: vec![e2.clone()] }).unwrap();
        let signed = serde_json::json!({
            "index": index_text,
            "signature": rji_updates::sign_text(&index_text, &secret).unwrap(),
        });
        serve(
            listener,
            vec![
                ("/index.json".into(), signed.to_string().into_bytes()),
                ("/v1.json".into(), v1.clone()),
                ("/v2.json".into(), v2.clone()),
                ("/plugin.wasm".into(), wasm.clone()),
                ("/tampered.wasm".into(), tampered),
            ],
        );

        let dir = std::env::temp_dir().join(format!("rji-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Fresh install of 1.0.0; the signed index then offers 1.1.0.
        install(&e1, &dir).unwrap();
        let installed = Plugin::load(&dir.join("hello")).unwrap();
        assert_eq!(installed.manifest.version, "1.0.0");
        let index = fetch_index(&format!("{base}/index.json"), &public).unwrap();
        let updates = updates_available(&index, &[&installed]);
        assert_eq!(updates.len(), 1);
        install(updates[0], &dir).unwrap();
        assert_eq!(Plugin::load(&dir.join("hello")).unwrap().manifest.version, "1.1.0");

        // A tampered module fails verification and leaves 1.1.0 intact.
        let mut bad = e2.clone();
        bad.version = "1.2.0".into();
        bad.files[1].url = format!("{base}/tampered.wasm");
        assert!(install(&bad, &dir).is_err());
        assert_eq!(Plugin::load(&dir.join("hello")).unwrap().manifest.version, "1.1.0");
        assert!(!dir.join("hello.new").exists());

        // The wrong key rejects the whole index.
        let (_, other) = rji_updates::keygen();
        assert!(fetch_index(&format!("{base}/index.json"), &other).is_err());

        // Unsafe file names are refused before anything is downloaded.
        let mut evil = e1.clone();
        evil.files[0].name = "../escape.json".into();
        assert!(install(&evil, &dir).is_err());

        uninstall("hello", &dir).unwrap();
        assert!(!dir.join("hello").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_local_folder() {
        let src = std::env::temp_dir().join(format!("rji-plugin-src-{}", std::process::id()));
        let dir = std::env::temp_dir().join(format!("rji-plugin-dst-{}", std::process::id()));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("plugin.json"), manifest_json("2.0.0")).unwrap();
        std::fs::write(src.join("plugin.wasm"), wat::parse_str(WAT).unwrap()).unwrap();
        std::fs::write(src.join("notes.txt"), "not copied").unwrap();
        let installed = install_from_folder(&src, &dir).unwrap();
        assert!(installed.join("plugin.wasm").exists() && !installed.join("notes.txt").exists());
        let _ = (std::fs::remove_dir_all(&src), std::fs::remove_dir_all(&dir));
    }
}
