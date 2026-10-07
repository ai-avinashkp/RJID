//! The custom title bar: app menus (File, Edit, …), the window title, and
//! the minimize / maximize / close buttons, drawn by the app in place of
//! the system title bar (see `main.rs`).
//!
//! On Windows the drag area and the three buttons are tagged with
//! `WindowControlArea`, which GPUI reports to Windows' own hit-testing —
//! so dragging, double-click-to-maximize, Aero Snap (incl. the snap
//! layouts flyout on the maximize button), and the close button's normal
//! `WM_CLOSE` (with our unsaved-changes prompt) all behave natively.
//!
//! Responsive: below `COMPACT_MENU_WIDTH` the menu names collapse into a
//! single ☰ button whose dropdown lists every menu.

use std::path::PathBuf;

use gpui::{
    Context, MouseButton, MouseDownEvent, Window, WindowControlArea, deferred, div, prelude::*, px,
};
use rji_jdwp_client::StepDepth;
use rji_theme::Theme;

use super::{DebugState, RootView};
use crate::editor_view::EditorCommand;

pub(super) const TITLEBAR_HEIGHT: f32 = 34.0;
/// Below this window width the menu names collapse into one ☰ menu.
pub(super) const COMPACT_MENU_WIDTH: f32 = 820.0;
/// Below this width the centered window title is hidden.
const HIDE_TITLE_WIDTH: f32 = 600.0;
const DROPDOWN_WIDTH: f32 = 290.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuId {
    File,
    Edit,
    View,
    Go,
    Run,
    Terminal,
    Plugins,
    Help,
    /// The collapsed ☰ menu (narrow windows): every menu in one list.
    All,
}

impl MenuId {
    pub(super) const BAR: [MenuId; 8] = [
        MenuId::File,
        MenuId::Edit,
        MenuId::View,
        MenuId::Go,
        MenuId::Run,
        MenuId::Terminal,
        MenuId::Plugins,
        MenuId::Help,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            MenuId::File => "File",
            MenuId::Edit => "Edit",
            MenuId::View => "View",
            MenuId::Go => "Go",
            MenuId::Run => "Run",
            MenuId::Terminal => "Terminal",
            MenuId::Plugins => "Plugins",
            MenuId::Help => "Help",
            MenuId::All => "☰",
        }
    }

    /// Alt+<letter> opens the menu, as in most desktop apps.
    pub(super) fn for_alt_key(key: &str) -> Option<MenuId> {
        Some(match key {
            "f" => MenuId::File,
            "e" => MenuId::Edit,
            "v" => MenuId::View,
            "g" => MenuId::Go,
            "r" => MenuId::Run,
            "t" => MenuId::Terminal,
            "p" => MenuId::Plugins,
            "h" => MenuId::Help,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) enum MenuAction {
    GoToFile,
    CommandPalette,
    NewProject,
    OpenFolder,
    OpenFolderInNewWindow,
    OpenRecent(PathBuf),
    ClearRecent,
    NewFile,
    NewFolder,
    Save,
    SaveAll,
    ToggleAutoSave,
    CloseTab,
    Exit,
    Editor(EditorCommand),
    ToggleTree,
    ToggleTerminal,
    MoveTree,
    MoveTerminal,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Appearance,
    RefreshTree,
    NextTab,
    PreviousTab,
    Run,
    RunCommand(String),
    Debug,
    AttachDebugger,
    Continue,
    Step(StepDepth),
    StopDebug,
    AddRunConfig,
    AndroidDevices,
    FocusTerminal,
    NewTerminal,
    CloseTerminal,
    ClearTerminal,
    RestartTerminal,
    ManagePlugins,
    InstallPluginFromFolder,
    OpenPluginsFolder,
    ReloadPlugins,
    Shortcuts,
    CheckUpdates,
    InstallJdtls,
    About,
}

pub(super) enum MenuEntry {
    Item {
        label: String,
        shortcut: Option<&'static str>,
        action: MenuAction,
        enabled: bool,
    },
    Separator,
    Header(String),
}

fn item(label: impl Into<String>, shortcut: Option<&'static str>, action: MenuAction) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        shortcut,
        action,
        enabled: true,
    }
}

fn item_if(enabled: bool, label: impl Into<String>, shortcut: Option<&'static str>, action: MenuAction) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        shortcut,
        action,
        enabled,
    }
}

impl RootView {
    pub(super) fn menu_entries(&self, menu: MenuId) -> Vec<MenuEntry> {
        use MenuAction as A;
        let has_editor = self.active_tab.is_some();
        let has_workspace = self.workspace_root.is_some();
        let debug = self.debug_state;
        let paused = debug == DebugState::Paused;
        let e = |cmd| A::Editor(cmd);
        match menu {
            MenuId::File => {
                let mut entries = vec![
                    item("New Project…", None, A::NewProject),
                    item_if(has_workspace, "New File…", None, A::NewFile),
                    item_if(has_workspace, "New Folder…", None, A::NewFolder),
                    MenuEntry::Separator,
                    item("Open Folder…", Some("Ctrl+O"), A::OpenFolder),
                    item("Open Folder in New Window…", None, A::OpenFolderInNewWindow),
                ];
                let recent: Vec<PathBuf> = self
                    .app_settings
                    .recent_workspaces
                    .iter()
                    .filter(|p| Some(p.as_path()) != self.workspace_root.as_deref())
                    .take(5)
                    .cloned()
                    .collect();
                if !recent.is_empty() {
                    entries.push(MenuEntry::Header("Open Recent".into()));
                    for path in recent {
                        let label = path
                            .file_name()
                            .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                        entries.push(item(label, None, A::OpenRecent(path)));
                    }
                    entries.push(item("Clear Recent Folders", None, A::ClearRecent));
                }
                entries.extend([
                    MenuEntry::Separator,
                    item_if(has_editor, "Save", Some("Ctrl+S"), A::Save),
                    item_if(has_editor, "Save All", Some("Ctrl+Shift+S"), A::SaveAll),
                    item(
                        if self.app_settings.auto_save { "✓ Auto Save (on focus change)" } else { "Auto Save (on focus change)" },
                        None,
                        A::ToggleAutoSave,
                    ),
                    MenuEntry::Separator,
                    item_if(has_editor, "Close Tab", Some("Ctrl+W"), A::CloseTab),
                    item("Exit", Some("Alt+F4"), A::Exit),
                ]);
                entries
            }
            MenuId::Edit => vec![
                item_if(has_editor, "Undo", Some("Ctrl+Z"), e(EditorCommand::Undo)),
                item_if(has_editor, "Redo", Some("Ctrl+Y"), e(EditorCommand::Redo)),
                MenuEntry::Separator,
                item_if(has_editor, "Cut", Some("Ctrl+X"), e(EditorCommand::Cut)),
                item_if(has_editor, "Copy", Some("Ctrl+C"), e(EditorCommand::Copy)),
                item_if(has_editor, "Paste", Some("Ctrl+V"), e(EditorCommand::Paste)),
                item_if(has_editor, "Select All", Some("Ctrl+A"), e(EditorCommand::SelectAll)),
                MenuEntry::Separator,
                item_if(has_editor, "Find", Some("Ctrl+F"), e(EditorCommand::Find)),
                item_if(has_editor, "Replace", Some("Ctrl+H"), e(EditorCommand::Replace)),
                item_if(has_editor, "Find Next", Some("F3"), e(EditorCommand::FindNext)),
                item_if(has_editor, "Find Previous", Some("Shift+F3"), e(EditorCommand::FindPrevious)),
                MenuEntry::Separator,
                item_if(has_editor, "Toggle Line Comment", Some("Ctrl+/"), e(EditorCommand::ToggleComment)),
                item_if(has_editor, "Duplicate Line", Some("Ctrl+D"), e(EditorCommand::DuplicateLine)),
                item_if(has_editor, "Indent", Some("Tab"), e(EditorCommand::Indent)),
                item_if(has_editor, "Outdent", Some("Shift+Tab"), e(EditorCommand::Outdent)),
                MenuEntry::Separator,
                item_if(has_editor, "Code Completion", Some("Ctrl+Space"), e(EditorCommand::Completion)),

            ],
            MenuId::View => vec![
                item(
                    if self.app_settings.show_tree { "Hide File Tree" } else { "Show File Tree" },
                    Some("Ctrl+B"),
                    A::ToggleTree,
                ),
                item(
                    if self.app_settings.show_terminal { "Hide Terminal" } else { "Show Terminal" },
                    Some("Ctrl+`"),
                    A::ToggleTerminal,
                ),
                item_if(has_workspace, "Refresh File Tree", None, A::RefreshTree),
                MenuEntry::Header("Layout (or drag a panel's header)".into()),
                item(
                    if self.app_settings.tree_side == rji_settings::PanelSide::Left { "Move File Tree to the Right" } else { "Move File Tree to the Left" },
                    None,
                    A::MoveTree,
                ),
                item(
                    if self.app_settings.terminal_dock == rji_settings::TerminalDock::Bottom { "Move Terminal to the Right" } else { "Move Terminal to the Bottom" },
                    None,
                    A::MoveTerminal,
                ),
                MenuEntry::Separator,
                item("Zoom In", Some("Ctrl+="), A::ZoomIn),
                item("Zoom Out", Some("Ctrl+-"), A::ZoomOut),
                item("Reset Zoom", Some("Ctrl+0"), A::ZoomReset),
                MenuEntry::Separator,
                item(format!("Theme & Fonts… ({})", self.theme().kind.display_name()), None, A::Appearance),
            ],
            MenuId::Go => vec![
                item_if(has_editor, "Go to Definition", Some("F12 / Ctrl+Click"), e(EditorCommand::GoToDefinition)),
                item_if(has_workspace, "Go to File…", Some("Ctrl+P"), A::GoToFile),
                item("Command Palette…", Some("Ctrl+Shift+P"), A::CommandPalette),
                MenuEntry::Separator,
                item_if(has_editor, "Go to Line…", Some("Ctrl+G"), e(EditorCommand::GoToLine)),
                MenuEntry::Separator,
                item_if(self.tabs.len() > 1, "Next Tab", Some("Ctrl+Tab"), A::NextTab),
                item_if(self.tabs.len() > 1, "Previous Tab", Some("Ctrl+Shift+Tab"), A::PreviousTab),
            ],
            MenuId::Run => self.run_menu_entries(),
            MenuId::Terminal => vec![
                item("New Terminal", Some("Ctrl+Shift+`"), A::NewTerminal),
                item("Focus Terminal", None, A::FocusTerminal),
                MenuEntry::Separator,
                item("Clear", Some("Ctrl+L"), A::ClearTerminal),
                item("Restart Shell", None, A::RestartTerminal),
                item_if(!self.terminals.is_empty(), "Close Terminal", Some("exit"), A::CloseTerminal),
            ],
            MenuId::Plugins => {
                // Plugin commands are run from Manage Plugins (one button per
                // command), keeping this menu to plugin management.
                let mut entries: Vec<MenuEntry> = Vec::new();
                entries.extend([
                    item("Manage Plugins…", None, A::ManagePlugins),
                    item("Install Plugin from Folder…", None, A::InstallPluginFromFolder),
                    item("Open Plugins Folder", None, A::OpenPluginsFolder),
                    item("Reload Plugins", None, A::ReloadPlugins),
                ]);
                entries
            }
            MenuId::Help => vec![
                item("Keyboard Shortcuts", None, A::Shortcuts),
                item("Check for Toolchain Updates…", None, A::CheckUpdates),
                item_if(
                    self.lsp_status == super::LspStatus::NotInstalled && self.jdtls_download.is_none(),
                    "Install Java Language Server…",
                    None,
                    A::InstallJdtls,
                ),
                item("About RJID", None, A::About),
            ],
            MenuId::All => MenuId::BAR
                .iter()
                .flat_map(|&menu| {
                    std::iter::once(MenuEntry::Header(menu.label().to_string())).chain(
                        self.menu_entries(menu)
                            .into_iter()
                            .filter(|e| !matches!(e, MenuEntry::Header(_))),
                    )
                })
                .collect(),
        }
        .into_iter()
        .map(|entry| match entry {
            // Nothing else runs while a debug session owns the program.
            MenuEntry::Item { label, shortcut, action, enabled } => {
                let blocked = debug != DebugState::Idle
                    && matches!(action, A::Run | A::RunCommand(_) | A::Debug | A::AttachDebugger);
                let needs_pause = matches!(action, A::Continue | A::Step(_));
                MenuEntry::Item {
                    label,
                    shortcut,
                    enabled: enabled && !blocked && (!needs_pause || paused),
                    action,
                }
            }
            other => other,
        })
        .collect()
    }

    fn run_menu_entries(&self) -> Vec<MenuEntry> {
        use MenuAction as A;
        let config = self.active_run_config();
        let session = self.debug_state != DebugState::Idle;
        let mut entries = vec![
            item_if(config.is_some(), "Run", Some("F5"), A::Run),
            item_if(
                config
                    .as_ref()
                    .is_some_and(|c| crate::debug_session::debug_launch(&c.command, 0).is_some()),
                "Debug",
                None,
                A::Debug,
            ),
            item_if(self.workspace_root.is_some(), "Attach to JVM…", None, A::AttachDebugger),
            item_if(self.workspace_root.is_some(), "Run Configurations…", None, A::AddRunConfig),
            item("Android Devices…", None, A::AndroidDevices),
        ];
        // Stepping only while debugging (not a block of greyed-out items).
        if session {
            entries.extend([
                MenuEntry::Separator,
                item("Continue", Some("F8"), A::Continue),
                item("Step Over", Some("F10"), A::Step(StepDepth::Over)),
                item("Step Into", Some("F11"), A::Step(StepDepth::Into)),
                item("Step Out", Some("Shift+F11"), A::Step(StepDepth::Out)),
                item("Stop", Some("Shift+F5"), A::StopDebug),
            ]);
        }

        // The detected project's tasks (Debug above debugs the selected one).
        if let Some(project) = &self.project {
            entries.push(MenuEntry::Header(format!("{} tasks", project.kind_label())));
            for task in rji_project_java::project_tasks(project) {
                entries.push(item(task.label.clone(), None, A::RunCommand(task.command)));
            }
        }
        entries
    }

    pub(super) fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        use MenuAction as A;
        self.open_menu = None;
        match action {
            A::NewProject => self.open_new_project(window, cx),
            A::GoToFile => self.open_palette(false, window, cx),
            A::CommandPalette => self.open_palette(true, window, cx),
            A::OpenFolder => self.pick_folder(window, cx),
            A::OpenFolderInNewWindow => self.pick_folder_for_new_window(window, cx),
            A::OpenRecent(path) => self.open_folder(path, window, cx),
            A::ClearRecent => {
                // Keep only the open folder (it's reopened on the next start).
                let current = self.workspace_root.clone();
                self.app_settings.recent_workspaces.retain(|p| Some(p) == current.as_ref());
                self.save_app_settings();
                self.notify_user("Recent folders cleared");
            }
            A::NewFile => self.prompt_new_entry(false, None, window, cx),
            A::NewFolder => self.prompt_new_entry(true, None, window, cx),
            A::Save => {
                if let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) {
                    tab.view.update(cx, |editor, cx| {
                        editor.save(cx);
                        cx.notify();
                    });
                }
            }
            A::SaveAll => {
                if !self.save_tabs(None, cx) {
                    self.notify_user("Some files could not be saved");
                }
            }
            A::CloseTab => {
                if let Some(idx) = self.active_tab {
                    self.close_tab(idx, window, cx);
                }
            }
            A::Exit => {
                if self.request_close(window, cx) {
                    window.remove_window();
                }
            }
            A::Editor(command) => {
                if let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) {
                    tab.view.update(cx, |editor, cx| editor.command(command, window, cx));
                }
            }
            A::ToggleTree => self.toggle_tree(cx),
            A::ToggleTerminal => self.toggle_terminal(window, cx),
            A::ZoomIn => self.set_zoom(self.app_settings.ui_zoom + rji_settings::ZOOM_STEP, cx),
            A::ZoomOut => self.set_zoom(self.app_settings.ui_zoom - rji_settings::ZOOM_STEP, cx),
            A::ZoomReset => self.set_zoom(1.0, cx),
            A::Appearance => self.open_appearance(window, cx),
            A::ToggleAutoSave => {
                self.app_settings.auto_save = !self.app_settings.auto_save;
                self.save_app_settings();
                self.notify_user(if self.app_settings.auto_save { "Auto save on: edits are saved when the editor loses focus" } else { "Auto save off" });
            }
            A::MoveTree => {
                let side = if self.app_settings.tree_side == rji_settings::PanelSide::Left { rji_settings::PanelSide::Right } else { rji_settings::PanelSide::Left };
                self.dock_tree(side, cx);
            }
            A::MoveTerminal => {
                let dock = if self.app_settings.terminal_dock == rji_settings::TerminalDock::Bottom { rji_settings::TerminalDock::Right } else { rji_settings::TerminalDock::Bottom };
                self.dock_terminal(dock, cx);
            }
            A::RefreshTree => self.refresh_tree(cx),
            A::NextTab => self.cycle_tab(true, window, cx),
            A::PreviousTab => self.cycle_tab(false, window, cx),
            A::Run => self.run_active_config(window, cx),
            A::RunCommand(command) => self.run_in_terminal(&command, cx),
            A::Debug => self.start_debug(window, cx),
            A::AttachDebugger => self.prompt_attach(window, cx),
            A::Continue => self.resume_debug(cx),
            A::Step(depth) => self.step(depth, cx),
            A::StopDebug => self.stop_debug(cx),
            A::AddRunConfig => self.open_run_configs(window, cx),
            A::AndroidDevices => self.open_android_panel(cx),
            A::FocusTerminal => {
                self.show_terminal();
                self.focus_terminal(window, cx);
            }
            A::NewTerminal => {
                self.show_terminal();
                self.new_terminal(cx);
                self.focus_terminal(window, cx);
            }
            A::ClearTerminal => {
                if let Some(view) = self.active_terminal().cloned() {
                    view.update(cx, |t, cx| t.clear(cx));
                }
            }
            A::RestartTerminal => {
                if let Some(view) = self.active_terminal().cloned() {
                    view.update(cx, |t, cx| t.restart(cx));
                }
            }
            A::CloseTerminal => {
                let index = self.active_terminal;
                self.close_terminal(index, cx);
            }
            A::ManagePlugins => self.show_plugins = true,
            A::InstallPluginFromFolder => self.install_plugin_from_folder(window, cx),
            A::OpenPluginsFolder => {
                let dir = super::plugins_ui::plugins_dir();
                let _ = std::fs::create_dir_all(&dir);
                cx.open_with_system(&dir);
            }
            A::ReloadPlugins => self.reload_plugins(cx),
            A::Shortcuts => self.show_shortcuts = true,
            A::InstallJdtls => self.offer_jdtls_download(window, cx),
            A::CheckUpdates => self.check_for_updates(true, cx),
            A::About => self.show_about(window, cx),
        }
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: MenuId, cx: &mut Context<Self>) {
        self.open_menu = if self.open_menu == Some(menu) { None } else { Some(menu) };
        cx.notify();
    }

    pub(super) fn render_titlebar(&self, theme: Theme, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        let compact = width < COMPACT_MENU_WIDTH;
        let menus: Vec<MenuId> = if compact { vec![MenuId::All] } else { MenuId::BAR.to_vec() };

        let menu_buttons = menus.into_iter().map(|menu| {
            let open = self.open_menu == Some(menu);
            div()
                .relative()
                .h_full()
                .child(
                    div()
                        .id(("menu", menu as usize))
                        .h_full()
                        .flex()
                        .items_center()
                        .px(px(9.))
                        .text_color(if open { theme.foreground } else { theme.foreground_muted })
                        .when(open, |s| s.bg(theme.accent.opacity(0.18)))
                        .hover(|s| s.bg(theme.accent.opacity(0.12)).text_color(theme.foreground))
                        .child(menu.label())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.toggle_menu(menu, cx);
                                cx.stop_propagation();
                            }),
                        )
                        // Once any menu is open, hovering another switches to it.
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.open_menu.is_some() && this.open_menu != Some(menu) {
                                this.open_menu = Some(menu);
                                cx.notify();
                            }
                        })),
                )
                .when(open, |container| container.child(self.render_dropdown(menu, theme, window, cx)))
        });

        let title = self.window_title(cx);

        let mut bar = div()
            .id("titlebar")
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(13.))
            .text_color(theme.foreground_muted);
        if cfg!(target_os = "macos") {
            // Leave room for the traffic-light buttons.
            bar = bar.child(div().w(px(72.)).h_full().window_control_area(WindowControlArea::Drag));
        }
        bar.child(
            div()
                .flex_shrink_0()
                .h_full()
                .flex()
                .items_center()
                .px_2()
                .text_color(theme.accent)
                .child("RJ"),
        )
        .children(menu_buttons)
        // Everything between the menus and the window buttons drags the
        // window (and double-clicks to maximize).
        .child(
            div()
                .id("titlebar-drag")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                // While a menu is open this area is a plain click target that
                // closes it (a native drag region never reports the click).
                .when(self.open_menu.is_none(), |s| s.window_control_area(WindowControlArea::Drag))
                .when(self.open_menu.is_some(), |s| {
                    s.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.open_menu = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                })
                .when(!cfg!(target_os = "windows") && self.open_menu.is_none(), |s| {
                    s.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                })
                .when(width >= HIDE_TITLE_WIDTH, |s| {
                    s.child(div().whitespace_nowrap().overflow_hidden().child(title))
                }),
        )
        // macOS draws its own traffic lights; Linux keeps the system title
        // bar (GPUI can't hide it there), so custom buttons are Windows-only.
        .when(cfg!(target_os = "windows"), |bar| bar.child(window_buttons(theme, window)))
    }

    /// "App.java — sample-app" style title, also pushed to the OS window
    /// title (taskbar, Alt+Tab) when it changes.
    pub(super) fn window_title(&self, cx: &Context<Self>) -> String {
        let workspace = self
            .workspace_root
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let file = self.active_tab.and_then(|i| self.tabs.get(i)).map(|tab| {
            let editor = tab.view.read(cx);
            let name = editor.file_name();
            if editor.dirty { format!("● {name}") } else { name }
        });
        match (file, workspace) {
            (Some(file), Some(ws)) => format!("{file} — {ws}"),
            (None, Some(ws)) => ws,
            (Some(file), None) => file,
            (None, None) => "RJID".to_string(),
        }
    }

    fn render_dropdown(&self, menu: MenuId, theme: Theme, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let viewport_h = f32::from(window.viewport_size().height);
        let max_h = (viewport_h - TITLEBAR_HEIGHT - 16.0).max(120.0);
        let rows = self.menu_entries(menu).into_iter().enumerate().map(|(i, entry)| match entry {
            MenuEntry::Separator => div().my_1().h(px(1.)).bg(theme.border).into_any_element(),
            MenuEntry::Header(text) => div()
                .px_3()
                .pt_1()
                .text_size(px(11.))
                .text_color(theme.accent)
                .child(text)
                .into_any_element(),
            MenuEntry::Item {
                label,
                shortcut,
                action,
                enabled,
            } => div()
                .id(("menu-item", i))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_4()
                .h(px(26.))
                .px_3()
                .mx_1()
                .rounded_sm()
                .text_color(if enabled { theme.foreground } else { theme.foreground_muted.opacity(0.6) })
                .when(enabled, |s| s.cursor_pointer().hover(|s| s.bg(theme.accent.opacity(0.22))))
                .child(div().whitespace_nowrap().overflow_hidden().child(label))
                .children(shortcut.map(|keys| {
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(theme.foreground_muted)
                        .child(keys)
                }))
                .when(enabled, |s| {
                    s.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.run_menu_action(action.clone(), window, cx);
                        }),
                    )
                })
                .into_any_element(),
        });

        // `deferred` paints the dropdown after (above) the rest of the
        // window, although it's laid out here under its menu button.
        deferred(
            div()
                .id(("dropdown", menu as usize))
                .absolute()
                .top(px(TITLEBAR_HEIGHT))
                .left_0()
                .w(px(DROPDOWN_WIDTH))
                .max_h(px(max_h))
                .overflow_y_scroll()
                .py_1()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_b_md()
                .shadow_lg()
                .text_size(px(13.))
                // Wheel and clicks stay in the menu (never scroll the editor below).
                .occlude()
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .children(rows),
        )
        .with_priority(1)
    }
}

/// Minimize / maximize-restore / close. Windows handles the clicks itself
/// via `WindowControlArea` (so Snap layouts and `WM_CLOSE` — and with it
/// the unsaved-changes prompt — work natively).
fn window_buttons(theme: Theme, window: &Window) -> impl IntoElement {
    let button = |id: &'static str, glyph: &'static str, area: WindowControlArea, danger: bool| {
        div()
            .id(id)
            .w(px(46.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.foreground)
            .window_control_area(area)
            .when(danger, |s| s.hover(|s| s.bg(gpui::rgb(0xc42b1c)).text_color(gpui::white())))
            .when(!danger, |s| s.hover(|s| s.bg(theme.foreground.opacity(0.10))))
            .child(glyph)
    };
    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .h_full()
        .child(button("win-min", "—", WindowControlArea::Min, false))
        .child(button(
            "win-max",
            if window.is_maximized() { "❐" } else { "☐" },
            WindowControlArea::Max,
            false,
        ))
        .child(button("win-close", "✕", WindowControlArea::Close, true))
}
