//! Layered, portable settings.
//!
//! - App-level: `<app_dir>/.rji-settings/settings.json` (theme, update channel,
//!   plugins, recent workspaces). `<app_dir>` is the directory the IDE binary
//!   runs from, not `%APPDATA%` — the whole install stays self-contained.
//! - Per-workspace: `<opened-folder>/.rji/settings.json`, optional, layered on
//!   top of app-level settings (like `.vscode`).
//!
//! Every read here is fail-safe: a missing or corrupt settings file never
//! panics — it just falls back to defaults so a hand-edited typo can't break
//! the IDE on startup.

use std::path::{Path, PathBuf};

use rji_theme::ThemeKind;
use serde::{Deserialize, Serialize};

/// Global, app-level settings, persisted next to the running executable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemeKind,
    pub update_channel: UpdateChannel,
    pub enabled_plugins: Vec<String>,
    pub recent_workspaces: Vec<PathBuf>,
    /// UI zoom factor applied to the whole window (Ctrl+=/Ctrl+-/Ctrl+0),
    /// persisted so it sticks across restarts. 1.0 = 100%.
    pub ui_zoom: f32,
    /// File-tree panel width in px (drag its right edge to resize).
    pub tree_width: f32,
    /// Terminal panel height in px (drag its top edge to resize).
    pub terminal_height: f32,
    pub show_tree: bool,
    pub show_terminal: bool,
    /// Which side the file tree is docked on (drag its header to move it).
    pub tree_side: PanelSide,
    /// Where the terminal is docked.
    pub terminal_dock: TerminalDock,
    /// Terminal width in px when docked on the right.
    pub terminal_width: f32,
    pub updates: UpdateSettings,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalDock {
    #[default]
    Bottom,
    Right,
}

/// Toolchain update checks (JDK, Maven, Spring Boot, JavaFX).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for new releases in the background and notify (default on).
    pub check_automatically: bool,
    /// Also apply *patch-level* updates to versions pinned in the
    /// project's build files automatically, with a backup (default off;
    /// never applies minor/major upgrades or installs a JDK).
    pub auto_apply_safe: bool,
    /// Download new versions of the IDE itself in the background (they're
    /// signature-checked and installed on the next restart; default off).
    pub auto_download_ide: bool,
    /// Install updates to installed plugins from the signed registry
    /// automatically (default off: notify only).
    pub auto_update_plugins: bool,
    pub check_interval_hours: u64,
    /// Unix seconds of the last completed check.
    pub last_check_unix: u64,
    /// `component:version` entries the user chose to ignore.
    pub ignored: Vec<String>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings {
            check_automatically: true,
            auto_apply_safe: false,
            auto_download_ide: false,
            auto_update_plugins: false,
            check_interval_hours: 24,
            last_check_unix: 0,
            ignored: Vec::new(),
        }
    }
}

pub const TREE_WIDTH_RANGE: (f32, f32) = (140.0, 600.0);
pub const TERMINAL_HEIGHT_RANGE: (f32, f32) = (90.0, 800.0);
pub const TERMINAL_WIDTH_RANGE: (f32, f32) = (220.0, 1200.0);

/// Zoom step for Ctrl+=/Ctrl+-, and the min/max clamp so text can't be
/// zoomed into illegibility or nothingness.
pub const ZOOM_STEP: f32 = 0.1;
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 3.0;

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            theme: ThemeKind::default(),
            update_channel: UpdateChannel::default(),
            enabled_plugins: Vec::new(),
            recent_workspaces: Vec::new(),
            ui_zoom: 1.0,
            tree_width: 240.0,
            terminal_height: 220.0,
            show_tree: true,
            show_terminal: true,
            tree_side: PanelSide::Left,
            terminal_dock: TerminalDock::Bottom,
            terminal_width: 460.0,
            updates: UpdateSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateChannel {
    Stable,
    Beta,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        UpdateChannel::Stable
    }
}

/// Optional per-workspace overrides, layered on top of [`AppSettings`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceSettings {
    pub theme: Option<ThemeKind>,
    /// Named, user-defined run commands for this workspace (e.g. a specific
    /// main class, program args, or test invocation), managed in Run ▸ Run
    /// Configurations… (or by editing this file). Without any, ▶ Run uses
    /// the task detected for the project type.
    pub run_configs: Vec<RunConfig>,
    /// Name of the selected entry in `run_configs`; `""` means "automatic"
    /// (the detected task). `None` before any choice: the first entry is
    /// used, or the detected task if there are none.
    pub active_run_config: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub name: String,
    /// A full shell command line, sent to the integrated terminal verbatim
    /// (same mechanism as the Maven/Gradle toolbar buttons).
    pub command: String,
}

/// Directory the running executable lives in — the root for portable,
/// app-level settings. Falls back to the current working directory if the
/// executable path can't be resolved (should not happen in practice).
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn app_settings_path() -> PathBuf {
    app_dir().join(".rji-settings").join("settings.json")
}

fn workspace_settings_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".rji").join("settings.json")
}

/// Loads app-level settings, falling back to defaults on any error (missing
/// file, unreadable, or invalid JSON) so a corrupt file never blocks startup.
pub fn load_app_settings() -> AppSettings {
    load_json_or_default(&app_settings_path())
}

/// Loads workspace-level overrides for `workspace_root`, defaulting to "no
/// overrides" the same way `load_app_settings` defaults on error.
pub fn load_workspace_settings(workspace_root: &Path) -> WorkspaceSettings {
    load_json_or_default(&workspace_settings_path(workspace_root))
}

fn load_json_or_default<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|err| {
            eprintln!(
                "rji-settings: {} is not valid JSON ({err}); using defaults",
                path.display()
            );
            T::default()
        }),
        Err(_) => T::default(),
    }
}

/// Persists app-level settings, creating `.rji-settings/` if needed.
pub fn save_app_settings(settings: &AppSettings) -> anyhow::Result<()> {
    save_json(&app_settings_path(), settings)
}

/// Persists per-workspace overrides, creating `.rji/` if needed.
pub fn save_workspace_settings(
    workspace_root: &Path,
    settings: &WorkspaceSettings,
) -> anyhow::Result<()> {
    save_json(&workspace_settings_path(workspace_root), settings)
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Clamps a zoom factor into the supported range — used whenever
/// `ui_zoom` changes so a bad settings-file edit can't zoom the UI away.
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The theme that should actually be used: the workspace override if present,
/// otherwise the app-level choice.
pub fn effective_theme(app: &AppSettings, workspace: &WorkspaceSettings) -> ThemeKind {
    workspace.theme.unwrap_or(app.theme)
}
