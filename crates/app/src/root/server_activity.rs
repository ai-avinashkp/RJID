//! What the Java language server is doing, for the status bar: importing
//! the project, resolving/downloading dependencies after `pom.xml` or
//! `build.gradle` changes, indexing. Also what happens when a build file is
//! saved (re-detect the project, tell jdtls to re-import).

use std::path::Path;

use gpui::Context;
use rji_lsp_client::ServerStatus;

use super::RootView;

/// Build files whose changes alter dependencies or project structure.
const BUILD_FILES: &[&str] = &["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"];

/// jdtls jobs that run constantly (every keystroke, every save) and would
/// only make the status bar flicker.
const QUIET_TASKS: &[&str] = &["Building", "Register Watchers", "Detect installed JVMs", "LookupJDKToolchainsJob", "Reporting encoding changes"];

/// Fast re-imports still show their indicator this long.
const MIN_ACTIVITY_DISPLAY: std::time::Duration = std::time::Duration::from_millis(1500);
/// Give up waiting for jdtls's verdict after this.
const DEPENDENCY_UPDATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Status-bar spinner frames.
pub(super) const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// One task jdtls reported with `$/progress`.
pub(super) struct ServerTask {
    token: String,
    title: String,
    text: String,
}

/// "Synchronizing projects - 0% " -> "Synchronizing projects";
/// "Importing Maven project(s) - Project 'cli'" stays as is.
fn tidy(message: &str, percentage: Option<u32>) -> String {
    let mut text = message.trim().trim_end_matches(['-', ' ']).to_string();
    for noise in [" - 0%", " -"] {
        if let Some(stripped) = text.strip_suffix(noise) {
            text = stripped.trim().to_string();
        }
    }
    match percentage {
        Some(p) if p > 0 && !text.contains('%') => format!("{text} {p}%"),
        _ => text,
    }
}

impl RootView {
    /// Auto-save (if enabled): writes `view` if it has unsaved edits and its
    /// file still exists (a deleted file isn't silently recreated).
    pub(super) fn autosave(&mut self, view: &gpui::Entity<crate::editor_view::CodeEditorView>, cx: &mut Context<Self>) {
        if !self.app_settings.auto_save {
            return;
        }
        let (dirty, exists) = {
            let editor = view.read(cx);
            (editor.dirty, editor.path.is_file())
        };
        if dirty && exists {
            view.update(cx, |editor, cx| {
                editor.save(cx);
                cx.notify();
            });
        }
    }

    /// Auto-save every edited file (the window lost focus).
    pub(super) fn autosave_all(&mut self, cx: &mut Context<Self>) {
        let views: Vec<_> = self.tabs.iter().map(|t| t.view.clone()).collect();
        for view in views {
            self.autosave(&view, cx);
        }
    }

    /// Hooks that need the window: auto-save when the user switches to
    /// another app.
    pub fn install_window_hooks(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let subscription = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.autosave_all(cx);
            }
        });
        self.window_subscriptions.push(subscription);
    }

    /// Applies one progress/status report from jdtls.
    pub(super) fn on_server_status(&mut self, status: ServerStatus, cx: &mut Context<Self>) {
        match status {
            ServerStatus::Progress { token, title, message, percentage, done } => {
                if done {
                    self.server_tasks.retain(|t| t.token != token);
                } else {
                    let index = match self.server_tasks.iter().position(|t| t.token == token) {
                        Some(index) => index,
                        None => {
                            self.server_tasks.push(ServerTask { token, title: String::new(), text: String::new() });
                            self.server_tasks.len() - 1
                        }
                    };
                    let task = &mut self.server_tasks[index];
                    if let Some(title) = title {
                        task.title = title;
                    }
                    let message = message.unwrap_or_else(|| task.title.clone());
                    task.text = tidy(&message, percentage);
                }
            }
            // A one-off message ("Updating project configurations...").
            ServerStatus::Status { kind, message } if kind == "Message" && !message.is_empty() => {
                self.server_message = Some(message.trim_end_matches('.').to_string());
            }
            // The verdict after a re-import: "OK", or a warning/error summary.
            ServerStatus::Status { kind, message } if kind == "ProjectStatus" => {
                self.server_message = None;
                if self.dependency_update.is_some() {
                    self.finish_dependency_update(message, cx);
                }
            }
            ServerStatus::Status { kind, .. } if kind == "ServiceReady" => {
                self.server_message = None;
            }
            ServerStatus::Status { .. } => {}
        }
        if self.server_tasks.is_empty() && self.dependency_update.is_none() {
            self.server_message = None;
        }
        self.ensure_spinner(cx);
        cx.notify();
    }

    /// jdtls reported the project status after a build-file save: keep the
    /// indicator up for a moment (fast re-imports would only flash), then
    /// report the result.
    fn finish_dependency_update(&mut self, status: String, cx: &mut Context<Self>) {
        let Some((started, generation)) = self.dependency_update else { return };
        let remaining = MIN_ACTIVITY_DISPLAY.saturating_sub(started.elapsed());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            this.update(cx, |this, cx| {
                if this.dependency_update.map(|(_, g)| g) != Some(generation) {
                    return;
                }
                this.dependency_update = None;
                this.server_message = None;
                if status.eq_ignore_ascii_case("OK") {
                    this.notify_user("✓ Project dependencies updated");
                } else {
                    this.notify_user("⚠ The build file has problems — hover the red underline in it for details and quick fixes");
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Animates the status-bar spinner while the server is busy; the loop
    /// ends by itself once there's nothing to show.
    pub(super) fn ensure_spinner(&mut self, cx: &mut Context<Self>) {
        if self.spinner_running || self.server_activity().is_none() {
            return;
        }
        self.spinner_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(140)).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        if this.server_activity().is_none() {
                            this.spinner_running = false;
                            cx.notify();
                            return false;
                        }
                        this.spinner_frame = this.spinner_frame.wrapping_add(1);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// The status-bar text while the server is busy, if it is.
    pub(super) fn server_activity(&self) -> Option<String> {
        if let Some(text) = self.jdtls_download_text() {
            return Some(text);
        }
        self.server_tasks
            .iter()
            .rev()
            .find(|t| !QUIET_TASKS.iter().any(|q| t.title.starts_with(q)))
            .map(|t| t.text.clone())
            .filter(|text| !text.is_empty())
            .or_else(|| self.server_message.clone())
            .or_else(|| self.dependency_update.map(|_| "Resolving project dependencies".to_string()))
    }

    /// A file was written to disk. For build files: re-detect the project
    /// type and have jdtls re-import it (resolving new dependencies).
    pub(super) fn on_file_saved(&mut self, path: &Path, cx: &mut Context<Self>) {
        let is_build_file = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| BUILD_FILES.contains(&name));
        let in_workspace = self.workspace_root.as_deref().is_some_and(|root| path.starts_with(root));
        if !is_build_file || !in_workspace {
            return;
        }
        self.refresh_tree(cx);
        *self.devtools.borrow_mut() =
            super::devtools_hook_for(self.workspace_root.as_deref(), &self.maven_project, &self.gradle_project);
        let notified = self
            .lsp
            .borrow_mut()
            .as_mut()
            .is_some_and(|client| client.project_configuration_update(path).is_ok());
        if notified {
            // Shown until jdtls reports the project status (or gives up).
            self.dependency_update_generation += 1;
            let generation = self.dependency_update_generation;
            self.dependency_update = Some((std::time::Instant::now(), generation));
            self.server_message = Some("Resolving project dependencies".into());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DEPENDENCY_UPDATE_TIMEOUT).await;
                this.update(cx, |this, cx| {
                    if this.dependency_update.map(|(_, g)| g) == Some(generation) {
                        this.dependency_update = None;
                        this.server_message = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            self.ensure_spinner(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::tidy;

    #[test]
    fn tidies_jdtls_progress_text() {
        assert_eq!(tidy("Synchronizing projects - 0% ", Some(0)), "Synchronizing projects");
        assert_eq!(tidy("Importing Maven project(s) - Project 'cli'", Some(49)), "Importing Maven project(s) - Project 'cli' 49%");
        assert_eq!(tidy("Setting classpath containers - 51% Project 'cli'", Some(50)), "Setting classpath containers - 51% Project 'cli'");
        assert_eq!(tidy("Updating project configurations", None), "Updating project configurations");
    }
}
