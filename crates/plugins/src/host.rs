//! Loading and running plugins.
//!
//! A plugin is a folder with `plugin.json` (its manifest) and a
//! WebAssembly module. Plugins are sandboxed by construction:
//!
//! - **No capabilities.** The module may not import *anything* — no host
//!   functions, so no file system, network, clock, or process access. A
//!   module that declares any import is refused at load time.
//! - **Bounded.** Each command runs in a fresh instance with a CPU budget
//!   (wasmi fuel metering — an infinite loop is stopped), a memory cap, and
//!   size caps on input and output.
//! - **Narrow API.** The plugin receives JSON (the command id plus the
//!   selected text or document) and returns JSON describing an edit or a
//!   message. The IDE applies it as one undoable edit.
//!
//! ABI (version 1): the module exports `memory`, `alloc(len: i32) -> i32`,
//! and `run(ptr: i32, len: i32) -> i64`, which returns the output's
//! `(ptr << 32) | len` in its own memory.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

pub const API_VERSION: u32 = 1;
/// CPU budget per command (wasmi fuel ≈ instructions).
const FUEL_PER_RUN: u64 = 300_000_000;
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_IO_BYTES: usize = 16 * 1024 * 1024;
const MAX_WASM_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    /// Lowercase id (`a-z 0-9 - .`), also the folder name.
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub commands: Vec<PluginCommand>,
    #[serde(default = "default_wasm")]
    pub wasm: String,
}

fn default_wasm() -> String {
    "plugin.wasm".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
}

impl PluginManifest {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(valid_id(&self.id), "plugin id `{}` must be lowercase letters, digits, - or .", self.id);
        ensure!(
            self.api_version == API_VERSION,
            "plugin `{}` needs plugin API {} but this IDE provides {API_VERSION}",
            self.id,
            self.api_version
        );
        ensure!(!self.commands.is_empty(), "plugin `{}` declares no commands", self.id);
        ensure!(
            !self.wasm.contains(['/', '\\']) && self.wasm.ends_with(".wasm"),
            "plugin `{}`: `wasm` must be a .wasm file name in the plugin folder",
            self.id
        );
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

/// What a command is given.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInput {
    pub command: String,
    /// The selection, or the whole document when nothing is selected.
    pub text: String,
    pub is_selection: bool,
    pub file_name: String,
    pub language: String,
}

/// What a command asks the IDE to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutput {
    /// Replace the text the command was given.
    Replace(String),
    /// Just show a message.
    Message(String),
}

#[derive(Deserialize)]
struct RawOutput {
    replace: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

struct Limits(StoreLimits);

pub struct Plugin {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    engine: Engine,
    module: Module,
}

impl Plugin {
    /// Loads and validates a plugin folder. Nothing runs yet.
    pub fn load(dir: &Path) -> anyhow::Result<Plugin> {
        let manifest_text = std::fs::read_to_string(dir.join("plugin.json"))
            .with_context(|| format!("reading {}", dir.join("plugin.json").display()))?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_text).context("invalid plugin.json")?;
        manifest.validate()?;
        let wasm_path = dir.join(&manifest.wasm);
        let size = std::fs::metadata(&wasm_path)
            .with_context(|| format!("missing {}", wasm_path.display()))?
            .len();
        ensure!(size <= MAX_WASM_BYTES, "plugin `{}` module is too large", manifest.id);
        let wasm = std::fs::read(&wasm_path)?;
        Self::from_parts(manifest, dir.to_path_buf(), &wasm)
    }

    pub fn from_parts(manifest: PluginManifest, dir: PathBuf, wasm: &[u8]) -> anyhow::Result<Plugin> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm).map_err(|e| anyhow!("plugin `{}` is not valid WebAssembly: {e}", manifest.id))?;
        if let Some(import) = module.imports().next() {
            bail!(
                "plugin `{}` imports `{}::{}` — plugins may not import anything (no file, network, or system access)",
                manifest.id,
                import.module(),
                import.name()
            );
        }
        Ok(Plugin {
            manifest,
            dir,
            engine,
            module,
        })
    }

    /// Runs one command in a fresh, limited instance.
    pub fn run(&self, input: &CommandInput) -> anyhow::Result<CommandOutput> {
        let id = &self.manifest.id;
        let input_json = serde_json::to_vec(input)?;
        ensure!(input_json.len() <= MAX_IO_BYTES, "input is too large for a plugin");

        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY_BYTES)
            .instances(1)
            .memories(1)
            .tables(1)
            .build();
        let mut store = Store::new(&self.engine, Limits(limits));
        store.limiter(|data| &mut data.0);
        store.set_fuel(FUEL_PER_RUN).map_err(|e| anyhow!("{e}"))?;

        let linker = Linker::<Limits>::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &self.module)
            .and_then(|pre| pre.start(&mut store))
            .map_err(|e| anyhow!("plugin `{id}` failed to start: {e}"))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| anyhow!("plugin `{id}` exports no `memory`"))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "alloc")
            .map_err(|_| anyhow!("plugin `{id}` must export `alloc(i32) -> i32`"))?;
        let run = instance
            .get_typed_func::<(i32, i32), i64>(&store, "run")
            .map_err(|_| anyhow!("plugin `{id}` must export `run(i32, i32) -> i64`"))?;

        let trap = |e: wasmi::Error, _store: &Store<Limits>| {
            if e.as_trap_code() == Some(wasmi::core::TrapCode::OutOfFuel) {
                anyhow!("plugin `{id}` took too long and was stopped")
            } else {
                anyhow!("plugin `{id}` crashed: {e}")
            }
        };
        let ptr = alloc.call(&mut store, input_json.len() as i32).map_err(|e| trap(e, &store))?;
        memory
            .write(&mut store, ptr as u32 as usize, &input_json)
            .map_err(|_| anyhow!("plugin `{id}` returned an invalid input buffer"))?;
        let packed = run
            .call(&mut store, (ptr, input_json.len() as i32))
            .map_err(|e| trap(e, &store))?;

        let out_ptr = (packed as u64 >> 32) as usize;
        let out_len = (packed as u64 & 0xffff_ffff) as usize;
        ensure!(out_len <= MAX_IO_BYTES, "plugin `{id}` output is too large");
        let mut output = vec![0u8; out_len];
        memory
            .read(&store, out_ptr, &mut output)
            .map_err(|_| anyhow!("plugin `{id}` returned an invalid output buffer"))?;
        let raw: RawOutput = serde_json::from_slice(&output)
            .map_err(|e| anyhow!("plugin `{id}` returned invalid output: {e}"))?;
        match raw {
            RawOutput { error: Some(err), .. } => bail!("{}: {err}", self.manifest.name),
            RawOutput { replace: Some(text), .. } => Ok(CommandOutput::Replace(text)),
            RawOutput { message: Some(msg), .. } => Ok(CommandOutput::Message(msg)),
            _ => bail!("plugin `{id}` returned neither `replace`, `message`, nor `error`"),
        }
    }
}

/// Loads every plugin under `plugins_dir` (one folder each). Broken ones
/// are reported, not fatal.
pub fn discover(plugins_dir: &Path) -> (Vec<Plugin>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return (plugins, errors);
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.extension().is_some_and(|e| e == "old" || e == "new"))
        .collect();
    dirs.sort();
    for dir in dirs {
        match Plugin::load(&dir) {
            Ok(plugin) => plugins.push(plugin),
            Err(err) => errors.push(format!("{}: {err:#}", dir.display())),
        }
    }
    (plugins, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            api_version: API_VERSION,
            description: String::new(),
            author: String::new(),
            commands: vec![PluginCommand { id: "go".into(), title: "Go".into() }],
            wasm: "plugin.wasm".into(),
        }
    }

    fn plugin(wat: &str) -> anyhow::Result<Plugin> {
        Plugin::from_parts(manifest("test"), PathBuf::new(), &wat::parse_str(wat).unwrap())
    }

    fn input() -> CommandInput {
        CommandInput {
            command: "go".into(),
            text: "hello".into(),
            is_selection: true,
            file_name: "A.java".into(),
            language: "Java".into(),
        }
    }

    /// A minimal well-behaved plugin: ignores input, returns fixed JSON.
    const CONSTANT: &str = r#"(module
        (memory (export "memory") 1)
        (data (i32.const 1024) "{\"replace\":\"HELLO\"}")
        (func (export "alloc") (param i32) (result i32) (i32.const 2048))
        (func (export "run") (param i32 i32) (result i64)
            (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 19))))"#;

    #[test]
    fn runs_a_well_behaved_plugin() {
        assert_eq!(plugin(CONSTANT).unwrap().run(&input()).unwrap(), CommandOutput::Replace("HELLO".into()));
    }

    #[test]
    fn refuses_any_import() {
        let wat = r#"(module (import "env" "read_file" (func (param i32))) (memory (export "memory") 1))"#;
        let err = plugin(wat).err().unwrap().to_string();
        assert!(err.contains("may not import"), "{err}");
    }

    #[test]
    fn stops_an_infinite_loop() {
        let wat = r#"(module (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "run") (param i32 i32) (result i64) (loop (br 0)) (i64.const 0)))"#;
        let err = plugin(wat).unwrap().run(&input()).unwrap_err().to_string();
        assert!(err.contains("took too long"), "{err}");
    }

    #[test]
    fn caps_memory_growth() {
        // Tries to grow to 4 GiB; the limiter refuses, `memory.grow` returns
        // -1, and the plugin reports that as an error.
        let wat = r#"(module (memory (export "memory") 1)
            (data (i32.const 0) "{\"error\":\"no memory\"}")
            (func (export "alloc") (param i32) (result i32) (i32.const 512))
            (func (export "run") (param i32 i32) (result i64)
                (if (i32.ne (memory.grow (i32.const 65535)) (i32.const -1)) (then unreachable))
                (i64.const 21)))"#;
        let err = plugin(wat).unwrap().run(&input()).unwrap_err().to_string();
        assert!(err.contains("no memory"), "{err}");
    }

    #[test]
    fn rejects_out_of_bounds_output_and_bad_manifests() {
        let wat = r#"(module (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "run") (param i32 i32) (result i64)
                (i64.or (i64.shl (i64.const 60000) (i64.const 32)) (i64.const 100000))))"#;
        assert!(plugin(wat).unwrap().run(&input()).is_err());

        let mut bad = manifest("Bad ID!");
        assert!(bad.validate().is_err());
        bad = manifest("ok");
        bad.api_version = 99;
        assert!(bad.validate().unwrap_err().to_string().contains("plugin API"));
        bad = manifest("ok");
        bad.wasm = "../evil.wasm".into();
        assert!(bad.validate().is_err());
    }
}
