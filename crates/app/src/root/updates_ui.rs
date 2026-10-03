//! Toolchain update checks in the UI: background checks, the Updates
//! panel, and applying updates — IntelliJ-style. By default the IDE only
//! *notifies*. Changes happen when the user clicks, or (opt-in) for
//! patch-level releases of versions pinned in the project's build files.
//! JDKs / system Maven are never installed by the IDE: they get a download
//! link. Every file edit keeps a `.rji-backup` and skips files with
//! unsaved changes.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Context, MouseButton, PromptLevel, Window, deferred, div, prelude::*, px};
use rji_theme::Theme;
use rji_updates::{Bump, CheckReport, Installed, UpdateItem, VersionSite, find_sites};

use super::RootView;

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The IDE's own release channel, fixed at build time (`RJI_UPDATE_URL`,
/// `RJI_UPDATE_PUBKEY`). The public key is deliberately *not* a runtime
/// setting: whoever controls it controls what gets installed.
const IDE_UPDATE_URL: Option<&str> = option_env!("RJI_UPDATE_URL");
const IDE_UPDATE_KEY: Option<&str> = option_env!("RJI_UPDATE_PUBKEY");

/// Self-update state of the IDE itself.
#[derive(Clone)]
pub(super) enum IdeUpdate {
    /// This build has no release channel configured.
    NotConfigured,
    Unknown,
    UpToDate,
    Available(rji_updates::ReleaseAsset, String),
    Downloading(String),
    /// Verified and staged; installed on the next start.
    Ready(String),
    Failed(String),
}

impl IdeUpdate {
    pub(super) fn initial() -> Self {
        if IDE_UPDATE_URL.is_some() && IDE_UPDATE_KEY.is_some() {
            IdeUpdate::Unknown
        } else {
            IdeUpdate::NotConfigured
        }
    }
}

type IdeCheck = Option<Result<Option<(rji_updates::ReleaseAsset, String)>, String>>;

/// Fetches + verifies the IDE's release manifest (blocking).
fn check_ide_release() -> IdeCheck {
    let (url, key) = (IDE_UPDATE_URL?, IDE_UPDATE_KEY?);
    Some(
        rji_updates::fetch_manifest(url, key)
            .map(|manifest| {
                rji_updates::applicable_asset(&manifest, env!("CARGO_PKG_VERSION"))
                    .map(|asset| (asset.clone(), manifest.version.clone()))
            })
            .map_err(|err| format!("{err:#}")),
    )
}

impl RootView {
    /// Background check if enabled and the last one is older than the
    /// configured interval. Called at startup and on opening a folder.
    pub(super) fn maybe_check_updates(&mut self, cx: &mut Context<Self>) {
        let settings = &self.app_settings.updates;
        if !settings.check_automatically {
            return;
        }
        let due = now_unix().saturating_sub(settings.last_check_unix) >= settings.check_interval_hours * 3600;
        if due {
            self.check_for_updates(false, cx);
        }
    }

    /// `manual` = the user asked (Help ▸ Check for Updates): the panel opens
    /// with the result even if nothing is new.
    pub(super) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if manual {
            self.show_updates = true;
        }
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        let workspace = self.workspace_root.clone();
        let jdk = self.jdk_candidates.first().and_then(|j| j.version.clone());
        cx.spawn(async move |this, cx| {
            let (report, ide, registry) = cx
                .background_spawn(async move {
                    let maven = rji_updates::detect_system_maven();
                    let report = rji_updates::check(workspace.as_deref(), &Installed { jdk, maven });
                    (report, check_ide_release(), super::plugins_ui::check_registry())
                })
                .await;
            this.update(cx, |this, cx| {
                this.on_ide_check(ide, cx);
                this.on_registry_check(registry, cx);
                this.on_update_report(report, manual, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn on_ide_check(&mut self, ide: IdeCheck, cx: &mut Context<Self>) {
        // Don't clobber a download in progress or one waiting for restart.
        if matches!(self.ide_update, IdeUpdate::Downloading(_) | IdeUpdate::Ready(_)) {
            return;
        }
        self.ide_update = match ide {
            None => IdeUpdate::NotConfigured,
            Some(Ok(None)) => IdeUpdate::UpToDate,
            Some(Ok(Some((asset, version)))) => IdeUpdate::Available(asset, version),
            Some(Err(err)) => IdeUpdate::Failed(err),
        };
        if self.app_settings.updates.auto_download_ide && matches!(self.ide_update, IdeUpdate::Available(..)) {
            self.download_ide_update(cx);
        }
    }

    /// Downloads, verifies, and stages the new IDE build in the background.
    fn download_ide_update(&mut self, cx: &mut Context<Self>) {
        let IdeUpdate::Available(asset, version) = self.ide_update.clone() else {
            return;
        };
        let Ok(exe) = std::env::current_exe() else {
            self.ide_update = IdeUpdate::Failed("can't locate the running executable".into());
            return;
        };
        self.ide_update = IdeUpdate::Downloading(version.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { rji_updates::download_and_stage(&asset, &exe) })
                .await;
            this.update(cx, |this, cx| {
                this.ide_update = match result {
                    Ok(_) => {
                        this.notify_user(format!("RJID {version} is ready — restart to install"));
                        IdeUpdate::Ready(version)
                    }
                    Err(err) => IdeUpdate::Failed(format!("{err:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Restart into the staged update — after the usual unsaved-changes
    /// prompt and child-process shutdown.
    fn restart_to_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_unsaved(None, "Restart to update?", window, cx, |this, _window, cx| {
            this.close_confirmed = true;
            this.shutdown_children(cx);
            cx.restart();
        });
    }

    fn on_update_report(&mut self, mut report: CheckReport, manual: bool, cx: &mut Context<Self>) {
        self.update_checking = false;
        self.app_settings.updates.last_check_unix = now_unix();
        let ignored = self.app_settings.updates.ignored.clone();
        report.items.retain(|item| !ignored.contains(&item.ignore_key()));

        if self.app_settings.updates.auto_apply_safe {
            let mut applied = Vec::new();
            for item in &report.items {
                if let (Some(site), Some(patch)) = (&item.site, &item.safe_patch)
                    && !self.file_has_unsaved_changes(&site.file, cx)
                    && rji_updates::apply_update(site, patch).is_ok()
                {
                    applied.push(format!("{} {} → {patch}", item.component.label(), item.current));
                }
            }
            if !applied.is_empty() {
                self.after_files_updated(cx);
                report = self.refresh_report_sites(report);
                self.notify_user(format!("Auto-updated {} (backups kept as *.rji-backup)", applied.join(", ")));
            }
        }

        if !manual && !report.items.is_empty() {
            let n = report.items.len();
            self.notify_user(format!("{n} update{} available — click ⬆ in the status bar", if n == 1 { "" } else { "s" }));
        }
        self.update_report = Some(report);
        self.save_app_settings();
        cx.notify();
    }

    fn file_has_unsaved_changes(&self, file: &Path, cx: &Context<Self>) -> bool {
        self.tabs.iter().any(|t| t.path == file && t.view.read(cx).dirty)
    }

    /// After build files changed on disk: reload their open tabs and
    /// re-detect the project (versions, DevTools, …).
    fn after_files_updated(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.view.update(cx, |editor, cx| {
                editor.reload_from_disk();
                cx.notify();
            });
        }
        self.refresh_tree(cx);
    }

    /// Re-reads version sites (an edit can shift byte ranges) and drops
    /// items that are now current.
    fn refresh_report_sites(&self, mut report: CheckReport) -> CheckReport {
        let Some(root) = self.workspace_root.as_deref() else {
            return report;
        };
        let sites: Vec<VersionSite> = find_sites(root);
        report.items.retain_mut(|item| {
            let Some(old) = &item.site else {
                return true;
            };
            let Some(site) = sites.iter().find(|s| s.component == old.component && s.file == old.file) else {
                return false;
            };
            item.current = site.current.clone();
            item.site = Some(site.clone());
            let current = rji_updates::Version::parse(&item.current);
            let latest = rji_updates::Version::parse(&item.latest);
            if item.safe_patch.as_ref().is_some_and(|p| p == &item.current) {
                item.safe_patch = None;
            }
            match (current, latest) {
                (Some(c), Some(l)) if l > c => true,
                _ => false,
            }
        });
        report
    }

    /// Applies one update the user clicked. Major upgrades ask first,
    /// since they usually need code changes.
    fn apply_item(&mut self, index: usize, version: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.update_report.as_ref().and_then(|r| r.items.get(index)).cloned() else {
            return;
        };
        let Some(site) = item.site.clone() else {
            return;
        };
        if self.file_has_unsaved_changes(&site.file, cx) {
            let name = site.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.notify_user(format!("Save or discard your changes to {name} first"));
            return cx.notify();
        }
        let is_major = rji_updates::Version::parse(&version)
            .zip(rji_updates::Version::parse(&item.current))
            .is_some_and(|(new, cur)| new.major() != cur.major());
        if is_major {
            if self.prompt_open {
                return;
            }
            self.prompt_open = true;
            let message = format!("Upgrade {} {} → {version}?", item.component.label(), item.current);
            let answer = window.prompt(
                PromptLevel::Warning,
                &message,
                Some("This is a major version upgrade. It often needs code or configuration changes — check the release notes and rebuild afterwards. A backup of the build file is kept."),
                &["Upgrade", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let answer = answer.await.ok();
                this.update(cx, |this, cx| {
                    this.prompt_open = false;
                    if answer == Some(0) {
                        this.write_update(&item, &site, &version, cx);
                    }
                })
                .ok();
            })
            .detach();
        } else {
            self.write_update(&item, &site, &version, cx);
        }
    }

    fn write_update(&mut self, item: &UpdateItem, site: &VersionSite, version: &str, cx: &mut Context<Self>) {
        match rji_updates::apply_update(site, version) {
            Ok(backup) => {
                self.after_files_updated(cx);
                if let Some(report) = self.update_report.take() {
                    self.update_report = Some(self.refresh_report_sites(report));
                }
                let backup = backup.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify_user(format!(
                    "{} updated {} → {version}. Rebuild to verify (backup: {backup})",
                    item.component.label(),
                    item.current
                ));
            }
            Err(err) => self.notify_user(format!("Update failed: {err:#}")),
        }
        cx.notify();
    }

    fn ignore_item(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(report) = self.update_report.as_mut()
            && index < report.items.len()
        {
            let item = report.items.remove(index);
            self.app_settings.updates.ignored.push(item.ignore_key());
            self.save_app_settings();
        }
        cx.notify();
    }

    pub(super) fn update_count(&self) -> usize {
        let ide = matches!(self.ide_update, IdeUpdate::Available(..) | IdeUpdate::Ready(_)) as usize;
        self.update_report.as_ref().map_or(0, |r| r.items.len()) + ide + self.plugin_update_ids().len()
    }

    fn render_ide_row(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let current = env!("CARGO_PKG_VERSION");
        let (status, color) = match &self.ide_update {
            IdeUpdate::NotConfigured => (
                "Self-update isn't configured for this build (no signed release channel).".to_string(),
                theme.foreground_muted,
            ),
            IdeUpdate::Unknown => ("Not checked yet.".to_string(), theme.foreground_muted),
            IdeUpdate::UpToDate => ("✓ Up to date.".to_string(), theme.success),
            IdeUpdate::Available(_, v) => (format!("Version {v} is available."), theme.accent),
            IdeUpdate::Downloading(v) => (format!("Downloading and verifying {v}…"), theme.foreground_muted),
            IdeUpdate::Ready(v) => (format!("{v} is downloaded and verified — installs on restart."), theme.success),
            IdeUpdate::Failed(err) => (format!("Update failed: {err}"), theme.warning),
        };
        let action = match &self.ide_update {
            IdeUpdate::Available(..) => Some(
                div()
                    .id("ide-download")
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.accent)
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent.opacity(0.15)))
                    .child("Download")
                    .on_click(cx.listener(|this, _, _, cx| this.download_ide_update(cx))),
            ),
            IdeUpdate::Ready(_) => Some(
                div()
                    .id("ide-restart")
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.accent)
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.accent.opacity(0.15)))
                    .child("Restart now")
                    .on_click(cx.listener(|this, _, window, cx| this.restart_to_update(window, cx))),
            ),
            _ => None,
        };
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(div().text_color(theme.foreground).child(format!("RJID {current}")))
            .child(div().flex_1().min_w(px(160.)).text_color(color).child(status))
            .children(action)
    }

    pub(super) fn render_updates_panel(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let settings = &self.app_settings.updates;
        let last = match settings.last_check_unix {
            0 => "never".to_string(),
            t => {
                let mins = now_unix().saturating_sub(t) / 60;
                match mins {
                    0 => "just now".into(),
                    m if m < 60 => format!("{m} min ago"),
                    m if m < 60 * 48 => format!("{} h ago", m / 60),
                    m => format!("{} days ago", m / 1440),
                }
            }
        };
        let button = |id: (&'static str, usize), label: String, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(if primary { theme.accent } else { theme.border })
                .text_color(if primary { theme.accent } else { theme.foreground })
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent.opacity(0.15)))
                .whitespace_nowrap()
                .child(label)
        };
        let checkbox = |id: &'static str, on: bool, label: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(if on { theme.accent } else { theme.border })
                        .when(on, |d| d.bg(theme.accent).text_color(theme.background).text_size(px(10.)).child("✓")),
                )
                .child(label)
        };

        let mut body = div().flex().flex_col().gap_2();
        match &self.update_report {
            _ if self.update_checking => {
                body = body.child(div().text_color(theme.foreground_muted).child("Checking for updates…"));
            }
            None => {
                body = body.child(div().text_color(theme.foreground_muted).child("Not checked yet."));
            }
            Some(report) => {
                if report.items.is_empty() {
                    body = body.child(div().text_color(theme.success).child("✓ Everything is up to date."));
                }
                for (i, item) in report.items.iter().enumerate() {
                    let (bump_text, bump_color) = match item.bump {
                        Bump::Patch => ("patch", theme.success),
                        Bump::Minor => ("minor", theme.accent),
                        Bump::Major => ("major", theme.warning),
                    };
                    let where_ = item
                        .site
                        .as_ref()
                        .and_then(|s| s.file.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "installed".to_string());
                    let latest = item.latest.clone();
                    let mut actions = div().flex().flex_row().flex_wrap().gap_1();
                    if item.site.is_some() {
                        actions = actions.child(button(("upd-latest", i), format!("Update to {latest}"), true).on_click(
                            cx.listener(move |this, _, window, cx| this.apply_item(i, latest.clone(), window, cx)),
                        ));
                        if let Some(patch) = item.safe_patch.clone().filter(|p| *p != item.latest) {
                            actions = actions.child(
                                button(("upd-patch", i), format!("{patch} (patch)"), false).on_click(cx.listener(
                                    move |this, _, window, cx| this.apply_item(i, patch.clone(), window, cx),
                                )),
                            );
                        }
                    } else if let Some(url) = item.download_url.clone() {
                        actions = actions.child(
                            button(("upd-download", i), "Download…".to_string(), true)
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        );
                    }
                    actions = actions.child(
                        button(("upd-ignore", i), "Ignore".to_string(), false)
                            .on_click(cx.listener(move |this, _, _, cx| this.ignore_item(i, cx))),
                    );
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_color(theme.foreground).child(item.component.label()))
                                    .child(format!("{} → {}", item.current, item.latest))
                                    .child(div().text_size(px(11.)).text_color(bump_color).child(bump_text))
                                    .child(div().text_size(px(11.)).text_color(theme.foreground_muted).child(where_)),
                            )
                            .child(actions),
                    );
                }
                for err in &report.errors {
                    body = body.child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.warning)
                            .child(format!("Couldn't check {err}")),
                    );
                }
            }
        }

        let auto = settings.check_automatically;
        let safe = settings.auto_apply_safe;
        deferred(
            div()
                .id("updates-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .bg(theme.background.opacity(0.55))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("updates-panel")
                        .w(px(560.))
                        .max_w_full()
                        .max_h(px(560.))
                        .overflow_y_scroll()
                        .mx_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .bg(theme.surface)
                        .border_1()
                        .border_color(theme.border)
                        .rounded_lg()
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(theme.foreground_muted)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .child(div().text_size(px(16.)).text_color(theme.foreground).child("Toolchain updates"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap_1()
                                        .child(button(("upd-check", 0), "Check now".into(), false).on_click(
                                            cx.listener(|this, _, _, cx| this.check_for_updates(true, cx)),
                                        ))
                                        .child(button(("upd-close", 0), "Close".into(), false).on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.show_updates = false;
                                                cx.notify();
                                            },
                                        ))),
                                ),
                        )
                        .child(div().text_size(px(11.)).child(format!(
                            "The IDE, JDK, Maven, Spring Boot and JavaFX — last checked {last}"
                        )))
                        .child(self.render_ide_row(theme, cx))
                        .child(body)
                        .child(div().h(px(1.)).bg(theme.border))
                        .child(checkbox("upd-auto", auto, "Check for updates automatically (daily)").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.app_settings.updates.check_automatically ^= true;
                                this.save_app_settings();
                                cx.notify();
                            }),
                        ))
                        .child(
                            checkbox(
                                "upd-safe",
                                safe,
                                "Automatically apply safe updates (patch releases of versions in this project's build files)",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.app_settings.updates.auto_apply_safe ^= true;
                                this.save_app_settings();
                                cx.notify();
                            })),
                        )
                        .when(!matches!(self.ide_update, IdeUpdate::NotConfigured), |panel| {
                            panel.child(
                                checkbox(
                                    "upd-ide",
                                    self.app_settings.updates.auto_download_ide,
                                    "Download IDE updates automatically (verified, installed on restart)",
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app_settings.updates.auto_download_ide ^= true;
                                    this.save_app_settings();
                                    cx.notify();
                                })),
                            )
                        })
                        .child(div().text_size(px(11.)).child(
                            "Minor and major upgrades always ask first. JDKs and system Maven are never installed \
                             automatically — use Download. Edited build files keep a .rji-backup copy, and files \
                             with unsaved changes are never touched.",
                        )),
                ),
        )
        .with_priority(3)
    }
}
