//! Go to File (Ctrl+P) and the command palette (Ctrl+Shift+P), in one box
//! like VS Code: type to fuzzy-search the project's files; start with `>`
//! to search every menu command instead. `App.java:42` opens at line 42.
//! It works without the file tree, so it's the way to open files in a
//! narrow window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Entity, Focusable, MouseButton, Subscription, Window, deferred, div, prelude::*, px};
use rji_theme::{Theme, icon_for};

use super::RootView;
use super::titlebar::{MenuAction, MenuEntry, MenuId};
use crate::fuzzy;
use crate::text_input::{TextInput, TextInputEvent};

const MAX_RESULTS: usize = 50;
const VISIBLE_ROWS: usize = 12;
/// The file list is re-scanned in the background when older than this.
const INDEX_TTL: Duration = Duration::from_secs(15);
const MAX_FILES: usize = 50_000;
/// Folders never listed (build output, VCS, IDE metadata).
const SKIP_DIRS: &[&str] = &["target", "build", "out", "bin", "node_modules", ".git", ".gradle", ".idea", ".vscode", ".rji", ".mvn", ".settings"];

pub(super) struct Palette {
    input: Entity<TextInput>,
    selected: usize,
    _subscription: Subscription,
}

/// Project files (relative paths, `/`-separated) for one workspace.
pub(super) struct FileIndex {
    root: PathBuf,
    built: Instant,
    files: Arc<Vec<String>>,
}

enum Row {
    File { rel: String, open: bool },
    Command { label: String, shortcut: Option<&'static str>, action: MenuAction },
}

/// Every file under `root`, skipping build output and hidden folders.
fn scan_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(entry.path());
                }
            } else if let Ok(rel) = entry.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
                if out.len() >= MAX_FILES {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

impl RootView {
    /// Opens the palette; `commands` starts it in command mode (`>`).
    pub(super) fn open_palette(&mut self, commands: bool, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(
                if commands { "Type a command…" } else { "Search files by name (append :line to jump) — type > for commands" },
                theme,
                cx,
            );
            if commands {
                input.set_text(">", cx);
                input.move_to_end(cx);
            }
            input
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => {
                if let Some(p) = this.palette.as_mut() {
                    p.selected = 0;
                }
                cx.notify();
            }
            TextInputEvent::Submit { .. } => this.accept_palette(None, window, cx),
            TextInputEvent::Cancel => this.close_palette(window, cx),
        });
        let focus = input.focus_handle(cx);
        self.palette = Some(Palette { input, selected: 0, _subscription: subscription });
        self.open_menu = None;
        self.context_menu = None;
        window.focus(&focus, cx);
        self.refresh_file_index(cx);
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    /// Re-scans the workspace in the background if the list is missing or
    /// stale; the old list stays usable meanwhile.
    fn refresh_file_index(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace_root.clone() else { return };
        let fresh = self
            .file_index
            .as_ref()
            .is_some_and(|i| i.root == root && i.built.elapsed() < INDEX_TTL);
        if fresh || self.file_index_scanning {
            return;
        }
        self.file_index_scanning = true;
        let scan_root = root.clone();
        let task = cx.background_executor().spawn(async move { scan_files(&scan_root) });
        cx.spawn(async move |this, cx| {
            let files = task.await;
            this.update(cx, |this, cx| {
                this.file_index_scanning = false;
                if this.workspace_root.as_ref() == Some(&root) {
                    this.file_index = Some(FileIndex { root, built: Instant::now(), files: Arc::new(files) });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Up/Down while the palette is open (called from the window's key
    /// capture, so the input never sees them). Returns true if handled.
    pub(super) fn palette_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if !matches!(key, "up" | "down" | "pageup" | "pagedown") {
            return false;
        }
        let count = self.palette_rows(cx).len();
        let Some(p) = self.palette.as_mut() else { return false };
        if count == 0 {
            return true;
        }
        let step = if key.starts_with("page") { VISIBLE_ROWS } else { 1 };
        p.selected = if key.ends_with("up") {
            p.selected.saturating_sub(step)
        } else {
            (p.selected + step).min(count - 1)
        };
        cx.notify();
        true
    }

    fn palette_rows(&self, cx: &Context<Self>) -> Vec<Row> {
        let Some(p) = self.palette.as_ref() else { return Vec::new() };
        let query = p.input.read(cx).text().to_string();
        if let Some(command_query) = query.strip_prefix('>') {
            let commands = self.palette_commands();
            let labels: Vec<&str> = commands.iter().map(|(label, ..)| label.as_str()).collect();
            return fuzzy::rank(command_query, labels.iter().copied(), MAX_RESULTS, false)
                .into_iter()
                .map(|i| {
                    let (label, shortcut, action) = commands[i].clone();
                    Row::Command { label, shortcut, action }
                })
                .collect();
        }

        let (name_query, _) = fuzzy::split_line_suffix(query.trim());
        let root = self.workspace_root.as_deref();
        let open: Vec<String> = self
            .tabs
            .iter()
            .filter_map(|t| t.path.strip_prefix(root?).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
            .collect();
        let Some(index) = self.file_index.as_ref() else {
            // Still scanning: open tabs are all we know about.
            return open.into_iter().map(|rel| Row::File { rel, open: true }).collect();
        };
        if name_query.is_empty() {
            // Open files first, then the rest alphabetically.
            let mut rows: Vec<Row> = open.iter().map(|rel| Row::File { rel: rel.clone(), open: true }).collect();
            rows.extend(
                index
                    .files
                    .iter()
                    .filter(|f| !open.contains(f))
                    .take(MAX_RESULTS)
                    .map(|rel| Row::File { rel: rel.clone(), open: false }),
            );
            return rows;
        }
        fuzzy::rank(name_query, index.files.iter().map(String::as_str), MAX_RESULTS, true)
            .into_iter()
            .map(|i| {
                let rel = index.files[i].clone();
                let open = open.contains(&rel);
                Row::File { rel, open }
            })
            .collect()
    }

    /// Every enabled menu item, as "Menu: Item".
    fn palette_commands(&self) -> Vec<(String, Option<&'static str>, MenuAction)> {
        let mut out = Vec::new();
        for menu in MenuId::BAR {
            for entry in self.menu_entries(menu) {
                if let MenuEntry::Item { label, shortcut, action, enabled: true } = entry {
                    out.push((format!("{}: {}", menu.label(), label.trim_start_matches("✓ ")), shortcut, action));
                }
            }
        }
        out
    }

    /// Enter, or a click on row `clicked`.
    fn accept_palette(&mut self, clicked: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.palette_rows(cx);
        let Some(p) = self.palette.as_ref() else { return };
        let query = p.input.read(cx).text().to_string();
        let Some(row) = rows.into_iter().nth(clicked.unwrap_or(p.selected)) else { return };
        self.palette = None;
        match row {
            Row::Command { action, .. } => {
                self.focus_active_editor(window, cx);
                self.run_menu_action(action, window, cx);
            }
            Row::File { rel, .. } => {
                let Some(root) = self.workspace_root.clone() else { return };
                let path = root.join(&rel);
                // Show it in the tree too (when the tree is visible).
                if let Some(dir) = path.parent() {
                    for ancestor in dir.ancestors().take_while(|a| a.starts_with(&root) && *a != root) {
                        self.expanded_dirs.insert(ancestor.to_path_buf());
                    }
                }
                self.open_file(path, window, cx);
                if let (_, Some(line)) = fuzzy::split_line_suffix(query.trim())
                    && let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i))
                {
                    tab.view.update(cx, |editor, cx| {
                        editor.reveal_line(line.saturating_sub(1));
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_palette(&self, theme: Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let p = self.palette.as_ref()?;
        let rows = self.palette_rows(cx);
        let selected = p.selected.min(rows.len().saturating_sub(1));
        let first = selected.saturating_sub(VISIBLE_ROWS - 1);
        let width = (self.viewport_width - 24.0).clamp(260.0, 620.0);
        let commands = p.input.read(cx).text().starts_with('>');

        let list = rows.iter().enumerate().skip(first).take(VISIBLE_ROWS).map(|(i, row)| {
            let base = div()
                .id(("palette-row", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .overflow_hidden()
                .when(i == selected, |r| r.bg(theme.accent.opacity(0.25)))
                .hover(|s| s.bg(theme.accent.opacity(0.14)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.accept_palette(Some(i), window, cx);
                    }),
                );
            match row {
                Row::File { rel, open } => {
                    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
                    let icon = icon_for(name, false);
                    base.child(div().w(px(14.)).flex_shrink_0().text_size(px(10.)).text_color(icon.color).child(icon.glyph))
                        .child(div().flex_shrink_0().whitespace_nowrap().text_color(theme.foreground).child(name.to_string()))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.5))
                                .text_color(theme.foreground_muted)
                                .child(dir.to_string()),
                        )
                        .when(*open, |r| {
                            r.child(div().flex_1()).child(div().flex_shrink_0().text_size(px(11.)).text_color(theme.accent).child("open"))
                        })
                }
                Row::Command { label, shortcut, .. } => base
                    .justify_between()
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_color(theme.foreground).child(label.clone()))
                    .children(shortcut.map(|s| div().flex_shrink_0().text_size(px(11.)).text_color(theme.foreground_muted).child(s))),
            }
        });

        let empty = rows.is_empty().then(|| {
            let text = if self.workspace_root.is_none() && !commands {
                "Open a folder to search its files"
            } else if self.file_index.is_none() && !commands {
                "Indexing files…"
            } else if commands {
                "No matching commands"
            } else {
                "No matching files"
            };
            div().px_2().py_1().text_color(theme.foreground_muted).child(text)
        });

        Some(
            deferred(
                div()
                    .id("palette-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(48.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_palette(window, cx);
                        }),
                    )
                    .child(
                        div()
                            .id("palette")
                            .w(px(width))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .bg(theme.surface)
                            .border_1()
                            .border_color(theme.accent.opacity(0.5))
                            .rounded_lg()
                            .shadow_lg()
                            .text_size(px(13.))
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(p.input.clone())
                            .children(list)
                            .children(empty)
                            .child(
                                div()
                                    .pt_1()
                                    .text_size(px(11.))
                                    .text_color(theme.foreground_muted)
                                    .child(if commands {
                                        "↑↓ choose · Enter run · Esc close · delete > to search files"
                                    } else {
                                        "↑↓ choose · Enter open · Esc close · type > for commands"
                                    }),
                            ),
                    ),
            )
            .with_priority(5),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::scan_files;

    #[test]
    fn scan_skips_build_output_and_hidden_folders() {
        let root = std::env::temp_dir().join(format!("rji-palette-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for rel in ["src/main/java/App.java", "pom.xml", "target/classes/App.class", ".git/HEAD", "node_modules/x.js"] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        assert_eq!(scan_files(&root), vec!["pom.xml".to_string(), "src/main/java/App.java".to_string()]);
        std::fs::remove_dir_all(&root).ok();
    }
}
