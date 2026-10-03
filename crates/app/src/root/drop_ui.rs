//! Dragging files and folders from the OS file manager onto the window.
//!
//! - A folder with nothing open yet: opened right away.
//! - A folder while a project is open: asks "This Window / New Window /
//!   Cancel". A new window is a second IDE process for that folder (each
//!   window owns its own language server, terminal and debugger).
//! - Files: opened as tabs (several at once is fine).

use std::path::PathBuf;

use gpui::{Context, ExternalPaths, PromptLevel, Window};

use super::RootView;

impl RootView {
    pub(super) fn on_external_drop(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) = paths.paths().iter().cloned().partition(|p| p.is_dir());
        for file in files.iter().filter(|f| f.is_file()) {
            self.open_file(file.clone(), window, cx);
        }
        let Some(folder) = dirs.into_iter().next() else {
            cx.notify();
            return;
        };
        if self.workspace_root.is_none() {
            self.open_folder(folder, window, cx);
            return;
        }
        if self.workspace_root.as_deref() == Some(folder.as_path()) || self.prompt_open {
            return;
        }
        self.prompt_open = true;
        let name = folder
            .file_name()
            .map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned());
        let answer = window.prompt(
            PromptLevel::Info,
            &format!("Open \"{name}\""),
            Some("Open it in this window (closing the current project), or in a new window?"),
            &["This Window", "New Window", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            this.update_in(cx, |this, window, cx| {
                this.prompt_open = false;
                match answer {
                    Some(0) => this.open_folder(folder, window, cx),
                    Some(1) => this.open_in_new_window(folder),
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts another instance of the IDE on `folder`.
    pub(super) fn open_in_new_window(&mut self, folder: PathBuf) {
        let spawned = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg(&folder)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        });
        match spawned {
            Ok(_) => self.notify_user(format!("Opening {} in a new window…", folder.display())),
            Err(err) => self.notify_user(format!("Couldn't open a new window: {err}")),
        }
    }
}

impl RootView {
    /// File ▸ Open Folder in New Window…
    pub(super) fn pick_folder_for_new_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    this.open_in_new_window(path);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}
