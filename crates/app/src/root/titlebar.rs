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
    Selection,
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
    const BAR: [MenuId; 9] = [
        MenuId::File,
        MenuId::Edit,
        MenuId::Selection,
        MenuId::View,
        MenuId::Go,
        MenuId::Run,
        MenuId::Terminal,
        MenuId::Plugins,
        MenuId::Help,
    ];

    fn label(self) -> &'static str {
        match self {
            MenuId::File => "File",
            MenuId::Edit => "Edit",
            MenuId::Selection => "Selection",
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
            "s" => MenuId::Selection,
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
    NewProject,
    OpenFolder,
    OpenFolderInNewWindow,
    OpenRecent(PathBuf),
    NewFile,
    NewFolder,
    Save,
    SaveAll,
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
    NextTheme,
    RefreshTree,
    NextTab,
    PreviousTab,
    Run,
    RunCommand(String),
    Debug,
    DebugCommand(String),
    AttachDebugger,
    Continue,
    Step(StepDepth),
    StopDebug,
    AddRunConfig,
    AndroidDevices,
    FocusTerminal,
    ClearTerminal,
    RestartTerminal,
    RunPlugin(usize, String),
    ManagePlugins,
    InstallPluginFromFolder,
    OpenPluginsFolder,
    ReloadPlugins,
    Shortcuts,
    CheckUpdates,
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
    fn menu_entries(&self, menu: MenuId) -> Vec<MenuEntry> {
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
                }
                entries.extend([
                    MenuEntry::Separator,
                    item_if(has_editor, "Save", Some("Ctrl+S"), A::Save),
                    item_if(has_editor, "Save All", Some("Ctrl+Shift+S"), A::SaveAll),
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
                MenuEntry::Header("Code".into()),
                item_if(has_editor, "Quick Fix…", Some("Ctrl+."), e(EditorCommand::QuickFix)),
                item_if(has_editor, "Rename Symbol…", Some("F2"), e(EditorCommand::Rename)),
                item_if(has_editor, "Refactor…", Some("Ctrl+Shift+R"), e(EditorCommand::Refactor)),
                item_if(has_editor, "Generate…", Some("Alt+Insert"), e(EditorCommand::ContextMenu)),
                item_if(has_editor, "Organize Imports", Some("Shift+Alt+O"), e(EditorCommand::OrganizeImports)),
                item_if(has_editor, "Format Document", Some("Shift+Alt+F"), e(EditorCommand::FormatDocument)),
            ],
            MenuId::Selection => vec![
                item_if(has_editor, "Select All", Some("Ctrl+A"), e(EditorCommand::SelectAll)),
                item_if(has_editor, "Select Line", Some("Ctrl+L"), e(EditorCommand::SelectLine)),
                item_if(has_editor, "Select Word", None, e(EditorCommand::SelectWord)),
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
                item(format!("Next Theme ({})", self.theme().kind.display_name()), None, A::NextTheme),
            ],
            MenuId::Go => vec![
                item_if(has_editor, "Go to Line…", Some("Ctrl+G"), e(EditorCommand::GoToLine)),
                MenuEntry::Separator,
                item_if(self.tabs.len() > 1, "Next Tab", Some("Ctrl+Tab"), A::NextTab),
                item_if(self.tabs.len() > 1, "Previous Tab", Some("Ctrl+Shift+Tab"), A::PreviousTab),
            ],
            MenuId::Run => self.run_menu_entries(),
            MenuId::Terminal => vec![
                item("Focus Terminal", None, A::FocusTerminal),
                item("Clear", Some("Ctrl+L"), A::ClearTerminal),
                item("Restart Shell", None, A::RestartTerminal),
            ],
            MenuId::Plugins => {
                // Plugin commands act on the open file, so they're listed only
                // while one is open (instead of a block of greyed-out items).
                let commands = self.plugin_commands();
                let mut entries: Vec<MenuEntry> = Vec::new();
                if commands.is_empty() {
                    entries.push(MenuEntry::Header("No plugins installed".into()));
                } else if !has_editor {
                    entries.push(MenuEntry::Header(format!(
                        "{} plugin command{} — open a file to use them",
                        commands.len(),
                        if commands.len() == 1 { "" } else { "s" }
                    )));
                } else {
                    entries.push(MenuEntry::Header("Run on the selection (or whole file)".into()));
                    entries.extend(commands.into_iter().map(|(plugin, command, label)| {
                        item_if(!self.plugin_state.running, label, None, A::RunPlugin(plugin, command))
                    }));
                }
                entries.extend([
                    MenuEntry::Separator,
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
                    && matches!(action, A::Run | A::RunCommand(_) | A::Debug | A::DebugCommand(_) | A::AttachDebugger);
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
            MenuEntry::Separator,
            item_if(session, "Continue", Some("F8"), A::Continue),
            item_if(session, "Step Over", Some("F10"), A::Step(StepDepth::Over)),
            item_if(session, "Step Into", Some("F11"), A::Step(StepDepth::Into)),
            item_if(session, "Step Out", Some("Shift+F11"), A::Step(StepDepth::Out)),
            item_if(session, "Stop", Some("Shift+F5"), A::StopDebug),
        ];

        // The detected project's tasks, with debug variants where the IDE
        // can attach.
        if let Some(project) = &self.project {
            entries.push(MenuEntry::Header(format!("{} tasks", project.kind_label())));
            for task in rji_project_java::project_tasks(project) {
                let debuggable = crate::debug_session::debug_launch(&task.command, 0).is_some();
                entries.push(item(task.label.clone(), None, A::RunCommand(task.command.clone())));
                if debuggable && task.group == rji_project_java::TaskGroup::Run {
                    entries.push(item(format!("{} (debug)", task.label), None, A::DebugCommand(task.command)));
                }
            }
        }
        entries
    }

    pub(super) fn run_menu_action(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        use MenuAction as A;
        self.open_menu = None;
        match action {
            A::NewProject => self.open_new_project(window, cx),
            A::OpenFolder => self.pick_folder(window, cx),
            A::OpenFolderInNewWindow => self.pick_folder_for_new_window(window, cx),
            A::OpenRecent(path) => self.open_folder(path, window, cx),
            A::NewFile => self.prompt_new_entry(false, None, window, cx),
            A::NewFolder => self.prompt_new_entry(true, None, window, cx),
            A::Save => {
                if let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) {
                    tab.view.update(cx, |editor, cx| {
                        editor.save();
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
            A::NextTheme => self.cycle_theme(cx),
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
            A::Run => self.run_active_config(cx),
            A::RunCommand(command) => self.run_in_terminal(&command, cx),
            A::Debug => self.start_debug(window, cx),
            A::DebugCommand(command) => self.debug_command(&command, window, cx),
            A::AttachDebugger => self.prompt_attach(window, cx),
            A::Continue => self.resume_debug(cx),
            A::Step(depth) => self.step(depth, cx),
            A::StopDebug => self.stop_debug(cx),
            A::AddRunConfig => self.open_run_configs(window, cx),
            A::AndroidDevices => self.open_android_panel(cx),
            A::FocusTerminal => {
                self.show_terminal();
                let handle = gpui::Focusable::focus_handle(self.terminal.read(cx), cx);
                window.focus(&handle, cx);
            }
            A::ClearTerminal => self.terminal.update(cx, |t, cx| t.clear(cx)),
            A::RestartTerminal => self.terminal.update(cx, |t, cx| t.restart(cx)),
            A::RunPlugin(plugin, command) => self.run_plugin_command(plugin, command, cx),
            A::ManagePlugins => self.show_plugins = true,
            A::InstallPluginFromFolder => self.install_plugin_from_folder(window, cx),
            A::OpenPluginsFolder => {
                let dir = super::plugins_ui::plugins_dir();
                let _ = std::fs::create_dir_all(&dir);
                cx.open_with_system(&dir);
            }
            A::ReloadPlugins => self.reload_plugins(cx),
            A::Shortcuts => self.show_shortcuts = true,
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
