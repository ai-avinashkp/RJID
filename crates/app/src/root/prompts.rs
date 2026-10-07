//! Window-level overlays: the modal text prompt (New File/Folder, Rename,
//! Attach to JVM), the file tree's right-click menu, and the keyboard
//! shortcut sheet.

use std::path::{Path, PathBuf};

use gpui::{
    ClipboardItem, Context, Entity, Focusable, MouseButton, MouseDownEvent, Pixels, Point,
    PromptLevel, Subscription, Window, deferred, div, prelude::*, px,
};
use rji_theme::Theme;

use super::RootView;
use crate::text_input::{TextInput, TextInputEvent};
use rji_project_java::java_source::{JavaKind, create_java_type, create_package, package_for_dir, source_root_for};

/// Every shortcut, for the Help sheet and the welcome page.
pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+O", "Open folder"),
    ("Ctrl+S / Ctrl+Shift+S", "Save / save all"),
    ("Ctrl+W / Ctrl+Tab", "Close / switch tab"),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
    ("Ctrl+X / C / V", "Cut / copy / paste (whole line if nothing selected)"),
    ("Ctrl+F / Ctrl+H", "Find / replace"),
    ("F3 / Shift+F3", "Next / previous match"),
    ("Ctrl+P / Ctrl+Shift+P", "Go to file / command palette"),
    ("F12 / Ctrl+Click", "Go to definition"),
    ("Ctrl+G", "Go to line"),
    ("Ctrl+Space", "Code completion"),
    ("Ctrl+. / F2", "Quick fix / rename symbol"),
    ("Shift+Alt+S / Alt+Insert", "Source action: generate, override, organize imports"),
    ("Shift+Alt+O / Shift+Alt+F", "Organize imports / format document"),
    ("Ctrl+/", "Toggle line comment"),
    ("Ctrl+D / Ctrl+L", "Duplicate line / select line"),
    ("Tab / Shift+Tab", "Indent / outdent"),
    ("Ctrl+B / Ctrl+`", "Toggle file tree / terminal"),
    ("Ctrl+= / Ctrl+- / Ctrl+0", "Zoom in / out / reset"),
    ("F5 / Shift+F5", "Run / stop debugging"),
    ("F8 / F10 / F11 / Shift+F11", "Continue / step over / into / out"),
    ("Alt+F, Alt+E, …", "Open a menu"),
];

pub(super) enum PromptKind {
    NewFile { dir: PathBuf },
    NewFolder { dir: PathBuf },
    NewJavaType { dir: PathBuf, kind: JavaKind },
    NewJavaPackage { dir: PathBuf },
    Rename { path: PathBuf },
    Attach,
}

pub(super) struct InputPrompt {
    kind: PromptKind,
    title: String,
    hint: String,
    input: Entity<TextInput>,
    error: Option<String>,
    _subscription: Subscription,
}

pub(super) struct TreeContextMenu {
    path: PathBuf,
    is_dir: bool,
    position: Point<Pixels>,
}

impl RootView {
    fn open_input_prompt(
        &mut self,
        kind: PromptKind,
        title: impl Into<String>,
        hint: impl Into<String>,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme();
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", theme, cx);
            input.set_text(initial, cx);
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Submit { .. } => this.submit_prompt(window, cx),
            TextInputEvent::Cancel => this.close_prompt(window, cx),
            TextInputEvent::Changed => {
                if let Some(prompt) = this.prompt.as_mut() {
                    prompt.error = None;
                    cx.notify();
                }
            }
        });
        let handle = input.focus_handle(cx);
        self.prompt = Some(InputPrompt {
            kind,
            title: title.into(),
            hint: hint.into(),
            input,
            error: None,
            _subscription: subscription,
        });
        self.context_menu = None;
        self.open_menu = None;
        window.focus(&handle, cx);
        cx.notify();
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    /// The directory new files go in by default: the active file's folder
    /// (if it's inside the workspace), else the workspace root.
    fn default_new_entry_dir(&self) -> Option<PathBuf> {
        let root = self.workspace_root.clone()?;
        let active_dir = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| t.path.parent())
            .filter(|dir| dir.starts_with(&root))
            .map(Path::to_path_buf);
        Some(active_dir.unwrap_or(root))
    }

    pub(super) fn prompt_new_entry(&mut self, is_dir: bool, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = dir.or_else(|| self.default_new_entry_dir()) else {
            return;
        };
        let shown = self.relative_display(&dir);
        let (kind, title) = if is_dir {
            (PromptKind::NewFolder { dir }, "New Folder")
        } else {
            (PromptKind::NewFile { dir }, "New File")
        };
        self.open_input_prompt(
            kind,
            title,
            format!("In {shown} — use / for subfolders"),
            "",
            window,
            cx,
        );
    }

    /// New Java File…: a name plus the kind (class, interface, …) chosen
    /// with the chips in the prompt.
    pub(super) fn prompt_new_java_type(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let hint = match package_for_dir(&dir) {
            Some(pkg) if !pkg.is_empty() => format!("In package {pkg} — type a name, or sub.package.Name"),
            Some(_) => "In the default package — type a name, or package.Name".to_string(),
            None => format!("In {} — type a name", self.relative_display(&dir)),
        };
        self.open_input_prompt(PromptKind::NewJavaType { dir, kind: JavaKind::Class }, "New Java File", hint, "", window, cx);
    }

    /// New Java Package…: a fully-qualified name, prefilled with the
    /// folder's own package.
    pub(super) fn prompt_new_java_package(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let initial = package_for_dir(&dir).filter(|p| !p.is_empty()).map(|p| format!("{p}.")).unwrap_or_default();
        let root = source_root_for(&dir).unwrap_or_else(|| dir.clone());
        let hint = format!("Created under {}", self.relative_display(&root));
        self.open_input_prompt(PromptKind::NewJavaPackage { dir }, "New Java Package", hint, &initial, window, cx);
        // Caret after the prefilled "com.example." rather than selecting it.
        if let Some(prompt) = self.prompt.as_ref() {
            prompt.input.update(cx, |input, cx| input.move_to_end(cx));
        }
    }

    pub(super) fn prompt_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let hint = format!("Rename {}", self.relative_display(&path));
        self.open_input_prompt(PromptKind::Rename { path }, "Rename", hint, &name, window, cx);
    }

    pub(super) fn prompt_attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_input_prompt(
            PromptKind::Attach,
            "Attach to JVM",
            "host:port of a JVM started with -agentlib:jdwp=transport=dt_socket,server=y,address=5005",
            "localhost:5005",
            window,
            cx,
        );
    }

    fn relative_display(&self, path: &Path) -> String {
        match self.workspace_root.as_deref().and_then(|root| path.strip_prefix(root).ok()) {
            Some(rel) if rel.as_os_str().is_empty() => "the project root".to_string(),
            Some(rel) => rel.display().to_string(),
            None => path.display().to_string(),
        }
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Taken out while acting on it; put back only if validation fails.
        let Some(mut prompt) = self.prompt.take() else {
            return;
        };
        let value = prompt.input.read(cx).text().trim().to_string();
        let result = match &prompt.kind {
            PromptKind::NewFile { dir } => create_entry(dir, &value, false).map(|p| (Some(p), false)),
            PromptKind::NewFolder { dir } => create_entry(dir, &value, true).map(|p| (Some(p), true)),
            PromptKind::NewJavaType { dir, kind } => create_java_type(dir, *kind, &value).map(|p| (Some(p), false)),
            PromptKind::NewJavaPackage { dir } => create_package(dir, &value).map(|p| (Some(p), true)),
            PromptKind::Rename { path } => rename_entry(path, &value).map(|p| {
                self.retarget_tabs(path, &p, cx);
                (None, false)
            }),
            PromptKind::Attach => parse_attach_address(&value).map(|addr| {
                self.attach_debugger(addr, window, cx);
                (None, false)
            }),
        };
        match result {
            Ok((created, is_dir)) => {
                // Opening a new file focuses it; otherwise return focus.
                let opens_file = created.is_some() && !is_dir;
                self.tree_cache.borrow_mut().clear();
                if let Some(path) = created {
                    // Make the new entry visible in the tree.
                    if let Some(root) = self.workspace_root.clone() {
                        let first = if is_dir { Some(path.as_path()) } else { path.parent() };
                        for dir in first.into_iter().flat_map(Path::ancestors) {
                            if !dir.starts_with(&root) || dir == root {
                                break;
                            }
                            self.expanded_dirs.insert(dir.to_path_buf());
                        }
                    }
                    if !is_dir {
                        self.open_file(path, window, cx);
                    }
                }
                if !opens_file && self.debug_state == super::DebugState::Idle {
                    self.focus_active_editor(window, cx);
                }
            }
            Err(message) => {
                prompt.error = Some(message);
                self.prompt = Some(prompt);
            }
        }
        cx.notify();
    }

    /// After a rename/move, point open tabs (and breakpoints) at the new
    /// path — including files inside a renamed folder.
    fn retarget_tabs(&mut self, old: &Path, new: &Path, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            if let Ok(rest) = tab.path.strip_prefix(old) {
                let moved = if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) };
                tab.path = moved.clone();
                tab.view.update(cx, |editor, cx| {
                    editor.path = moved;
                    cx.notify();
                });
            }
        }
        let moved: Vec<(PathBuf, _)> = self
            .breakpoints
            .keys()
            .filter(|p| p.starts_with(old))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|p| {
                let lines = self.breakpoints.remove(&p)?;
                let rest = p.strip_prefix(old).ok()?.to_path_buf();
                Some((if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) }, lines))
            })
            .collect();
        self.breakpoints.extend(moved);
        self.expanded_dirs = self
            .expanded_dirs
            .drain()
            .map(|d| match d.strip_prefix(old) {
                Ok(rest) if rest.as_os_str().is_empty() => new.to_path_buf(),
                Ok(rest) => new.join(rest),
                Err(_) => d,
            })
            .collect();
    }

    /// Asks, then deletes a file or folder (permanently — there's no
    /// cross-platform recycle bin API here, so the prompt says so).
    pub(super) fn delete_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_open {
            return;
        }
        self.prompt_open = true;
        let is_dir = path.is_dir();
        let message = format!(
            "Delete {} \"{}\"?",
            if is_dir { "folder" } else { "file" },
            self.relative_display(&path)
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("This permanently deletes it from disk and cannot be undone."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            this.update_in(cx, |this, window, cx| {
                this.prompt_open = false;
                if answer != Some(0) {
                    return;
                }
                let result = if is_dir {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                match result {
                    Ok(()) => {
                        // Close tabs for anything that no longer exists.
                        while let Some(idx) = this.tabs.iter().position(|t| t.path.starts_with(&path)) {
                            this.close_tab_now(idx, window, cx);
                        }
                        this.breakpoints.retain(|p, _| !p.starts_with(&path));
                        this.tree_cache.borrow_mut().clear();
                    }
                    Err(err) => this.notify_user(format!("Delete failed: {err}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_open {
            return;
        }
        self.prompt_open = true;
        let detail = format!(
            "Version {}\nRust for Java IDE — a Java IDE written in Rust on GPUI.\n\nSource-available under the PolyForm Noncommercial License 1.0.0: free for noncommercial use; commercial use needs the author's permission.\nThird-party components: THIRD_PARTY_LICENSES.md.",
            env!("CARGO_PKG_VERSION")
        );
        let answer = window.prompt(PromptLevel::Info, "RJID — Rust for Java IDE", Some(&detail), &["OK"], cx);
        cx.spawn_in(window, async move |this, cx| {
            let _ = answer.await;
            this.update(cx, |this, _| this.prompt_open = false).ok();
        })
        .detach();
    }

    pub(super) fn open_tree_context_menu(&mut self, path: PathBuf, is_dir: bool, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.context_menu = Some(TreeContextMenu { path, is_dir, position });
        cx.notify();
    }

    fn context_action(&mut self, action: ContextAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu.take() else {
            return;
        };
        let dir = if menu.is_dir {
            menu.path.clone()
        } else {
            menu.path.parent().map(Path::to_path_buf).unwrap_or_else(|| menu.path.clone())
        };
        match action {
            ContextAction::NewFile => self.prompt_new_entry(false, Some(dir), window, cx),
            ContextAction::NewFolder => self.prompt_new_entry(true, Some(dir), window, cx),
            ContextAction::NewJavaType => self.prompt_new_java_type(dir, window, cx),
            ContextAction::NewJavaPackage => self.prompt_new_java_package(dir, window, cx),
            ContextAction::Rename => self.prompt_rename(menu.path, window, cx),
            ContextAction::Delete => self.delete_path(menu.path, window, cx),
            ContextAction::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(menu.path.display().to_string()));
            }
            ContextAction::CopyRelativePath => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.relative_display(&menu.path)));
            }
            ContextAction::Reveal => cx.reveal_path(&menu.path),
            ContextAction::OpenInTerminal => {
                let command = if cfg!(windows) {
                    format!("Set-Location -LiteralPath '{}'", dir.display().to_string().replace('\'', "''"))
                } else {
                    format!("cd '{}'", dir.display().to_string().replace('\'', "'\\''"))
                };
                self.run_in_terminal(&command, cx);
                let shown = self.relative_display(&dir);
                self.notify_user(format!("Terminal is now in {}", if shown.is_empty() { "the project root".to_string() } else { shown }));
            }
        }
        cx.notify();
    }

    // ---- rendering ---------------------------------------------------------

    /// Everything that floats above the window: click-away backdrops,
    /// the tree context menu, the input prompt, and the shortcut sheet.
    pub(super) fn render_overlays(&self, theme: Theme, top_offset: f32, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        let mut layers = Vec::new();

        // Clicking anywhere else closes an open menu. The backdrop starts
        // below the title bar so hovering other menu names still works.
        if self.open_menu.is_some() || self.context_menu.is_some() {
            let top = if self.open_menu.is_some() { top_offset } else { 0.0 };
            layers.push(
                div()
                    .id("menu-backdrop")
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .occlude()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.open_menu = None;
                            this.context_menu = None;
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.context_menu = None;
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            );
        }

        if let Some(menu) = &self.context_menu {
            layers.push(self.render_context_menu(menu, theme, cx).into_any_element());
        }
        if let Some(prompt) = &self.prompt {
            layers.push(render_prompt(prompt, theme, cx).into_any_element());
        }
        if self.show_shortcuts {
            layers.push(render_shortcuts(theme, cx).into_any_element());
        }
        if self.show_updates {
            layers.push(self.render_updates_panel(theme, cx).into_any_element());
        }
        if self.show_plugins {
            layers.push(self.render_plugins_panel(theme, cx).into_any_element());
        }
        if self.show_android {
            layers.push(self.render_android_panel(theme, cx).into_any_element());
        }
        if let Some(panel) = self.render_new_project_panel(theme, cx) {
            layers.push(panel.into_any_element());
        }
        if let Some(panel) = self.render_run_configs_panel(theme, cx) {
            layers.push(panel.into_any_element());
        }
        if let Some(palette) = self.render_palette(theme, cx) {
            layers.push(palette.into_any_element());
        }
        if let Some(panel) = self.render_appearance(theme, cx) {
            layers.push(panel.into_any_element());
        }
        if let Some((message, at)) = &self.notice
            && at.elapsed() < super::notice_duration(message)
        {
            layers.push(
                deferred(
                    div()
                        .id("notice-toast")
                        .absolute()
                        .right(px(16.))
                        .bottom(px(super::STATUS_BAR_HEIGHT + 12.))
                        .max_w(px(460.))
                        .ml_4()
                        .px_3()
                        .py_2()
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.warning.opacity(0.7))
                        .rounded_lg()
                        .shadow_lg()
                        .text_size(px(12.))
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .child(message.clone())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
                .with_priority(4)
                .into_any_element(),
            );
        }
        layers
    }

    fn render_context_menu(&self, menu: &TreeContextMenu, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let mut entries: Vec<Option<(&'static str, ContextAction)>> = vec![
            Some(("New File…", ContextAction::NewFile)),
            Some(("New Folder…", ContextAction::NewFolder)),
            Some(("New Java File…", ContextAction::NewJavaType)),
            Some(("New Java Package…", ContextAction::NewJavaPackage)),
            None,
            Some(("Rename…", ContextAction::Rename)),
            Some(("Delete", ContextAction::Delete)),
            None,
            Some(("Copy Path", ContextAction::CopyPath)),
            Some(("Copy Relative Path", ContextAction::CopyRelativePath)),
            Some((
                if cfg!(windows) { "Reveal in File Explorer" } else { "Reveal in File Manager" },
                ContextAction::Reveal,
            )),
            Some((if menu.is_dir { "Open Terminal Here (cd)" } else { "Open Terminal in This Folder (cd)" }, ContextAction::OpenInTerminal)),
        ];
        // Java entries only where Java sources live.
        let dir = if menu.is_dir { menu.path.clone() } else { menu.path.parent().map(Path::to_path_buf).unwrap_or_default() };
        if self.project.is_none() && source_root_for(&dir).is_none() {
            entries.retain(|e| !matches!(e, Some((_, ContextAction::NewJavaType | ContextAction::NewJavaPackage))));
        }
        if self.workspace_root.as_deref() == Some(menu.path.as_path()) {
            // The workspace root itself can't be renamed or deleted here.
            entries.retain(|e| !matches!(e, Some((_, ContextAction::Rename | ContextAction::Delete))));
        }
        let rows = entries.into_iter().enumerate().map(|(i, entry)| match entry {
            None => div().my_1().h(px(1.)).bg(theme.border).into_any_element(),
            Some((label, action)) => div()
                .id(("ctx", i))
                .h(px(26.))
                .px_3()
                .mx_1()
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .when(action == ContextAction::Delete, |s| s.text_color(theme.error))
                .hover(|s| s.bg(theme.accent.opacity(0.22)))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.context_action(action, window, cx);
                    }),
                )
                .into_any_element(),
        });
        deferred(
            div()
                .id("tree-context-menu")
                .absolute()
                .left(menu.position.x)
                .top(menu.position.y)
                .w(px(230.))
                .py_1()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_lg()
                .text_size(px(13.))
                .text_color(theme.foreground)
                .children(rows),
        )
        .with_priority(2)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContextAction {
    NewFile,
    NewFolder,
    NewJavaType,
    NewJavaPackage,
    Rename,
    Delete,
    CopyPath,
    CopyRelativePath,
    Reveal,
    OpenInTerminal,
}

fn modal_backdrop(id: &'static str, theme: Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        .items_start()
        .pt(px(90.))
        .bg(theme.background.opacity(0.55))
        // Modal: clicks never reach what's underneath.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

fn render_prompt(prompt: &InputPrompt, theme: Theme, cx: &Context<RootView>) -> impl IntoElement {
    let button = |id: &'static str, label: &'static str, primary: bool| {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(if primary { theme.accent } else { theme.border })
            .text_color(if primary { theme.accent } else { theme.foreground })
            .hover(|s| s.bg(theme.accent.opacity(0.15)))
            .child(label)
    };
    deferred(
        modal_backdrop("prompt-backdrop", theme).child(
            div()
                .w(px(440.))
                .max_w_full()
                .mx_4()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_lg()
                .shadow_lg()
                .text_size(px(13.))
                .text_color(theme.foreground)
                .child(div().text_size(px(15.)).child(prompt.title.clone()))
                .child(div().text_color(theme.foreground_muted).text_size(px(12.)).child(prompt.hint.clone()))
                // New Java File: what kind of type to create.
                .children(match &prompt.kind {
                    PromptKind::NewJavaType { kind: current, .. } => Some(
                        div().flex().flex_row().flex_wrap().gap_1().children(JavaKind::ALL.iter().enumerate().map(|(i, &kind)| {
                            let selected = kind == *current;
                            div()
                                .id(("java-kind", i))
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .border_1()
                                .cursor_pointer()
                                .border_color(if selected { theme.accent } else { theme.border })
                                .bg(if selected { theme.accent.opacity(0.18) } else { theme.surface })
                                .text_color(if selected { theme.foreground } else { theme.foreground_muted })
                                .hover(|s| s.bg(theme.accent.opacity(0.12)))
                                .child(kind.label())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(PromptKind::NewJavaType { kind: k, .. }) = this.prompt.as_mut().map(|p| &mut p.kind) {
                                        *k = kind;
                                    }
                                    // Keep typing in the name box.
                                    if let Some(prompt) = this.prompt.as_ref() {
                                        let handle = prompt.input.focus_handle(cx);
                                        window.focus(&handle, cx);
                                    }
                                    cx.notify();
                                }))
                        })),
                    ),
                    _ => None,
                })
                .child(prompt.input.clone())
                .children(prompt.error.clone().map(|e| div().text_color(theme.error).child(e)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(button("prompt-cancel", "Cancel", false).on_click(
                            cx.listener(|this, _, window, cx| this.close_prompt(window, cx)),
                        ))
                        .child(button("prompt-ok", "OK", true).on_click(
                            cx.listener(|this, _, window, cx| this.submit_prompt(window, cx)),
                        )),
                ),
        ),
    )
    .with_priority(3)
}

fn render_shortcuts(theme: Theme, cx: &Context<RootView>) -> impl IntoElement {
    deferred(
        modal_backdrop("shortcuts-backdrop", theme)
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_shortcuts = false;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(560.))
                    .max_w_full()
                    .mx_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_4()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .shadow_lg()
                    .text_size(px(13.))
                    .child(
                        div()
                            .pb_2()
                            .text_size(px(16.))
                            .text_color(theme.foreground)
                            .child("Keyboard shortcuts"),
                    )
                    .children(SHORTCUTS.iter().map(|(keys, action)| {
                        div()
                            .flex()
                            .flex_row()
                            .gap_3()
                            .child(div().w(px(210.)).flex_shrink_0().text_color(theme.accent).child(*keys))
                            .child(div().text_color(theme.foreground).child(*action))
                    }))
                    .child(
                        div()
                            .pt_2()
                            .text_size(px(11.))
                            .text_color(theme.foreground_muted)
                            .child("Click anywhere or press Esc to close"),
                    ),
            ),
    )
    .with_priority(3)
}

/// Creates a file or folder named `name` (may contain `/` subfolders)
/// under `dir`. New `.java` files get a class skeleton with the package
/// derived from their location under a Maven/Gradle source root.
fn create_entry(dir: &Path, name: &str, is_dir: bool) -> Result<PathBuf, String> {
    let name = name.trim().trim_matches(['/', '\\']);
    if name.is_empty() {
        return Err("Enter a name".to_string());
    }
    if name.split(['/', '\\']).any(|part| part.is_empty() || part == "." || part == ".." || !valid_file_name(part)) {
        return Err("That name isn't allowed here".to_string());
    }
    let path = dir.join(name.replace('\\', "/"));
    if path.exists() {
        return Err(format!("{name} already exists"));
    }
    if is_dir {
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, new_file_template(&path)).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn rename_entry(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name.contains(['/', '\\']) || !valid_file_name(new_name) {
        return Err("Enter a valid name (no slashes)".to_string());
    }
    let target = path.with_file_name(new_name);
    if target == path {
        return Ok(target);
    }
    // Case-only renames are fine on Windows even though the target "exists".
    let case_only = target.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy());
    if target.exists() && !case_only {
        return Err(format!("{new_name} already exists"));
    }
    std::fs::rename(path, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Rejects characters Windows forbids in names (and control chars) —
/// checked on every platform so projects stay portable.
fn valid_file_name(part: &str) -> bool {
    !part.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control())
        && !part.ends_with(['.', ' '])
}

fn new_file_template(path: &Path) -> String {
    if path.extension().and_then(|e| e.to_str()) != Some("java") {
        return String::new();
    }
    let class = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Main");
    let package = java_package_for(path);
    let header = package.map(|p| format!("package {p};\n\n")).unwrap_or_default();
    format!("{header}public class {class} {{\n\n}}\n")
}

/// `…/src/main/java/com/example/Foo.java` -> `com.example`.
fn java_package_for(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .parent()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let root_end = parts
        .windows(3)
        .rposition(|w| w[0] == "src" && (w[1] == "main" || w[1] == "test") && w[2] == "java")
        .map(|i| i + 3)?;
    let package = parts[root_end..].join(".");
    (!package.is_empty()).then_some(package)
}

/// `5005`, `localhost:5005`, or `host:port` -> `host:port`.
fn parse_attach_address(value: &str) -> Result<String, String> {
    let value = value.trim();
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) => (if host.is_empty() { "127.0.0.1" } else { host }, port),
        None => ("127.0.0.1", value),
    };
    let host = if host.eq_ignore_ascii_case("localhost") { "127.0.0.1" } else { host };
    match port.parse::<u16>() {
        Ok(p) if p > 0 => Ok(format!("{host}:{p}")),
        _ => Err("Enter a port (e.g. 5005) or host:port".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_template_gets_package_from_source_root() {
        let path = Path::new("/p/app/src/main/java/com/example/Foo.java");
        assert_eq!(java_package_for(path).as_deref(), Some("com.example"));
        assert_eq!(new_file_template(path), "package com.example;\n\npublic class Foo {\n\n}\n");
        assert_eq!(new_file_template(Path::new("/p/Main.java")), "public class Main {\n\n}\n");
        assert_eq!(new_file_template(Path::new("/p/notes.txt")), "");
    }

    #[test]
    fn names_are_validated() {
        assert!(valid_file_name("App.java"));
        assert!(!valid_file_name("a:b"));
        assert!(!valid_file_name("trailing."));
    }

    #[test]
    fn create_and_rename_on_disk() {
        let dir = std::env::temp_dir().join(format!("rji-prompts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = create_entry(&dir, "pkg/Hello.java", false).unwrap();
        assert!(file.is_file());
        assert!(create_entry(&dir, "pkg/Hello.java", false).is_err());
        assert!(create_entry(&dir, "../escape.txt", false).is_err());
        let renamed = rename_entry(&file, "World.java").unwrap();
        assert!(renamed.is_file() && !file.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn attach_address_forms() {
        assert_eq!(parse_attach_address("5005").unwrap(), "127.0.0.1:5005");
        assert_eq!(parse_attach_address("localhost:8000").unwrap(), "127.0.0.1:8000");
        assert_eq!(parse_attach_address("10.0.0.2:5005").unwrap(), "10.0.0.2:5005");
        assert!(parse_attach_address("abc").is_err());
    }
}
