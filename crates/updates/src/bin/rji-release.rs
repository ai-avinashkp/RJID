//! Release tooling for RustedJavaIDE's signed self-updates.
//!
//!   rji-release keygen <secret-key-out>      new keypair; prints the public key
//!                                            (build the app with RJI_UPDATE_PUBKEY=<it>)
//!   rji-release hash <binary>                SHA-256 + size for a manifest asset
//!   rji-release sign <manifest.json> <secret-key-file>
//!                                            prints the signed manifest to publish
//!                                            at RJI_UPDATE_URL
//!   rji-release sign-index <plugins.json> <secret-key-file>
//!                                            prints the signed plugin registry index
//!                                            to publish at RJI_PLUGIN_REGISTRY_URL
//!
//! Keep the secret key offline and out of the repository.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["keygen", out] => keygen(out),
        ["hash", file] => hash(file),
        ["sign", manifest, secret] => sign(manifest, secret),
        ["sign-index", index, secret] => sign_index(index, secret),
        _ => {
            eprintln!("usage: rji-release keygen <secret-out> | hash <file> | sign <manifest.json> <secret-key> | sign-index <plugins.json> <secret-key>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(out: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!std::path::Path::new(out).exists(), "{out} already exists — not overwriting a key");
    let (secret, public) = rji_updates::keygen();
    std::fs::write(out, &secret)?;
    println!("secret key written to {out} (keep it offline)");
    println!("public key: {public}");
    Ok(())
}

fn hash(file: &str) -> anyhow::Result<()> {
    let (sha256, size) = rji_updates::sha256_file(std::path::Path::new(file))?;
    println!("{{ \"sha256\": \"{sha256}\", \"size\": {size}, \"target\": \"{}\" }}", rji_updates::current_target());
    Ok(())
}

fn sign(manifest: &str, secret: &str) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(manifest)?;
    let secret = std::fs::read_to_string(secret)?;
    let signed = rji_updates::sign_manifest(json.trim(), secret.trim())?;
    println!("{}", serde_json::to_string_pretty(&signed)?);
    Ok(())
}

fn sign_index(index: &str, secret: &str) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(index)?;
    let json = json.trim();
    // Must at least be JSON with a `plugins` array.
    let value: serde_json::Value = serde_json::from_str(json)?;
    anyhow::ensure!(value["plugins"].is_array(), "index must be {{\"plugins\": [...]}}");
    let signature = rji_updates::sign_text(json, std::fs::read_to_string(secret)?.trim())?;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "index": json, "signature": signature }))?);
    Ok(())
}
