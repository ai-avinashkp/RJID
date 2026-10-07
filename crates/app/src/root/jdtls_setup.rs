//! First-run setup for Java intelligence: when no language server is found
//! (no `JDTLS_HOME`, no copy of our own, no VS Code Java extension), the
//! IDE offers to download the Eclipse JDT Language Server — only after the
//! user agrees — and starts it once installed. Download, checksum and safe
//! unpacking live in `rji_updates::jdtls`.

use std::path::PathBuf;

use gpui::{Context, PromptLevel, Window};
use rji_project_java::JdkCandidate;

use super::{LspStatus, RootView, short_jdk_version};

/// jdtls (1.36 and later) only runs on Java 21+.
pub(super) const JDTLS_MIN_JAVA: u32 = 21;

/// `<app_dir>/.rji-settings/jdtls`: where the IDE keeps its own copy.
pub(super) fn managed_jdtls_root() -> PathBuf {
    rji_settings::app_dir().join(".rji-settings").join("jdtls")
}

/// Major version of a detected JDK ("1.8.0_392" -> 8, "25.0.2" -> 25).
pub(super) fn jdk_major(jdk: &JdkCandidate) -> Option<u32> {
    let raw = short_jdk_version(jdk.version.as_deref());
    let mut parts = raw.split(['.', '-', '+', '_']);
    match parts.next()?.parse::<u32>().ok()? {
        1 => parts.next()?.parse().ok(),
        major => Some(major),
    }
}

/// Download state shown in the status bar.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum JdtlsDownload {
    Checking,
    Downloading { done: u64, total: Option<u64> },
    Installing,
}

impl RootView {
    /// The JDK to run the language server with: the first one new enough.
    pub(super) fn jdk_for_jdtls(&self) -> Option<&JdkCandidate> {
        self.jdk_candidates.iter().find(|jdk| jdk_major(jdk).is_some_and(|m| m >= JDTLS_MIN_JAVA))
    }

    /// Status-bar text while downloading.
    pub(super) fn jdtls_download_text(&self) -> Option<String> {
        Some(match self.jdtls_download? {
            JdtlsDownload::Checking => "Finding the Java language server…".to_string(),
            JdtlsDownload::Downloading { done, total: Some(total) } if total > 0 => {
                format!("Downloading Java language server {}% ({} / {} MB)", done * 100 / total, done >> 20, total >> 20)
            }
            JdtlsDownload::Downloading { done, .. } => format!("Downloading Java language server ({} MB)", done >> 20),
            JdtlsDownload::Installing => "Installing Java language server…".to_string(),
        })
    }

    /// Asks before downloading (≈50 MB from download.eclipse.org).
    pub(super) fn offer_jdtls_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_open || self.jdtls_download.is_some() {
            return;
        }
        if self.jdk_for_jdtls().is_none() {
            self.notify_user(format!(
                "Java code intelligence needs a JDK {JDTLS_MIN_JAVA} or newer to run its language server. Install one (e.g. Eclipse Temurin), then restart RJID."
            ));
            cx.notify();
            return;
        }
        self.prompt_open = true;
        let answer = window.prompt(
            PromptLevel::Info,
            "Download the Java language server?",
            Some(
                "Completion, errors, quick fixes and refactorings come from the Eclipse JDT Language Server. \
                 None was found on this computer.\n\nRJID can download it now (about 50 MB) from download.eclipse.org, \
                 check its checksum and keep it in RJID's own folder. It's licensed under the Eclipse Public License 2.0.",
            ),
            &["Download", "Not Now"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            this.update(cx, |this, cx| {
                this.prompt_open = false;
                if answer == Some(0) {
                    this.download_jdtls(cx);
                } else {
                    this.jdtls_offer_declined = true;
                    this.notify_user("You can install it later from Help ▸ Install Java Language Server…");
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn download_jdtls(&mut self, cx: &mut Context<Self>) {
        if self.jdtls_download.is_some() {
            return;
        }
        self.jdtls_download = Some(JdtlsDownload::Checking);
        self.ensure_spinner(cx);
        let (tx, rx) = async_channel::unbounded::<JdtlsDownload>();
        let root = managed_jdtls_root();
        let task = cx.background_executor().spawn(async move {
            let release = rji_updates::jdtls::latest_milestone()?;
            let last = std::cell::Cell::new(u64::MAX);
            let tx_progress = tx.clone();
            let installed = rji_updates::jdtls::install(&release, &root, &rji_lsp_client::is_server_dir, &|done, total| {
                // ~1 MB steps are plenty for the status bar.
                if done >> 20 != last.get() {
                    last.set(done >> 20);
                    let _ = tx_progress.try_send(JdtlsDownload::Downloading { done, total });
                }
            });
            let _ = tx.try_send(JdtlsDownload::Installing);
            installed.map(|dir| (release.version, dir))
        });
        cx.spawn(async move |this, cx| {
            while let Ok(state) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.jdtls_download.is_some() {
                            this.jdtls_download = Some(state);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.jdtls_download = None;
                match result {
                    Ok((version, _dir)) => {
                        this.notify_user(format!("✓ Java language server {version} installed — starting it"));
                        this.lsp_status = LspStatus::Idle;
                        this.spawn_lsp_if_needed(cx);
                    }
                    Err(err) => {
                        this.notify_user(format!("Couldn't install the Java language server: {err:#}"));
                        this.lsp_status = LspStatus::NotInstalled;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
