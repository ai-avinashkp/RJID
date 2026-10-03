//! Sandboxed WebAssembly plugins (see `host` for the security model) and
//! their signed registry (`registry`) for installing and updating them.

mod host;
mod registry;

pub use host::{API_VERSION, CommandInput, CommandOutput, Plugin, PluginCommand, PluginManifest, discover, valid_id};
pub use registry::{
    RegistryEntry, RegistryFile, RegistryIndex, fetch_index, install, install_from_folder, uninstall,
    updates_available,
};
