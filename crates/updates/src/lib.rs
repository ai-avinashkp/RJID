//! Toolchain update checks (JDK, Maven, Spring Boot, JavaFX), IntelliJ
//! style: check and notify by default; changes are only ever made with the
//! user's click — or, if they opt in, automatically for patch-level
//! (bug/security fix) releases of versions pinned in the project's own
//! build files, with a backup. JDKs and system Maven are never installed
//! by the IDE; those get a download link.

mod checker;
pub mod jdtls;
mod project;
mod self_update;
mod sources;
mod version;

pub use checker::{CheckReport, Installed, UpdateItem, check, detect_system_maven, jdk_version_from_banner};
pub use project::{Component, VersionSite, apply_update, backup_path, find_sites};
pub use sources::maven_central_versions;
pub use version::{Bump, Version, is_stable, latest_stable};
pub use self_update::{
    ReleaseAsset, ReleaseManifest, SignedManifest, applicable_asset, apply_staged, cleanup_previous,
    current_target, download_and_stage, download_verified, fetch_manifest, fetch_signed_text, keygen,
    sha256_file, sign_manifest, sign_text, staged_path, verify_manifest, verify_signed_text,
};
