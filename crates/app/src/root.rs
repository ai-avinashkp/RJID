//! Top-level window: toolbar, file tree (+ debug panel), editor tabs,
//! terminal, and status bar — plus the app-wide concerns that tie them
//! together: workspace switching, the language server's lifecycle, debug
//! sessions, keyboard shortcuts, resizable panels, and not losing unsaved
//! work on close.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, PromptLevel, Render,
    ScrollHandle, Subscription, Window, div, prelude::*, px,
};
use rji_core::{FileNode, read_dir_tree};
use rji_jdwp_client::StepDepth;
use rji_project_java::{
    GradleProject, JdkCandidate, MavenProject, ProjectInfo, TaskGroup, default_run_task,
    detect_gradle_project, detect_jdks, detect_maven_project, detect_project, project_tasks,
};
use rji_settings::{
    AppSettings, PanelSide, RunConfig, TERMINAL_HEIGHT_RANGE, TERMINAL_WIDTH_RANGE, TREE_WIDTH_RANGE, TerminalDock, WorkspaceSettings, ZOOM_STEP,
    clamp_zoom, load_app_settings, load_workspace_settings, save_app_settings,
    save_workspace_settings,
};
use rji_theme::{Theme, icon_for};

mod prompts;
mod titlebar;
mod android_ui;
mod dock_ui;
mod drop_ui;
mod new_project_ui;
mod run_configs_ui;
mod plugins_ui;
mod updates_ui;

use prompts::{InputPrompt, SHORTCUTS, TreeContextMenu};
use titlebar::{MenuId, TITLEBAR_HEIGHT};

use crate::debug_session::{self, DebugSession, SessionUpdate, java_class_name};
use crate::devtools_hook::{DevtoolsHook, SharedDevtoolsHook};
use crate::editor_view::{CodeEditorView, EditorEvent};
use crate::lsp_shared::SharedLspClient;
use crate::scrollbar::scroll_thumb;
use crate::terminal_view::{TerminalEvent, TerminalView};

const TOOLBAR_HEIGHT: f32 = 40.0;
const TAB_BAR_HEIGHT: f32 = 32.0;
const STATUS_BAR_HEIGHT: f32 = 24.0;
const PANEL_HEADER_HEIGHT: f32 = 26.0;
const SPLITTER_SIZE: f32 = 5.0;
const MIN_EDITOR_HEIGHT: f32 = 120.0;
/// How long a directory listing is reused before re-reading the disk. The
/// whole window re-renders on every keystroke; without this cache the file
/// tree re-read every expanded directory each frame.
const TREE_CACHE_TTL: Duration = Duration::from_secs(2);
/// How long a notice toast stays up: longer messages get longer to read.
fn notice_duration(message: &str) -> Duration {
    Duration::from_millis((6000 + 45 * message.len() as u64).min(20_000))
}
const MAX_OPEN_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_RECENT_WORKSPACES: usize = 8;

#[derive(Clone, PartialEq)]
enum LspStatus {
    Idle,
    Starting,
    Ready,
    Unavailable(&'static str),
}

#[derive(Clone, Copy, PartialEq)]
enum DebugState {
    Idle,
    Connecting,
    Running,
    Paused,
}

#[derive(Clone, Copy, PartialEq)]
enum Splitter {
    Tree,
    Terminal,
    /// The left edge of a terminal docked on the right.
    TerminalSide,
}

struct OpenTab {
    path: PathBuf,
    view: Entity<CodeEditorView>,
    _subscription: Subscription,
}

pub struct RootView {
    app_settings: AppSettings,
    workspace_root: Option<PathBuf>,
    expanded_dirs: HashSet<PathBuf>,
    tabs: Vec<OpenTab>,
    active_tab: Option<usize>,
    terminal: Entity<TerminalView>,
    maven_project: Option<MavenProject>,
    gradle_project: Option<GradleProject>,
    /// What kind of project the workspace is (build tool, frameworks,
    /// entry points) — drives the Build/Run/Test tasks.
    project: Option<ProjectInfo>,
    /// Port of the web server the last run reported as listening.
    server_port: Option<u16>,
    _terminal_subscription: Subscription,
    new_project: Option<new_project_ui::NewProjectWizard>,
    run_configs: Option<run_configs_ui::RunConfigsPanel>,
    jdk_candidates: Vec<JdkCandidate>,
    focus_handle: FocusHandle,
    tree_scroll: ScrollHandle,
    debug_scroll: ScrollHandle,

    lsp: SharedLspClient,
    lsp_status: LspStatus,
    /// Bumped whenever the workspace changes, so a language server that
    /// finishes starting for the *previous* workspace is discarded.
    lsp_generation: u64,
    devtools: SharedDevtoolsHook,

    /// Breakpoints per file (0-based lines). Owned here rather than only in
    /// editors so they survive closing and reopening a file.
    breakpoints: HashMap<PathBuf, BTreeSet<u32>>,
    debug: Option<Rc<RefCell<DebugSession>>>,
    debug_state: DebugState,

    tree_cache: RefCell<HashMap<PathBuf, (Instant, Vec<FileNode>)>>,
    ws_settings_cache: RefCell<Option<(Option<SystemTime>, WorkspaceSettings)>>,
    resizing: Option<Splitter>,
    /// A transient message for the status bar (e.g. why a file won't open).
    notice: Option<(String, Instant)>,
    /// The notice an expiry repaint is already scheduled for.
    notice_timer: Option<Instant>,
    /// GPUI allows only one prompt at a time per window (a second one hits
    /// an `unreachable!`), so every prompt checks this first.
    prompt_open: bool,
    /// Set once the user has confirmed closing, so the close isn't
    /// intercepted a second time.
    close_confirmed: bool,

    open_menu: Option<MenuId>,
    prompt: Option<InputPrompt>,
    context_menu: Option<TreeContextMenu>,
    show_shortcuts: bool,
    /// Last title pushed to the OS window (taskbar / Alt+Tab), so it's only
    /// set when it changes rather than every frame.
    last_os_title: RefCell<String>,
    /// In a narrow window the tree auto-hides; Ctrl+B shows it anyway.
    tree_forced_in_narrow: bool,
    /// Window width at the last render (for width-dependent actions).
    viewport_width: f32,
    /// Width the right-docked terminal got at the last render (0 if not).
    side_terminal_width: f32,
    update_report: Option<rji_updates::CheckReport>,
    update_checking: bool,
    show_updates: bool,
    ide_update: updates_ui::IdeUpdate,
    plugin_state: plugins_ui::PluginState,
    show_plugins: bool,
    android: android_ui::AndroidState,
    show_android: bool,
}

/// Responsive breakpoints (window width in px).
const NARROW_TREE_WIDTH: f32 = 640.0;
const COMPACT_TOOLBAR_WIDTH: f32 = 900.0;
const MINIMAL_TOOLBAR_WIDTH: f32 = 640.0;
const COMPACT_STATUS_WIDTH: f32 = 720.0;
/// Below this width a right-docked terminal moves to the bottom.
const NARROW_SIDE_TERMINAL_WIDTH: f32 = 760.0;

impl RootView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let app_settings = load_app_settings();
        let workspace_root = initial_workspace(&app_settings);
        let theme = Theme::for_kind(app_settings.theme);
        let terminal_cwd = workspace_root.clone().unwrap_or_else(|| PathBuf::from("."));
        let zoom = app_settings.ui_zoom;
        let (terminal, terminal_subscription) = Self::make_terminal(terminal_cwd, theme, zoom, cx);
        let maven_project = workspace_root.as_deref().and_then(detect_maven_project);
        let gradle_project = workspace_root.as_deref().and_then(detect_gradle_project);
        let project = workspace_root.as_deref().and_then(detect_project);
        let devtools = Rc::new(RefCell::new(devtools_hook_for(
            workspace_root.as_deref(),
            &maven_project,
            &gradle_project,
        )));

        let mut root = RootView {
            app_settings,
            workspace_root,
            expanded_dirs: HashSet::new(),
            tabs: Vec::new(),
            active_tab: None,
            terminal,
            project,
            server_port: None,
            _terminal_subscription: terminal_subscription,
            new_project: None,
            run_configs: None,
            maven_project,
            gradle_project,
            jdk_candidates: detect_jdks(),
            focus_handle: cx.focus_handle(),
            tree_scroll: ScrollHandle::new(),
            debug_scroll: ScrollHandle::new(),
            lsp: Rc::new(RefCell::new(None)),
            lsp_status: LspStatus::Idle,
            lsp_generation: 0,
            devtools,
            breakpoints: HashMap::new(),
            debug: None,
            debug_state: DebugState::Idle,
            tree_cache: RefCell::new(HashMap::new()),
            ws_settings_cache: RefCell::new(None),
            resizing: None,
            notice: None,
            notice_timer: None,
            prompt_open: false,
            close_confirmed: false,
            open_menu: None,
            prompt: None,
            context_menu: None,
            show_shortcuts: false,
            last_os_title: RefCell::new(String::new()),
            tree_forced_in_narrow: false,
            viewport_width: 1280.0,
            side_terminal_width: 0.0,
            update_report: None,
            update_checking: false,
            show_updates: false,
            ide_update: updates_ui::IdeUpdate::initial(),
            plugin_state: plugins_ui::PluginState::load(),
            show_plugins: false,
            android: android_ui::AndroidState::default(),
            show_android: false,
        };
        if let Some(path) = root.workspace_root.clone() {
            root.remember_workspace(&path);
            root.prune_placeholder_run_configs();
        }
        root.maybe_check_updates(cx);
        root
    }

    /// A terminal panel plus the subscription that reacts to servers it
    /// starts (the "open in browser" link, port-in-use hints).
    fn make_terminal(
        cwd: PathBuf,
        theme: Theme,
        zoom: f32,
        cx: &mut Context<Self>,
    ) -> (Entity<TerminalView>, Subscription) {
        let terminal = cx.new(|cx| {
            let mut terminal = TerminalView::new(cwd, theme, cx);
            terminal.zoom = zoom;
            terminal
        });
        let subscription = cx.subscribe(&terminal, |this, _, event: &TerminalEvent, cx| {
            match *event {
                TerminalEvent::ServerStarted(port) => {
                    this.server_port = Some(port);
                    this.notify_user(format!("Server running at http://localhost:{port}/"));
                }
                TerminalEvent::PortInUse(port) => {
                    this.server_port = None;
                    this.notify_user(format!(
                        "Port {port} is already in use — stop the other server, or set server.port in application.properties"
                    ));
                }
            }
            cx.notify();
        });
        (terminal, subscription)
    }

    pub fn focus_handle_for_init(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Opens a file passed on the command line (`rjid.exe App.java`), once
    /// the window exists.
    pub fn open_command_line_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = command_line_path().filter(|p| p.is_file()) {
            if let Some(dir) = path.parent() {
                // Expand the tree down to the file so it's visible there too.
                if let Some(root) = self.workspace_root.clone() {
                    for ancestor in dir.ancestors().take_while(|a| a.starts_with(&root) && *a != root) {
                        self.expanded_dirs.insert(ancestor.to_path_buf());
                    }
                }
            }
            self.open_file(path, window, cx);
        }
    }

    fn theme(&self) -> Theme {
        Theme::for_kind(self.app_settings.theme)
    }

    fn notify_user(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
    }

    fn save_app_settings(&self) {
        let _ = save_app_settings(&self.app_settings);
    }

    /// This workspace's `.rji/settings.json`, re-read only when the file's
    /// modification time changes — so a hand-edit (through this very
    /// editor) takes effect right away without reading the disk each frame.
    fn workspace_settings(&self) -> WorkspaceSettings {
        let Some(root) = self.workspace_root.as_deref() else {
            return WorkspaceSettings::default();
        };
        let modified = std::fs::metadata(root.join(".rji").join("settings.json"))
            .and_then(|m| m.modified())
            .ok();
        if let Some((cached_at, cached)) = self.ws_settings_cache.borrow().as_ref()
            && *cached_at == modified
        {
            return cached.clone();
        }
        let loaded = load_workspace_settings(root);
        *self.ws_settings_cache.borrow_mut() = Some((modified, loaded.clone()));
        loaded
    }

    fn write_workspace_settings(&self, settings: &WorkspaceSettings) {
        if let Some(root) = self.workspace_root.as_deref() {
            let _ = save_workspace_settings(root, settings);
        }
        *self.ws_settings_cache.borrow_mut() = None;
    }

    fn active_run_config(&self) -> Option<RunConfig> {
        let settings = self.workspace_settings();
        if settings.active_run_config.as_deref() == Some(run_configs_ui::AUTOMATIC) {
            return self.auto_run_config();
        }
        settings
            .active_run_config
            .as_deref()
            .and_then(|name| settings.run_configs.iter().find(|c| c.name == name))
            .or_else(|| settings.run_configs.first())
            .cloned()
            .or_else(|| self.auto_run_config())
    }

    /// With no saved run configurations, Run uses the best task for the
    /// detected project type (Spring Boot, JavaFX, a main class, ...).
    fn auto_run_config(&self) -> Option<RunConfig> {
        let task = default_run_task(self.project.as_ref()?)?;
        Some(RunConfig {
            name: task.label,
            command: task.command,
        })
    }

    // ---- appearance ------------------------------------------------------

    fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        self.app_settings.theme = self.app_settings.theme.next();
        self.save_app_settings();
        let theme = self.theme();
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                view.theme = theme;
                cx.notify();
            });
        }
        self.terminal.update(cx, |view, cx| {
            view.theme = theme;
            cx.notify();
        });
        cx.notify();
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.app_settings.ui_zoom = clamp_zoom(zoom);
        self.save_app_settings();
        let zoom = self.app_settings.ui_zoom;
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                view.zoom = zoom;
                cx.notify();
            });
        }
        self.terminal.update(cx, |view, cx| {
            view.zoom = zoom;
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        if self.viewport_width < NARROW_TREE_WIDTH {
            // Narrow window: show/hide it just for now, leaving the saved
            // preference for wide windows alone.
            self.tree_forced_in_narrow = !self.tree_visible();
        } else {
            self.app_settings.show_tree = !self.app_settings.show_tree;
            self.save_app_settings();
        }
        cx.notify();
    }

    /// Whether the file tree is shown at the current window width.
    fn tree_visible(&self) -> bool {
        if self.viewport_width < NARROW_TREE_WIDTH {
            self.tree_forced_in_narrow
        } else {
            self.app_settings.show_tree
        }
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.app_settings.show_terminal = !self.app_settings.show_terminal;
        self.save_app_settings();
        if self.app_settings.show_terminal {
            let handle = self.terminal.focus_handle(cx);
            window.focus(&handle, cx);
        } else {
            self.focus_active_editor(window, cx);
        }
        cx.notify();
    }

    fn show_terminal(&mut self) {
        if !self.app_settings.show_terminal {
            self.app_settings.show_terminal = true;
            self.save_app_settings();
        }
    }

    // ---- workspace -------------------------------------------------------

    fn remember_workspace(&mut self, path: &Path) {
        let recent = &mut self.app_settings.recent_workspaces;
        // Normalizing also merges old duplicate spellings of one folder.
        let mut seen = HashSet::new();
        let normalized: Vec<PathBuf> = std::iter::once(path.to_path_buf())
            .chain(recent.drain(..))
            .map(normalize_path)
            .filter(|p| seen.insert(p.clone()))
            .collect();
        *recent = normalized;
        recent.truncate(MAX_RECENT_WORKSPACES);
        self.save_app_settings();
    }

    /// Opens a folder as the workspace — after offering to save any
    /// unsaved tabs, since they're all closed.
    fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_unsaved(None, "Open another folder?", window, cx, move |this, window, cx| {
            this.switch_workspace(path, window, cx);
        });
    }

    fn switch_workspace(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        self.stop_debug(cx);
        // The language server is bound to the old workspace root.
        if let Some(mut client) = self.lsp.borrow_mut().take() {
            client.shutdown();
        }
        self.lsp_generation += 1;
        self.lsp_status = LspStatus::Idle;

        self.maven_project = detect_maven_project(&path);
        self.gradle_project = detect_gradle_project(&path);
        self.project = detect_project(&path);
        *self.devtools.borrow_mut() =
            devtools_hook_for(Some(&path), &self.maven_project, &self.gradle_project);
        self.workspace_root = Some(path.clone());
        self.expanded_dirs.clear();
        self.tree_cache.borrow_mut().clear();
        *self.ws_settings_cache.borrow_mut() = None;
        self.breakpoints.clear();
        window.focus(&self.focus_handle, cx);
        self.tabs.clear();
        self.active_tab = None;
        self.remember_workspace(&path);
        self.prune_placeholder_run_configs();
        if self.app_settings.updates.check_automatically {
            // A different project pins different versions: re-check it.
            self.update_report = None;
            self.check_for_updates(false, cx);
        }

        let theme = self.theme();
        let zoom = self.app_settings.ui_zoom;
        self.terminal.update(cx, |terminal, _| terminal.shutdown());
        let (terminal, subscription) = Self::make_terminal(path, theme, zoom, cx);
        self.terminal = terminal;
        self._terminal_subscription = subscription;
        self.server_port = None;
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                this.update_in(cx, |view, window, cx| view.open_folder(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded_dirs.remove(&path) {
            self.expanded_dirs.insert(path);
        }
        cx.notify();
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        self.tree_cache.borrow_mut().clear();
        self.maven_project = self.workspace_root.as_deref().and_then(detect_maven_project);
        self.gradle_project = self.workspace_root.as_deref().and_then(detect_gradle_project);
        self.project = self.workspace_root.as_deref().and_then(detect_project);
        cx.notify();
    }

    fn collapse_tree(&mut self, cx: &mut Context<Self>) {
        self.expanded_dirs.clear();
        cx.notify();
    }

    /// A directory's entries, from cache when fresh (see `TREE_CACHE_TTL`).
    fn dir_entries(&self, dir: &Path) -> Vec<FileNode> {
        if let Some((read_at, entries)) = self.tree_cache.borrow().get(dir)
            && read_at.elapsed() < TREE_CACHE_TTL
        {
            return entries.clone();
        }
        let entries = read_dir_tree(dir).unwrap_or_default();
        self.tree_cache
            .borrow_mut()
            .insert(dir.to_path_buf(), (Instant::now(), entries.clone()));
        entries
    }

    // ---- tabs ------------------------------------------------------------

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalize_path(path);
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.activate_tab(idx, window, cx);
            return;
        }
        let content = match read_text_file(&path) {
            Ok(content) => content,
            Err(reason) => {
                self.notify_user(reason);
                cx.notify();
                return;
            }
        };

        let theme = self.theme();
        let zoom = self.app_settings.ui_zoom;
        let is_java = path.extension().and_then(|e| e.to_str()) == Some("java");
        let saved_breakpoints = self.breakpoints.get(&path).cloned().unwrap_or_default();
        let lsp = self.lsp.clone();
        let devtools = self.devtools.clone();
        let view = cx.new(|cx| {
            let mut editor = CodeEditorView::new(path.clone(), content.clone(), theme, lsp, devtools, cx);
            editor.zoom = zoom;
            editor.set_breakpoints(saved_breakpoints);
            editor
        });
        let subscription = cx.subscribe_in(&view, window, |this, view, event: &EditorEvent, window, cx| match event {
            EditorEvent::BreakpointsChanged => this.on_breakpoints_changed(view, cx),
            EditorEvent::Notice(message) => {
                this.notify_user(message.clone());
                cx.notify();
            }
            EditorEvent::ExternalEdits(files) => this.apply_external_edits(files.clone(), window, cx),
        });
        self.tabs.push(OpenTab {
            path: path.clone(),
            view,
            _subscription: subscription,
        });
        self.activate_tab(self.tabs.len() - 1, window, cx);

        if is_java {
            if let Some(client) = self.lsp.borrow_mut().as_mut() {
                let _ = client.did_open(&path, &content.replace("\r\n", "\n"));
            }
            self.spawn_lsp_if_needed(cx);
        }
    }

    /// Language-server edits to files other than the active editor's
    /// (rename across files, "Create class" quick fixes): open tabs are
    /// edited in place (undoable, unsaved); other files are edited on
    /// disk; newly created files are opened.
    fn apply_external_edits(&mut self, files: Vec<rji_lsp_client::FileEdit>, window: &mut Window, cx: &mut Context<Self>) {
        let mut created = Vec::new();
        let mut failed = Vec::new();
        for file in files {
            let path = normalize_path(file.path.clone());
            if let Some(tab) = self.tabs.iter().find(|t| t.path == path) {
                tab.view.update(cx, |editor, cx| {
                    editor.apply_text_edits(&file.edits);
                    cx.notify();
                });
                continue;
            }
            let original = if file.create && !path.exists() {
                String::new()
            } else {
                match read_text_file(&path) {
                    Ok(text) => text,
                    Err(_) => {
                        failed.push(path);
                        continue;
                    }
                }
            };
            let crlf = original.contains("\r\n");
            let mut buffer = rji_editor::TextBuffer::new(original.replace("\r\n", "\n"));
            let edits = file
                .edits
                .iter()
                .map(|e| {
                    let offset = |(line, ch): (u32, u32)| {
                        let line = line as usize;
                        if line >= buffer.line_count() {
                            return buffer.text().len();
                        }
                        let range = buffer.line_range(line);
                        let text = &buffer.text()[range.clone()];
                        let mut units = 0u32;
                        let col = text
                            .char_indices()
                            .find(|(_, c)| {
                                let reached = units >= ch;
                                units += c.len_utf16() as u32;
                                reached
                            })
                            .map_or(text.len(), |(i, _)| i);
                        range.start + col
                    };
                    let text = e.new_text.replace("\r\n", "\n").replace('\t', &" ".repeat(rji_editor::INDENT));
                    (offset(e.start)..offset(e.end), text)
                })
                .collect();
            buffer.apply_edits(edits);
            let out = if crlf { buffer.text().replace('\n', "\r\n") } else { buffer.text().to_string() };
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, out));
            match written {
                Ok(()) if file.create => created.push(path),
                Ok(()) => {}
                Err(_) => failed.push(path),
            }
        }
        self.tree_cache.borrow_mut().clear();
        if !failed.is_empty() {
            self.notify_user(format!("Couldn't update {} file(s): {}", failed.len(), failed[0].display()));
        }
        for path in created {
            self.open_file(path, window, cx);
        }
        cx.notify();
    }

    fn activate_tab(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active_tab = Some(idx);
        let handle = self.tabs[idx].view.focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if count == 0 {
            return;
        }
        let current = self.active_tab.unwrap_or(0);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.activate_tab(next, window, cx);
    }

    fn focus_active_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = match self.active_tab.and_then(|idx| self.tabs.get(idx)) {
            Some(tab) => tab.view.focus_handle(cx),
            None => self.focus_handle.clone(),
        };
        window.focus(&handle, cx);
    }

    /// Closes a tab, first offering to save it if it has unsaved changes.
    fn close_tab(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let path = tab.path.clone();
        self.confirm_unsaved(Some(path.clone()), "Close this file?", window, cx, move |this, window, cx| {
            if let Some(idx) = this.tabs.iter().position(|t| t.path == path) {
                this.close_tab_now(idx, window, cx);
            }
        });
    }

    fn close_tab_now(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        // Move focus off the tab *before* dropping it: dropping a focused
        // entity once crashed the app (a panic inside a Windows callback
        // aborts the process) when closing the last tab.
        window.focus(&self.focus_handle, cx);

        let closing_path = self.tabs[idx].path.clone();
        if closing_path.extension().and_then(|e| e.to_str()) == Some("java")
            && let Some(client) = self.lsp.borrow_mut().as_mut()
        {
            let _ = client.did_close(&closing_path);
        }

        self.tabs.remove(idx);
        self.active_tab = if self.tabs.is_empty() {
            None
        } else {
            Some(idx.min(self.tabs.len() - 1))
        };
        if let Some(new_idx) = self.active_tab {
            let handle = self.tabs[new_idx].view.focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// Saves the given tabs (all dirty ones when `only` is `None`); returns
    /// false if any save failed (the file stays marked dirty).
    fn save_tabs(&mut self, only: Option<&Path>, cx: &mut Context<Self>) -> bool {
        let mut all_saved = true;
        for tab in &self.tabs {
            if only.is_some_and(|p| p != tab.path) || !tab.view.read(cx).dirty {
                continue;
            }
            tab.view.update(cx, |view, cx| {
                view.save();
                cx.notify();
            });
            all_saved &= !tab.view.read(cx).dirty;
        }
        cx.notify();
        all_saved
    }

    fn dirty_file_names(&self, only: Option<&Path>, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .filter(|t| only.is_none_or(|p| p == t.path) && t.view.read(cx).dirty)
            .map(|t| t.view.read(cx).file_name())
            .collect()
    }

    /// Runs `proceed` right away if nothing (in scope) is unsaved; otherwise
    /// asks Save / Don't Save / Cancel first. `only` limits the question to
    /// one file; `None` means every open tab.
    fn confirm_unsaved(
        &mut self,
        only: Option<PathBuf>,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        proceed: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let dirty = self.dirty_file_names(only.as_deref(), cx);
        if dirty.is_empty() {
            return proceed(self, window, cx);
        }
        if self.prompt_open {
            return;
        }
        self.prompt_open = true;
        let detail = format!("Unsaved changes in: {}", dirty.join(", "));
        let answer = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&detail),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            this.update_in(cx, |this, window, cx| {
                this.prompt_open = false;
                match answer {
                    Some(0) => {
                        if this.save_tabs(only.as_deref(), cx) {
                            proceed(this, window, cx);
                        } else {
                            this.notify_user("Save failed — nothing was closed");
                            cx.notify();
                        }
                    }
                    Some(1) => proceed(this, window, cx),
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- app lifecycle -------------------------------------------------------

    /// Hooked to the window's close button (see `main.rs`). Returns whether
    /// the window may close now; with unsaved work it asks first and closes
    /// the window itself once the user decides.
    pub fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed || self.dirty_file_names(None, cx).is_empty() {
            self.shutdown_children(cx);
            return true;
        }
        self.confirm_unsaved(None, "Quit RJID?", window, cx, |this, window, cx| {
            this.close_confirmed = true;
            this.shutdown_children(cx);
            window.remove_window();
        });
        false
    }

    /// Stops every child process this window started (language server,
    /// debugged program, terminal shell). `Drop` alone isn't enough: it
    /// never runs if the process exits without unwinding.
    fn shutdown_children(&mut self, cx: &mut Context<Self>) {
        if let Some(mut client) = self.lsp.borrow_mut().take() {
            client.shutdown();
        }
        if let Some(session) = self.debug.take() {
            session.borrow_mut().stop();
        }
        self.terminal.update(cx, |terminal, _| terminal.shutdown());
    }

    // ---- language server -----------------------------------------------------

    /// Starts jdtls the first time a `.java` file is opened. Startup takes
    /// seconds, so it runs off the UI thread; once connected, `.java` tabs
    /// opened meanwhile are registered and diagnostics start flowing.
    fn spawn_lsp_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.lsp_status != LspStatus::Idle {
            return;
        }
        let Some(workspace_root) = self.workspace_root.clone() else {
            return;
        };
        let Some(jdk) = self.jdk_candidates.first() else {
            self.lsp_status = LspStatus::Unavailable("no JDK found");
            return;
        };
        let Some(jdtls) = rji_lsp_client::discover_jdtls() else {
            self.lsp_status = LspStatus::Unavailable("jdtls not found (install the VS Code Java extension)");
            return;
        };
        self.lsp_status = LspStatus::Starting;
        let java_exe = jdk.home.join("bin").join(if cfg!(windows) { "java.exe" } else { "java" });
        let data_dir = jdtls_data_dir(&workspace_root);
        let shared_lsp = self.lsp.clone();
        let generation = self.lsp_generation;

        cx.spawn(async move |root, cx| {
            let spawn_result = cx
                .background_spawn(async move {
                    rji_lsp_client::LspClient::spawn(&java_exe, &jdtls.server_dir, &workspace_root, &data_dir)
                })
                .await;

            let still_current = root
                .read_with(cx, |root, _| root.lsp_generation == generation)
                .unwrap_or(false);
            let mut client = match spawn_result {
                Ok(client) => client,
                Err(_) => {
                    if still_current {
                        root.update(cx, |root, cx| {
                            root.lsp_status = LspStatus::Unavailable("jdtls failed to start");
                            cx.notify();
                        })
                        .ok();
                    }
                    return;
                }
            };
            if !still_current {
                // The workspace changed while jdtls was starting.
                client.shutdown();
                return;
            }
            let diagnostics_rx = client.diagnostics_rx.clone();
            *shared_lsp.borrow_mut() = Some(client);

            root.update(cx, |root, cx| {
                root.lsp_status = LspStatus::Ready;
                for tab in &root.tabs {
                    if tab.path.extension().and_then(|e| e.to_str()) == Some("java") {
                        let content = tab.view.read(cx).text().to_string();
                        if let Some(client) = root.lsp.borrow_mut().as_mut() {
                            let _ = client.did_open(&tab.path, &content);
                        }
                    }
                }
                cx.notify();
            })
            .ok();

            while let Ok(published) = diagnostics_rx.recv().await {
                let alive = root
                    .update(cx, |root, cx| {
                        if let Some(tab) = root.tabs.iter().find(|t| t.path == published.file_path) {
                            tab.view.update(cx, |editor, cx| {
                                editor.set_diagnostics(published.diagnostics, cx);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    // ---- build / run ---------------------------------------------------------

    /// Status-bar hint for what a run will do, so a Spring Boot app that
    /// has no web server doesn't look like it "only built".
    fn explain_run(&mut self, command: &str) {
        let is_boot_run = command.contains("spring-boot:run") || command.contains("bootRun");
        let starts_app = is_boot_run
            || command.contains("exec:java")
            || command.contains("javafx:run")
            || command.split_whitespace().any(|w| w == "run" || w == "java");
        if !starts_app {
            return;
        }
        self.server_port = None;
        let Some(project) = self.project.as_ref() else { return };
        if project.spring_boot && !project.spring_web && is_boot_run {
            self.notify_user(
                "No web starter in this Spring Boot app, so it runs once and exits without listening on a port. \
                 Add spring-boot-starter-web (or -webmvc) to serve HTTP.",
            );
        } else if let Some(port) = project.server_port.filter(|p| *p != 0) {
            self.notify_user(format!("Starting… the app will serve http://localhost:{port}/ once it's up"));
        }
    }

    fn run_in_terminal(&mut self, command: &str, cx: &mut Context<Self>) {
        self.explain_run(command);
        self.show_terminal();
        let command = format!("{command}\r");
        self.terminal.update(cx, |view, cx| {
            view.send_text(&command);
            cx.notify();
        });
        cx.notify();
    }

    fn run_active_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_run_config() {
            Some(config) => {
                if let Some(problem) = self.stale_run_config(&config) {
                    self.notify_user(problem);
                    self.open_run_configs(window, cx);
                    return;
                }
                self.run_in_terminal(&config.command, cx)
            }
            None => {
                self.notify_user("Nothing to run yet — open Run ▸ Run Configurations… to add a command");
                cx.notify();
            }
        }
    }

    // ---- debugging -------------------------------------------------------------

    fn on_breakpoints_changed(&mut self, view: &Entity<CodeEditorView>, cx: &mut Context<Self>) {
        let editor = view.read(cx);
        let path = editor.path.clone();
        let lines = editor.breakpoints.clone();
        let class_name = editor.fully_qualified_class_name();
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        } else {
            self.breakpoints.insert(path.clone(), lines.clone());
        }
        if let (Some(session), Some(class_name)) = (&self.debug, class_name) {
            session.borrow_mut().set_breakpoints(path, class_name, lines);
        }
        cx.notify();
    }

    /// Debugs the active run config (see `debug_session::debug_launch` for
    /// which command shapes are supported: `java ...`, Maven
    /// `spring-boot:run` / `exec:java` / `test`, Gradle `run` / `bootRun` /
    /// `test`).
    fn start_debug(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.active_run_config() else {
            self.notify_user("No run configuration to debug");
            cx.notify();
            return;
        };
        if let Some(problem) = self.stale_run_config(&config) {
            self.notify_user(problem);
            self.open_run_configs(window, cx);
            return;
        }
        self.debug_command(&config.command, window, cx);
    }

    /// Runs `command` in the terminal with its JVM waiting for a debugger,
    /// then attaches.
    fn debug_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug_state != DebugState::Idle {
            return;
        }
        let Ok(port) = debug_session::free_port() else {
            self.notify_user("No free local port for the debugger");
            cx.notify();
            return;
        };
        let Some(launch) = debug_session::debug_launch(command, port) else {
            self.notify_user(format!("Don't know how to debug `{command}`"));
            cx.notify();
            return;
        };
        self.run_in_terminal(&launch.command, cx);
        self.attach_debugger(format!("127.0.0.1:{}", launch.port), window, cx);
    }

    /// Attaches to a JVM listening for a debugger at `addr` — one launched
    /// by `debug_command`, or started by hand with
    /// `-agentlib:jdwp=transport=dt_socket,server=y,address=5005`.
    fn attach_debugger(&mut self, addr: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug_state != DebugState::Idle {
            return;
        }
        let Some(root) = self.workspace_root.clone() else {
            return;
        };

        // Breakpoints to arm: open tabs know their class name from the
        // buffer; for closed files, read the package from disk.
        let targets: Vec<(PathBuf, String, BTreeSet<u32>)> = self
            .breakpoints
            .iter()
            .filter_map(|(path, lines)| {
                let class_name = match self.tabs.iter().find(|t| &t.path == path) {
                    Some(tab) => tab.view.read(cx).fully_qualified_class_name(),
                    None => java_class_name(path, &std::fs::read_to_string(path).unwrap_or_default()),
                }?;
                Some((path.clone(), class_name, lines.clone()))
            })
            .collect();

        self.debug_state = DebugState::Connecting;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let connected = cx
                .background_spawn(debug_session::connect_with_retry(addr))
                .await;
            let Ok(client) = connected else {
                this.update(cx, |this, cx| {
                    this.debug_state = DebugState::Idle;
                    this.notify_user("Debugger could not attach — is the JVM running with the JDWP agent?");
                    cx.notify();
                })
                .ok();
                return;
            };
            let events_rx = client.events_rx.clone();
            let session = Rc::new(RefCell::new(DebugSession::new(client, root)));
            for (path, class_name, lines) in targets {
                session.borrow_mut().set_breakpoints(path, class_name, lines);
            }
            this.update(cx, |this, cx| {
                this.debug = Some(session.clone());
                this.debug_state = DebugState::Running;
                cx.notify();
            })
            .ok();

            while let Ok(event) = events_rx.recv().await {
                let update = session.borrow_mut().handle_event(event);
                match update {
                    SessionUpdate::None => {}
                    SessionUpdate::Paused => {
                        this.update_in(cx, |this, window, cx| this.on_debug_paused(window, cx))
                            .ok();
                    }
                    SessionUpdate::Ended => break,
                }
            }
            this.update(cx, |this, cx| this.end_debug(cx)).ok();
        })
        .detach();
    }

    /// Opens (or switches to) the file the program stopped in and puts the
    /// caret on the paused line.
    fn on_debug_paused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debug_state = DebugState::Paused;
        let target = self.debug.as_ref().and_then(|session| {
            let session = session.borrow();
            let paused = session.paused.as_ref()?;
            paused
                .frames
                .iter()
                .find_map(|f| Some((f.path.clone()?, f.line?)))
        });
        self.clear_paused_lines(cx);
        if let Some((path, line)) = target {
            self.open_file(path.clone(), window, cx);
            if let Some(tab) = self.tabs.iter().find(|t| t.path == path) {
                tab.view.update(cx, |editor, cx| {
                    editor.paused_line = Some(line);
                    editor.reveal_line(line);
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    fn open_frame(&mut self, path: PathBuf, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path.clone(), window, cx);
        if let Some(tab) = self.tabs.iter().find(|t| t.path == path) {
            tab.view.update(cx, |editor, cx| {
                editor.reveal_line(line);
                cx.notify();
            });
        }
    }

    fn clear_paused_lines(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.view.update(cx, |editor, cx| {
                if editor.paused_line.take().is_some() {
                    cx.notify();
                }
            });
        }
    }

    fn resume_debug(&mut self, cx: &mut Context<Self>) {
        if self.debug_state != DebugState::Paused {
            return;
        }
        if let Some(session) = &self.debug {
            session.borrow_mut().resume();
        }
        self.debug_state = DebugState::Running;
        self.clear_paused_lines(cx);
        cx.notify();
    }

    fn step(&mut self, depth: StepDepth, cx: &mut Context<Self>) {
        if self.debug_state != DebugState::Paused {
            return;
        }
        if let Some(session) = &self.debug {
            session.borrow_mut().step(depth);
        }
        self.debug_state = DebugState::Running;
        self.clear_paused_lines(cx);
        cx.notify();
    }

    fn stop_debug(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.debug {
            session.borrow_mut().stop();
        }
        self.end_debug(cx);
    }

    fn end_debug(&mut self, cx: &mut Context<Self>) {
        if self.debug.take().is_some() || self.debug_state != DebugState::Idle {
            self.notify_user("Debug session ended");
        }
        self.debug_state = DebugState::Idle;
        self.clear_paused_lines(cx);
        cx.notify();
    }

    // ---- input ---------------------------------------------------------------------

    /// Runs before any child sees the key (capture phase): Escape closes an
    /// open menu or overlay even while an editor has focus.
    fn capture_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "escape" {
            return;
        }
        if self.open_menu.is_some() || self.context_menu.is_some() {
            self.open_menu = None;
            self.context_menu = None;
        } else if self.show_shortcuts {
            self.show_shortcuts = false;
        } else if self.show_updates {
            self.show_updates = false;
        } else if self.show_plugins {
            self.show_plugins = false;
        } else if self.show_android {
            self.show_android = false;
        } else if self.prompt.is_some() {
            self.close_prompt_from_root(window, cx);
        } else if self.new_project.is_some() {
            self.close_new_project(window, cx);
        } else if self.run_configs.is_some() {
            self.close_run_configs(window, cx);
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn close_prompt_from_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.focus_active_editor(window, cx);
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let ctrl = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;
        let alt = keystroke.modifiers.alt;
        if alt && !ctrl && !shift
            && let Some(menu) = MenuId::for_alt_key(keystroke.key.as_str())
        {
            // In a narrow window the menus are collapsed into ☰.
            let compact = self.viewport_width < titlebar::COMPACT_MENU_WIDTH;
            self.open_menu = Some(if compact { MenuId::All } else { menu });
            self.context_menu = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match keystroke.key.as_str() {
            "o" if ctrl => self.pick_folder(window, cx),
            "=" | "+" if ctrl => self.set_zoom(self.app_settings.ui_zoom + ZOOM_STEP, cx),
            "-" if ctrl => self.set_zoom(self.app_settings.ui_zoom - ZOOM_STEP, cx),
            "0" if ctrl => self.set_zoom(1.0, cx),
            "w" if ctrl => {
                if let Some(idx) = self.active_tab {
                    self.close_tab(idx, window, cx);
                }
            }
            "tab" if ctrl => self.cycle_tab(!shift, window, cx),
            "`" if ctrl => self.toggle_terminal(window, cx),
            "b" if ctrl => self.toggle_tree(cx),
            "s" if ctrl && shift => {
                if !self.save_tabs(None, cx) {
                    self.notify_user("Some files could not be saved");
                }
            }
            "f5" if shift => self.stop_debug(cx),
            "f5" => self.run_active_config(window, cx),
            "f8" => self.resume_debug(cx),
            "f10" => self.step(StepDepth::Over, cx),
            "f11" if shift => self.step(StepDepth::Out, cx),
            "f11" => self.step(StepDepth::Into, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_root_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(splitter) = self.resizing else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_resize(cx);
            return;
        }
        match splitter {
            Splitter::Tree => {
                let (min, max) = TREE_WIDTH_RANGE;
                let x = f32::from(event.position.x);
                let width = if self.app_settings.tree_side == PanelSide::Right {
                    // Measured from the right edge, inside a right-docked terminal.
                    let terminal = if self.side_terminal_width > 0.0 { self.side_terminal_width + SPLITTER_SIZE } else { 0.0 };
                    f32::from(window.viewport_size().width) - terminal - x
                } else {
                    x
                };
                self.app_settings.tree_width = width.clamp(min, max);
            }
            Splitter::TerminalSide => {
                let (min, max) = TERMINAL_WIDTH_RANGE;
                let width = f32::from(window.viewport_size().width) - f32::from(event.position.x);
                self.app_settings.terminal_width = width.clamp(min, max);
            }
            Splitter::Terminal => {
                let viewport_h = f32::from(window.viewport_size().height);
                let (min, max) = TERMINAL_HEIGHT_RANGE;
                let height = viewport_h - STATUS_BAR_HEIGHT - f32::from(event.position.y);
                self.app_settings.terminal_height = height.clamp(min, max);
            }
        }
        cx.notify();
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.save_app_settings();
            cx.notify();
        }
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---- rendering -----------------------------------------------------------------

impl RootView {
    /// The action bar under the title bar, IDE-style:
    /// - left: the project (name + detected type) and its Build / Test tasks;
    /// - right: what runs — server link, run-configuration selector, and the
    ///   ▶ Run / 🐞 Debug / ■ Stop icon buttons (step controls while
    ///   debugging).
    ///
    /// Opening folders, themes and zoom live in the File / View menus.
    /// Responsive: as the window narrows, labels collapse to icons and the
    /// project details drop out; the run controls always stay.
    fn render_toolbar(&self, theme: Theme, width: f32, cx: &Context<Self>) -> impl IntoElement {
        let compact = width < COMPACT_TOOLBAR_WIDTH;
        let minimal = width < MINIMAL_TOOLBAR_WIDTH;
        let mut left = div().flex().flex_row().items_center().gap_1().min_w_0().overflow_hidden();
        let mut right = div().flex().flex_row().flex_shrink_0().items_center().gap_1();

        // Project chip + Build/Test (all tasks are also under Run).
        if let Some(project) = &self.project {
            left = left.child(
                div()
                    .id("project-chip")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .bg(theme.foreground.opacity(0.06))
                    .tooltip(crate::tooltip::Tooltip::text(format!("{} — {}", project.name, project.kind_label()), theme))
                    .child(div().flex_shrink_0().text_color(theme.accent).child("▣"))
                    .child(div().whitespace_nowrap().overflow_hidden().text_color(theme.foreground).child(project.name.clone()))
                    .when(!compact, |chip| {
                        chip.child(
                            div()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_size(px(12.))
                                .text_color(theme.foreground_muted)
                                .child(project.kind_label()),
                        )
                    }),
            );
            if !minimal {
                for task in project_tasks(project)
                    .into_iter()
                    .filter(|t| matches!(t.group, TaskGroup::Build | TaskGroup::Test))
                {
                    let command = task.command.clone();
                    let glyph = if task.group == TaskGroup::Build { "⚒" } else { "✓" };
                    left = left.child(ghost_button(
                        gpui::SharedString::from(format!("task-{}", task.id)),
                        glyph,
                        (!compact).then(|| task.label.clone()),
                        format!("{} — {}", task.label, task.command),
                        theme.foreground,
                        theme,
                        cx.listener(move |this, _, _, cx| this.run_in_terminal(&command, cx)),
                    ));
                }
            }
        } else if self.workspace_root.is_none() {
            left = left.child(div().text_color(theme.foreground_muted).child("No folder open — File ▸ Open Folder (Ctrl+O)"));
        }

        if self.workspace_root.is_some() {
            match self.debug_state {
                DebugState::Idle => {
                    if let Some(port) = self.server_port {
                        right = right.child(
                            div()
                                .id("open-server")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py(px(3.))
                                .rounded_md()
                                .cursor_pointer()
                                .text_color(theme.success)
                                .bg(theme.success.opacity(0.10))
                                .hover(|s| s.bg(theme.success.opacity(0.2)))
                                .tooltip(crate::tooltip::Tooltip::text(format!("Open http://localhost:{port}/ in the browser"), theme))
                                .child("●")
                                .when(!minimal, |s| s.child(format!("localhost:{port} ↗")))
                                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&format!("http://localhost:{port}/")))),
                        );
                    }
                    let config = self.active_run_config();
                    right = right.child(
                        div()
                            .id("config")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .max_w(px(if compact { 150. } else { 260. }))
                            .px_2()
                            .py(px(3.))
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                            .tooltip(crate::tooltip::Tooltip::text("Run configuration — what ▶ Run executes. Click to choose or add one.", theme))
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(config.as_ref().map_or("Add Configuration…".to_string(), |c| c.name.clone())),
                            )
                            .child(div().flex_shrink_0().text_color(theme.foreground_muted).child("▾"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_run_configs(window, cx))),
                    );
                    let can_debug = config.as_ref().is_some_and(|c| debug_session::debug_launch(&c.command, 0).is_some());
                    right = right
                        .child(icon_action("run", "▶", "Run the selected configuration (F5)", theme.success, config.is_some(), theme, cx.listener(|this, _, window, cx| this.run_active_config(window, cx))))
                        .child(icon_action("debug", "🐞", "Debug the selected configuration", theme.accent, can_debug, theme, cx.listener(|this, _, window, cx| this.start_debug(window, cx))));
                }
                state => {
                    let paused = state == DebugState::Paused;
                    let step = |id: &'static str, glyph: &'static str, tip: &'static str, depth: StepDepth| {
                        icon_action(id, glyph, tip, theme.foreground, paused, theme, cx.listener(move |this, _, _, cx| this.step(depth, cx)))
                    };
                    right = right
                        .child(div().px_2().text_size(px(12.)).text_color(theme.warning).child(match state {
                            DebugState::Connecting => "Attaching…",
                            DebugState::Paused => "Paused",
                            _ => "Debugging",
                        }))
                        .child(icon_action("continue", "▶", "Continue (F8)", theme.success, paused, theme, cx.listener(|this, _, _, cx| this.resume_debug(cx))))
                        .child(step("step-over", "↷", "Step Over (F10)", StepDepth::Over))
                        .child(step("step-into", "↓", "Step Into (F11)", StepDepth::Into))
                        .child(step("step-out", "↑", "Step Out (Shift+F11)", StepDepth::Out))
                        .child(icon_action("stop", "■", "Stop debugging (Shift+F5)", theme.error, true, theme, cx.listener(|this, _, _, cx| this.stop_debug(cx))));
                }
            }
        }

        div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_2()
            .h(px(TOOLBAR_HEIGHT))
            .px_2()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_color(theme.foreground)
            .text_size(px(13.))
            .child(left)
            .child(right)
    }

    fn render_tree_entries(&self, dir: &Path, depth: usize, active: Option<&Path>, cx: &Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme();
        let mut elements = Vec::new();
        for entry in self.dir_entries(dir) {
            let indent = px((depth as f32) * 14.0 + 6.0);
            let click_path = entry.path.clone();
            let menu_path = entry.path.clone();
            let is_dir = entry.is_dir;
            let expanded = is_dir && self.expanded_dirs.contains(&entry.path);
            let is_active = active == Some(entry.path.as_path());
            let has_breakpoints = self.breakpoints.contains_key(&entry.path);

            let marker = if is_dir {
                div()
                    .w(px(14.))
                    .flex_shrink_0()
                    .text_color(icon_for(&entry.name, true).color)
                    .child(if expanded { "▾" } else { "▸" })
            } else {
                let icon = icon_for(&entry.name, false);
                div()
                    .w(px(14.))
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(icon.color)
                    .child(icon.glyph)
            };

            let row = div()
                .id(entry.path.to_string_lossy().into_owned())
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(22.))
                .pl(indent)
                .pr_2()
                .whitespace_nowrap()
                .text_color(if is_dir || is_active {
                    theme.foreground
                } else {
                    theme.foreground_muted
                })
                .cursor_pointer()
                .when(expanded, |s| s.bg(theme.accent.opacity(0.08)))
                .when(is_active, |s| s.bg(theme.accent.opacity(0.22)))
                .hover(|s| s.bg(theme.accent.opacity(0.16)))
                .child(marker)
                .child(entry.name.clone())
                .when(has_breakpoints, |s| {
                    s.child(div().w(px(6.)).h(px(6.)).rounded_full().bg(theme.error))
                })
                .on_click(cx.listener(move |this, _event, window, cx| {
                    if is_dir {
                        this.toggle_dir(click_path.clone(), cx);
                    } else {
                        this.open_file(click_path.clone(), window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_tree_context_menu(menu_path.clone(), is_dir, event.position, cx);
                        cx.stop_propagation();
                    }),
                );

            elements.push(row.into_any_element());
            if expanded {
                elements.extend(self.render_tree_entries(&entry.path, depth + 1, active, cx));
            }
        }
        elements
    }

    fn render_tree_panel(&self, theme: Theme, width: f32, height: f32, cx: &Context<Self>) -> impl IntoElement {
        let debug_h = if self.debug_state == DebugState::Idle {
            0.0
        } else {
            (height * 0.45).clamp(140.0, height - PANEL_HEADER_HEIGHT * 3.0).max(0.0)
        };
        let list_h = (height - PANEL_HEADER_HEIGHT - debug_h).max(0.0);
        let folder_name = self
            .workspace_root
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "NO FOLDER".to_string());
        let active = self
            .active_tab
            .and_then(|idx| self.tabs.get(idx))
            .map(|t| t.path.as_path());

        let header = panel_header(theme)
            .id("tree-header")
            // Drag the header to dock the tree on the other side.
            .on_drag(crate::docking::PanelDrag { panel: crate::docking::DockPanel::Tree, theme }, |drag, _, _, cx| {
                crate::docking::start_drag(drag, cx)
            })
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(folder_name))
            .when(self.workspace_root.is_some(), |h| {
                h.child(icon_button("tree-new-file", "+", "New file", theme, cx.listener(|this, _, window, cx| {
                    this.prompt_new_entry(false, None, window, cx)
                })))
                .child(icon_button("tree-new-folder", "⊞", "New folder", theme, cx.listener(|this, _, window, cx| {
                    this.prompt_new_entry(true, None, window, cx)
                })))
            })
            .child(icon_button("tree-refresh", "⟳", "Refresh", theme, cx.listener(|this, _, _, cx| this.refresh_tree(cx))))
            .child(icon_button("tree-collapse", "⊟", "Collapse all", theme, cx.listener(|this, _, _, cx| this.collapse_tree(cx))));

        let entries = self
            .workspace_root
            .as_deref()
            .map(|root| self.render_tree_entries(root, 0, active, cx))
            .unwrap_or_default();
        let tree_thumb = scroll_thumb(&self.tree_scroll, list_h, theme.accent);
        let list = div()
            .relative()
            .h(px(list_h))
            .child(
                div()
                    .id("file-tree-scroll")
                    .track_scroll(&self.tree_scroll)
                    .size_full()
                    .overflow_scroll()
                    .py_1()
                    .children(entries)
                    // Right-click on empty space: act on the project root.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if let Some(root) = this.workspace_root.clone() {
                                this.open_tree_context_menu(root, true, event.position, cx);
                            }
                        }),
                    ),
            )
            .children(tree_thumb);

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(width))
            .h(px(height))
            .bg(theme.surface)
            .text_size(px(13. * self.app_settings.ui_zoom))
            .child(header)
            .child(list)
            .when(debug_h > 0.0, |panel| panel.child(self.render_debug_panel(theme, debug_h, cx)))
    }

    fn render_debug_panel(&self, theme: Theme, height: f32, cx: &Context<Self>) -> impl IntoElement {
        let session = self.debug.as_ref().map(|s| s.borrow());
        let paused = session.as_ref().and_then(|s| s.paused.clone());
        let state_text = match self.debug_state {
            DebugState::Connecting => "attaching…",
            DebugState::Running => "running",
            DebugState::Paused => "paused",
            DebugState::Idle => "",
        };

        let mut body = div()
            .id("debug-scroll")
            .track_scroll(&self.debug_scroll)
            .flex()
            .flex_col()
            .size_full()
            .overflow_scroll()
            .px_2()
            .py_1()
            .gap_1()
            .text_size(px(12. * self.app_settings.ui_zoom));

        match paused {
            None => {
                let armed = session.as_ref().map_or(0, |s| {
                    self.breakpoints
                        .iter()
                        .flat_map(|(p, lines)| lines.iter().map(move |l| (p, *l)))
                        .filter(|(p, l)| s.is_armed(p, *l))
                        .count()
                });
                let total: usize = self.breakpoints.values().map(BTreeSet::len).sum();
                body = body.child(
                    div().text_color(theme.foreground_muted).child(if self.debug_state == DebugState::Connecting {
                        "Waiting for the program to start…".to_string()
                    } else {
                        format!("Running — {armed} of {total} breakpoints armed (they arm as classes load)")
                    }),
                );
            }
            Some(paused) => {
                body = body.child(section_label("VARIABLES", theme));
                match &paused.variables {
                    Ok(vars) if vars.is_empty() => {
                        body = body.child(div().text_color(theme.foreground_muted).child("(none in scope)"));
                    }
                    Ok(vars) => {
                        for var in vars {
                            body = body.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_1()
                                    .whitespace_nowrap()
                                    .child(div().text_color(theme.syntax.keyword).child(var.name.clone()))
                                    .child(div().text_color(theme.foreground_muted).child("="))
                                    .child(div().text_color(theme.syntax.string).child(var.value.clone()))
                                    .child(div().text_color(theme.foreground_muted).child(format!("({})", var.type_name))),
                            );
                        }
                    }
                    Err(reason) => {
                        body = body.child(div().text_color(theme.foreground_muted).child(reason.clone()));
                    }
                }
                body = body.child(section_label("CALL STACK", theme));
                for (i, frame) in paused.frames.iter().enumerate() {
                    let target = frame.path.clone().zip(frame.line);
                    body = body.child(
                        div()
                            .id(("frame", i))
                            .whitespace_nowrap()
                            .px_1()
                            .rounded_sm()
                            .text_color(if target.is_some() { theme.foreground } else { theme.foreground_muted })
                            .when(i == 0, |s| s.bg(theme.warning.opacity(0.18)))
                            .when(target.is_some(), |s| s.cursor_pointer().hover(|s| s.bg(theme.accent.opacity(0.16))))
                            .child(frame.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some((path, line)) = target.clone() {
                                    this.open_frame(path, line, window, cx);
                                }
                            })),
                    );
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .h(px(height))
            .border_t_1()
            .border_color(theme.border)
            .child(
                panel_header(theme)
                    .child(div().text_color(theme.accent).child("DEBUG"))
                    .child(div().flex_1().text_color(theme.foreground_muted).child(state_text)),
            )
            .child(div().flex_1().min_h_0().child(body))
    }

    fn render_tab_bar(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(idx, tab)| {
            let is_active = self.active_tab == Some(idx);
            let editor = tab.view.read(cx);
            let dirty = editor.dirty;
            let name = editor.file_name();
            let icon = icon_for(&name, false);

            div()
                .id(("tab", idx))
                .group("tab")
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .h_full()
                .pl_3()
                .pr_2()
                .border_r_1()
                .border_color(theme.border)
                .cursor_pointer()
                .text_color(if is_active { theme.foreground } else { theme.foreground_muted })
                .when(is_active, |s| s.bg(theme.background).border_t_2().border_color(theme.accent))
                .when(!is_active, |s| s.hover(|s| s.bg(theme.background.opacity(0.5))))
                .child(div().text_size(px(10.)).text_color(icon.color).child(icon.glyph))
                .child(name)
                .child(
                    div()
                        .id(("tab-close", idx))
                        .w(px(16.))
                        .flex()
                        .justify_center()
                        .rounded_sm()
                        .text_color(theme.foreground_muted)
                        .hover(|s| s.bg(theme.accent.opacity(0.2)).text_color(theme.foreground))
                        // Unsaved: a dot, which turns into × on hover.
                        .child(if dirty { "●" } else { "×" })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                // Keep the tab's own click from also firing.
                                cx.stop_propagation();
                                this.close_tab(idx, window, cx);
                            }),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| this.close_tab(idx, window, cx)),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate_tab(idx, window, cx)))
        });

        div()
            .id("tab-bar")
            .flex()
            .flex_row()
            .flex_shrink_0()
            .w_full()
            .h(px(TAB_BAR_HEIGHT))
            .overflow_x_scroll()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(13.))
            .children(tabs)
    }

    fn render_welcome(&self, theme: Theme, height: f32, cx: &Context<Self>) -> impl IntoElement {
        let recent: Vec<PathBuf> = self
            .app_settings
            .recent_workspaces
            .iter()
            .filter(|p| Some(p.as_path()) != self.workspace_root.as_deref() && p.is_dir())
            .take(5)
            .cloned()
            .collect();
        // Short windows get fewer shortcut rows (the full list is under
        // Help ▸ Keyboard Shortcuts).
        let shortcut_rows = ((height - 260.0) / 22.0).clamp(0.0, 10.0) as usize;
        let shortcuts = &SHORTCUTS[..shortcut_rows.min(SHORTCUTS.len())];

        div()
            .id("welcome")
            .flex()
            .flex_col()
            .w_full()
            .h(px(height))
            .overflow_y_scroll()
            .items_center()
            .justify_center()
            .gap_4()
            .px_4()
            .bg(theme.background)
            .text_color(theme.foreground_muted)
            .child(div().text_size(px(26.)).text_color(theme.foreground).child("RJID"))
            .child(div().child(match &self.workspace_root {
                Some(_) => "Open a file from the tree to start editing".to_string(),
                None => "Open a folder to get started (Ctrl+O)".to_string(),
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .child(toolbar_button(
                        "welcome-new-project",
                        "New Project…",
                        theme,
                        cx.listener(|this, _, window, cx| this.open_new_project(window, cx)),
                    ))
                    .child(toolbar_button(
                        "welcome-open-folder",
                        "Open Folder…",
                        theme,
                        cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                    )),
            )
            .when(!recent.is_empty(), |col| {
                col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(section_label("RECENT FOLDERS", theme))
                        .children(recent.into_iter().enumerate().map(|(i, path)| {
                            let label = path.display().to_string();
                            div()
                                .id(("recent", i))
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_folder(path.clone(), window, cx)
                                }))
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(12.))
                    .children(shortcuts.iter().map(|(keys, action)| {
                        div()
                            .flex()
                            .flex_row()
                            .gap_3()
                            .child(div().w(px(190.)).flex_shrink_0().text_color(theme.foreground).child(*keys))
                            .child(div().whitespace_nowrap().child(*action))
                    }))
                    .child(
                        div()
                            .id("all-shortcuts")
                            .pt_1()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("All keyboard shortcuts…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_shortcuts = true;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Responsive: in a narrow window the less important items (language,
    /// line endings, JDK) drop out first.
    fn render_status_bar(&self, theme: Theme, width: f32, cx: &Context<Self>) -> impl IntoElement {
        let compact = width < COMPACT_STATUS_WIDTH;
        let mut left = div().flex().flex_row().items_center().gap_3().min_w_0().overflow_hidden();
        let mut right = div().flex().flex_row().flex_shrink_0().items_center().gap_3();

        if let Some(tab) = self.active_tab.and_then(|idx| self.tabs.get(idx)) {
            let status = tab.view.read(cx).status();
            left = left
                .child(div().whitespace_nowrap().child(format!("Ln {}, Col {}", status.line, status.column)))
                .when(status.selected_chars > 0, |s| {
                    s.child(div().whitespace_nowrap().child(format!("({} sel)", status.selected_chars)))
                })
                .when(!compact, |s| s.child(status.language).child(status.line_ending))
                .when(status.errors + status.warnings > 0, |s| {
                    s.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .child(div().text_color(theme.error).child(format!("✕ {}", status.errors)))
                            .child(div().text_color(theme.warning).child(format!("⚠ {}", status.warnings))),
                    )
                });
            if let Some((severity, message)) = status.line_message {
                let color = match severity {
                    rji_lsp_client::DiagnosticSeverity::Error => theme.error,
                    rji_lsp_client::DiagnosticSeverity::Warning => theme.warning,
                    _ => theme.foreground_muted,
                };
                left = left.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(color)
                        .child(message.lines().next().unwrap_or("").to_string()),
                );
            }
        }

        match self.debug_state {
            DebugState::Idle => {}
            DebugState::Connecting => right = right.child(div().text_color(theme.warning).child("Debug: attaching…")),
            DebugState::Running => right = right.child(div().text_color(theme.success).child("Debug: running")),
            DebugState::Paused => right = right.child(div().text_color(theme.warning).child("Debug: paused")),
        }
        // Toolchain updates: a clickable count (opens the Updates panel).
        let updates = self.update_count();
        if self.update_checking || updates > 0 {
            right = right.child(
                div()
                    .id("status-updates")
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .text_color(if updates > 0 { theme.accent } else { theme.foreground_muted })
                    .hover(|s| s.bg(theme.accent.opacity(0.16)))
                    .child(if self.update_checking && updates == 0 {
                        "⟳ checking updates".to_string()
                    } else {
                        format!("⬆ {updates} update{}", if updates == 1 { "" } else { "s" })
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_updates = true;
                        cx.notify();
                    })),
            );
        }
        let (lsp_text, lsp_color) = match &self.lsp_status {
            LspStatus::Idle => (String::new(), theme.foreground_muted),
            LspStatus::Starting if compact => ("Java: starting…".to_string(), theme.foreground_muted),
            LspStatus::Starting => ("Java: starting language server…".to_string(), theme.foreground_muted),
            LspStatus::Ready => ("Java: ready".to_string(), theme.success),
            LspStatus::Unavailable(_) if compact => ("Java: unavailable".to_string(), theme.warning),
            LspStatus::Unavailable(why) => (format!("Java: {why}"), theme.warning),
        };
        if !lsp_text.is_empty() {
            right = right.child(div().whitespace_nowrap().text_color(lsp_color).child(lsp_text));
        }
        if !compact {
            right = right.child(match self.jdk_candidates.first() {
                Some(jdk) => div().child(format!("JDK {}", short_jdk_version(jdk.version.as_deref()))),
                None => div().text_color(theme.warning).child("No JDK detected"),
            });
        }
        let zoom = self.app_settings.ui_zoom;
        if (zoom - 1.0).abs() > 0.01 {
            right = right.child(format!("{:.0}%", zoom * 100.0));
        }

        div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_3()
            .h(px(STATUS_BAR_HEIGHT))
            .px_2()
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .text_color(theme.foreground_muted)
            .child(left)
            .child(right)
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let zoom = self.app_settings.ui_zoom;
        let viewport_w = f32::from(window.viewport_size().width);
        let viewport_h = f32::from(window.viewport_size().height);
        if (viewport_w - self.viewport_width).abs() > 0.5 {
            if viewport_w >= NARROW_TREE_WIDTH {
                self.tree_forced_in_narrow = false;
            }
            self.viewport_width = viewport_w;
        }

        // Repaint once when the current notice expires so its toast goes away.
        if let Some((message, at)) = &self.notice
            && self.notice_timer != Some(*at)
        {
            self.notice_timer = Some(*at);
            let remaining = notice_duration(message).saturating_sub(at.elapsed());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(remaining).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }

        // Keep the OS-level title (taskbar, Alt+Tab) in sync with ours.
        let title = self.window_title(cx);
        if *self.last_os_title.borrow() != title {
            window.set_window_title(&format!("{title} — RJID"));
            *self.last_os_title.borrow_mut() = title;
        }

        // Every panel gets an explicit pixel size computed from the real
        // viewport, rather than relying on flex grow/shrink: in this GPUI
        // version a `flex_1` sibling next to a fixed-height child didn't
        // reliably get its share of space, so the layout is kept fully
        // deterministic.
        // Docking: the tree on the left or right, the terminal at the bottom
        // or on the right (a narrow window always puts it at the bottom).
        let show_terminal = self.app_settings.show_terminal;
        let terminal_right = show_terminal
            && self.app_settings.terminal_dock == TerminalDock::Right
            && viewport_w >= NARROW_SIDE_TERMINAL_WIDTH;
        let terminal_bottom = show_terminal && !terminal_right;
        let tree_right = self.app_settings.tree_side == PanelSide::Right;

        let available = viewport_h - TITLEBAR_HEIGHT - TOOLBAR_HEIGHT - STATUS_BAR_HEIGHT;
        let terminal_h = if terminal_bottom {
            self.app_settings
                .terminal_height
                .min(available - MIN_EDITOR_HEIGHT - SPLITTER_SIZE)
                .max(0.0)
        } else {
            0.0
        };
        let main_h = (available - terminal_h - if terminal_bottom { SPLITTER_SIZE } else { 0.0 }).max(0.0);
        let editor_h = (main_h - TAB_BAR_HEIGHT).max(0.0);
        let tree_w = if self.tree_visible() {
            // Never let the tree crowd out the editor in a small window.
            self.app_settings
                .tree_width
                .min(viewport_w * if viewport_w < NARROW_TREE_WIDTH { 0.6 } else { 0.45 })
        } else {
            0.0
        };
        let terminal_w = if terminal_right {
            self.app_settings.terminal_width.min(viewport_w * 0.5).max(0.0)
        } else {
            0.0
        };
        self.side_terminal_width = terminal_w;

        let editor_area = match self.active_tab.and_then(|idx| self.tabs.get(idx)) {
            Some(tab) => div()
                .w_full()
                .min_w_0()
                .h(px(editor_h))
                .child(tab.view.clone())
                .into_any_element(),
            None => self.render_welcome(theme, editor_h, cx).into_any_element(),
        };

        let tree_splitter = || {
            splitter(
                "tree-splitter",
                theme,
                true,
                self.resizing == Some(Splitter::Tree),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.resizing = Some(Splitter::Tree);
                    cx.stop_propagation();
                }),
            )
        };
        let center = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h(px(main_h))
            .child(self.render_tab_bar(theme, cx))
            .child(editor_area);

        let mut main_row = div().flex().flex_row().flex_shrink_0().w_full().h(px(main_h));
        if tree_w > 0.0 && !tree_right {
            main_row = main_row.child(self.render_tree_panel(theme, tree_w, main_h, cx)).child(tree_splitter());
        }
        main_row = main_row.child(center);
        if tree_w > 0.0 && tree_right {
            main_row = main_row.child(tree_splitter()).child(self.render_tree_panel(theme, tree_w, main_h, cx));
        }
        if terminal_right {
            main_row = main_row
                .child(splitter(
                    "terminal-side-splitter",
                    theme,
                    true,
                    self.resizing == Some(Splitter::TerminalSide),
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.resizing = Some(Splitter::TerminalSide);
                        cx.stop_propagation();
                    }),
                ))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(terminal_w))
                        .h(px(main_h))
                        .overflow_hidden()
                        .child(self.terminal.clone()),
                );
        }
        let dock_zones = self.render_dock_zones(theme, cx);

        div()
            .id("root")
            .track_focus(&self.focus_handle)
            .key_context("RJID")
            .capture_key_down(cx.listener(|this, event, window, cx| this.capture_key_down(event, window, cx)))
            .on_key_down(cx.listener(|this, event, window, cx| this.handle_key_down(event, window, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_root_mouse_move(event, window, cx)
            }))
            // Files and folders dragged in from the OS file manager.
            .drag_over::<gpui::ExternalPaths>(move |style, _, _, _| style.bg(theme.accent.opacity(0.08)))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| this.on_external_drop(paths, window, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_resize(cx)),
            )
            .when(matches!(self.resizing, Some(Splitter::Tree | Splitter::TerminalSide)), |s| s.cursor_col_resize())
            .when(self.resizing == Some(Splitter::Terminal), |s| s.cursor_row_resize())
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .text_size(px(14.0 * zoom))
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_titlebar(theme, window, cx))
            .child(self.render_toolbar(theme, viewport_w, cx))
            .child(main_row)
            .when(terminal_bottom, |root| {
                root.child(splitter(
                    "terminal-splitter",
                    theme,
                    false,
                    self.resizing == Some(Splitter::Terminal),
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.resizing = Some(Splitter::Terminal);
                        cx.stop_propagation();
                    }),
                ))
                .child(
                    div()
                        .flex_shrink_0()
                        .w_full()
                        .h(px(terminal_h))
                        .overflow_hidden()
                        .child(self.terminal.clone()),
                )
            })
            .child(self.render_status_bar(theme, viewport_w, cx))
            .children(dock_zones)
            .children(self.render_overlays(theme, TITLEBAR_HEIGHT, cx))
    }
}

// ---- helpers -------------------------------------------------------------------

/// The path given on the command line, if it exists (`rjid.exe <folder>` or
/// `rjid.exe <file>`).
fn command_line_path() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .map(normalize_path)
}

/// One canonical spelling per location (absolute, long-form — no 8.3
/// `AVIAV~1` names — no `\\?\` prefix). Tabs, diagnostics, and breakpoints
/// are all matched by path equality, and jdtls reports long-form paths, so
/// every path entering the app goes through this.
fn normalize_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path)
        .map(strip_verbatim_prefix)
        .unwrap_or(path)
}

/// `canonicalize` on Windows returns `\\?\C:\...`; the plain form is what
/// every other path in the app (and jdtls's URIs) uses.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => path,
    }
}

/// Startup workspace: a folder passed on the command line (or, for a file,
/// its enclosing project), else the most recent workspace that still
/// exists, else the current directory.
fn initial_workspace(settings: &AppSettings) -> Option<PathBuf> {
    command_line_path()
        .map(|p| if p.is_file() { project_root_for_file(&p) } else { p })
        .or_else(|| settings.recent_workspaces.iter().find(|p| p.is_dir()).cloned())
        .or_else(|| std::env::current_dir().ok())
        .map(normalize_path)
}

/// The nearest ancestor that looks like a project root, else the file's
/// own directory.
fn project_root_for_file(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(file).to_path_buf();
    parent
        .ancestors()
        .find(|dir| {
            ["pom.xml", "build.gradle", "build.gradle.kts", ".git"]
                .iter()
                .any(|marker| dir.join(marker).exists())
        })
        .map(Path::to_path_buf)
        .unwrap_or(parent)
}

/// Reads a file for editing, refusing anything that editing and saving
/// could corrupt: binary files, non-UTF-8 text, and very large files.
fn read_text_file(path: &Path) -> Result<String, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let metadata = std::fs::metadata(path).map_err(|err| format!("Cannot open {name}: {err}"))?;
    if metadata.len() > MAX_OPEN_FILE_BYTES {
        return Err(format!("{name} is too large to open ({} MB)", metadata.len() / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|err| format!("Cannot open {name}: {err}"))?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(format!("{name} looks like a binary file — not opened"));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").map(<[u8]>::to_vec).unwrap_or(bytes);
    String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8 text — not opened (saving could corrupt it)"))
}

/// `java version "25.0.2" 2026-01-20 LTS` -> `25.0.2`.
fn short_jdk_version(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return "?".to_string();
    };
    let mut quoted = raw.split('"');
    match (quoted.next(), quoted.next()) {
        (Some(_), Some(version)) if !version.is_empty() => version.to_string(),
        _ => raw.split_whitespace().last().unwrap_or("?").to_string(),
    }
}

fn gradle_invocation(project: &GradleProject) -> String {
    // The wrapper script lives in the workspace root and needs a relative
    // prefix (`.\` on Windows, `./` elsewhere); a bare `gradle` (no
    // wrapper) resolves via PATH.
    if project.wrapper_command == "gradle" {
        project.wrapper_command.clone()
    } else if cfg!(windows) {
        format!(".\\{}", project.wrapper_command)
    } else {
        format!("./{}", project.wrapper_command)
    }
}

/// jdtls needs a writable per-project cache (`-data`). Kept under this
/// app's own portable settings folder, not `%APPDATA%`.
fn jdtls_data_dir(workspace_root: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    rji_settings::app_dir()
        .join(".rji-settings")
        .join("jdtls-data")
        .join(format!("{:x}", hasher.finish()))
}

/// The recompile-on-save hook, when Spring Boot DevTools is on the
/// project's classpath (Maven checked first; a workspace has one build tool
/// in practice).
fn devtools_hook_for(
    workspace_root: Option<&Path>,
    maven_project: &Option<MavenProject>,
    gradle_project: &Option<GradleProject>,
) -> Option<DevtoolsHook> {
    let workspace_root = workspace_root?;
    if maven_project.as_ref().is_some_and(|p| p.has_devtools) {
        return Some(DevtoolsHook {
            workspace_root: workspace_root.to_path_buf(),
            compile_command: "mvn compile -q".to_string(),
        });
    }
    if let Some(project) = gradle_project.as_ref().filter(|p| p.has_devtools) {
        return Some(DevtoolsHook {
            workspace_root: workspace_root.to_path_buf(),
            compile_command: format!("{} compileJava -q", gradle_invocation(project)),
        });
    }
    None
}

/// Borderless toolbar button: a glyph, an optional label, a tooltip.
fn ghost_button(
    id: impl Into<gpui::ElementId>,
    glyph: &'static str,
    label: Option<String>,
    tooltip: String,
    color: gpui::Rgba,
    theme: Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(3.))
        .rounded_md()
        .cursor_pointer()
        .whitespace_nowrap()
        .text_color(color)
        .hover(|s| s.bg(theme.foreground.opacity(0.08)))
        .active(|s| s.bg(theme.foreground.opacity(0.14)))
        .tooltip(crate::tooltip::Tooltip::text(tooltip, theme))
        .child(div().text_color(theme.foreground_muted).child(glyph))
        .children(label)
        .on_click(on_click)
}

/// Square icon button for Run / Debug / Stop / stepping. Disabled ones are
/// dimmed and ignore clicks (the tooltip still explains them).
fn icon_action(
    id: &'static str,
    glyph: &'static str,
    tooltip: &'static str,
    color: gpui::Rgba,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .w(px(30.))
        .h(px(28.))
        .rounded_md()
        .text_size(px(15.))
        .tooltip(crate::tooltip::Tooltip::text(tooltip, theme))
        .text_color(if enabled { color } else { theme.foreground_muted.opacity(0.4) })
        .when(enabled, |b| {
            b.cursor_pointer()
                .hover(|s| s.bg(color.opacity(0.16)))
                .active(|s| s.bg(color.opacity(0.26)))
                .on_click(on_click)
        })
        .child(glyph)
}

fn toolbar_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    theme: Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_shrink_0()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .text_color(theme.accent)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(theme.accent.opacity(0.14)))
        .active(|s| s.bg(theme.accent.opacity(0.24)))
        .child(label.into())
        .on_click(on_click)
}

fn icon_button(
    id: &'static str,
    glyph: &'static str,
    tooltip: &'static str,
    theme: Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .tooltip(crate::tooltip::Tooltip::text(tooltip, theme))
        .w(px(20.))
        .flex()
        .justify_center()
        .rounded_sm()
        .text_color(theme.foreground_muted)
        .cursor_pointer()
        .hover(|s| s.bg(theme.accent.opacity(0.16)).text_color(theme.foreground))
        .child(glyph)
        .on_click(on_click)
}

fn panel_header(theme: Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .h(px(PANEL_HEADER_HEIGHT))
        .px_2()
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(11.))
        .text_color(theme.foreground_muted)
}

fn section_label(label: &'static str, theme: Theme) -> impl IntoElement {
    div().pt_1().text_size(px(10.)).text_color(theme.foreground_muted).child(label)
}

/// A draggable divider between panels. `vertical` = a column divider
/// (resizes widths).
fn splitter(
    id: &'static str,
    theme: Theme,
    vertical: bool,
    active: bool,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_shrink_0()
        .when(vertical, |s| s.w(px(SPLITTER_SIZE)).h_full().cursor_col_resize())
        .when(!vertical, |s| s.h(px(SPLITTER_SIZE)).w_full().cursor_row_resize())
        .bg(if active { theme.accent } else { theme.border })
        .hover(|s| s.bg(theme.accent))
        .on_mouse_down(MouseButton::Left, on_mouse_down)
}

#[cfg(test)]
mod tests {
    use super::read_text_file;

    #[test]
    fn refuses_binary_and_non_utf8_but_strips_bom() {
        let dir = std::env::temp_dir().join(format!("rji-read-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("a.class");
        std::fs::write(&bin, [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00]).unwrap();
        assert!(read_text_file(&bin).unwrap_err().contains("binary"));
        let latin1 = dir.join("b.txt");
        std::fs::write(&latin1, [b'c', b'a', b'f', 0xE9]).unwrap();
        assert!(read_text_file(&latin1).unwrap_err().contains("UTF-8"));
        let bom = dir.join("c.txt");
        std::fs::write(&bom, b"\xEF\xBB\xBFhello").unwrap();
        assert_eq!(read_text_file(&bom).unwrap(), "hello");
        std::fs::remove_dir_all(&dir).ok();
    }
}
