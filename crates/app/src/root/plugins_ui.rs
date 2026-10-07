//! Plugins in the UI: loading them, running their commands on the active
//! editor (off the UI thread), the Plugins manager panel, installing from
//! a folder, and installing / updating from the signed registry.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, MouseButton, PathPromptOptions, Window, deferred, div, prelude::*, px};
use rji_plugins::{CommandInput, CommandOutput, Plugin, RegistryIndex};
use rji_theme::Theme;

use super::RootView;

/// The plugin registry, fixed at build time like the IDE's own update
/// channel. Its key falls back to the IDE release key.
const REGISTRY_URL: Option<&str> = option_env!("RJI_PLUGIN_REGISTRY_URL");
const REGISTRY_KEY: Option<&str> = match option_env!("RJI_PLUGIN_REGISTRY_PUBKEY") {
    Some(key) => Some(key),
    None => option_env!("RJI_UPDATE_PUBKEY"),
};

pub(super) fn plugins_dir() -> PathBuf {
    rji_settings::app_dir().join(".rji-settings").join("plugins")
}

pub(super) struct PluginState {
    pub plugins: Vec<Arc<Plugin>>,
    pub errors: Vec<String>,
    pub registry: Option<Result<RegistryIndex, String>>,
    pub running: bool,
}

impl PluginState {
    pub fn load() -> Self {
        let (plugins, errors) = rji_plugins::discover(&plugins_dir());
        PluginState {
            plugins: plugins.into_iter().map(Arc::new).collect(),
            errors,
            registry: None,
            running: false,
        }
    }
}

/// Registry check result, computed off the UI thread.
pub(super) fn check_registry() -> Option<Result<RegistryIndex, String>> {
    let (url, key) = (REGISTRY_URL?, REGISTRY_KEY?);
    Some(rji_plugins::fetch_index(url, key).map_err(|e| format!("{e:#}")))
}

impl RootView {
    pub(super) fn reload_plugins(&mut self, cx: &mut Context<Self>) {
        let registry = self.plugin_state.registry.take();
        self.plugin_state = PluginState::load();
        self.plugin_state.registry = registry;
        cx.notify();
    }

    /// Runs a command on the active editor's selection (or whole file) in
    /// a background thread; the result is applied as one undoable edit if
    /// the file hasn't changed meanwhile.
    pub(super) fn run_plugin_command(&mut self, plugin_index: usize, command: String, cx: &mut Context<Self>) {
        let Some(plugin) = self.plugin_state.plugins.get(plugin_index).cloned() else {
            return;
        };
        let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) else {
            self.notify_user("Open a file to run a plugin command on");
            return cx.notify();
        };
        if self.plugin_state.running {
            return;
        }
        let view = tab.view.clone();
        let (text, is_selection, range, version) = view.read(cx).plugin_target();
        let editor = view.read(cx);
        let input = CommandInput {
            command,
            text,
            is_selection,
            file_name: editor.file_name(),
            language: editor.language().to_string(),
        };
        self.plugin_state.running = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { plugin.run(&input) }).await;
            this.update(cx, |this, cx| {
                this.plugin_state.running = false;
                match result {
                    Ok(CommandOutput::Replace(text)) => {
                        let applied = view.update(cx, |editor, cx| {
                            let ok = editor.apply_plugin_replace(range, &text, version);
                            cx.notify();
                            ok
                        });
                        if !applied {
                            this.notify_user("The file changed while the plugin ran — result discarded");
                        }
                    }
                    Ok(CommandOutput::Message(message)) => this.notify_user(message),
                    Err(err) => this.notify_user(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn install_plugin_from_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Install plugin".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(source) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    match rji_plugins::install_from_folder(&source, &plugins_dir()) {
                        Ok(dir) => {
                            this.reload_plugins(cx);
                            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            this.notify_user(format!("Installed plugin {name}"));
                        }
                        Err(err) => this.notify_user(format!("Couldn't install plugin: {err:#}")),
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn uninstall_plugin(&mut self, id: String, cx: &mut Context<Self>) {
        match rji_plugins::uninstall(&id, &plugins_dir()) {
            Ok(()) => {
                self.reload_plugins(cx);
                self.notify_user(format!("Uninstalled plugin {id}"));
            }
            Err(err) => self.notify_user(format!("Couldn't uninstall {id}: {err:#}")),
        }
        cx.notify();
    }

    /// Installs (or updates) registry entries in the background.
    fn install_from_registry(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let Some(Ok(index)) = &self.plugin_state.registry else {
            return;
        };
        let entries: Vec<_> = index.plugins.iter().filter(|e| ids.contains(&e.id)).cloned().collect();
        if entries.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    entries
                        .iter()
                        .map(|e| (e.name.clone(), e.version.clone(), rji_plugins::install(e, &plugins_dir())))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                let mut done = Vec::new();
                for (name, version, result) in results {
                    match result {
                        Ok(_) => done.push(format!("{name} {version}")),
                        Err(err) => this.notify_user(format!("Plugin {name}: {err:#}")),
                    }
                }
                this.reload_plugins(cx);
                if !done.is_empty() {
                    this.notify_user(format!("Installed {}", done.join(", ")));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Called with each update check's registry result.
    pub(super) fn on_registry_check(&mut self, registry: Option<Result<RegistryIndex, String>>, cx: &mut Context<Self>) {
        self.plugin_state.registry = registry;
        if self.app_settings.updates.auto_update_plugins {
            let updates = self.plugin_update_ids();
            if !updates.is_empty() {
                self.install_from_registry(updates, cx);
            }
        }
    }

    pub(super) fn plugin_update_ids(&self) -> Vec<String> {
        let Some(Ok(index)) = &self.plugin_state.registry else {
            return Vec::new();
        };
        let installed: Vec<&Plugin> = self.plugin_state.plugins.iter().map(|p| p.as_ref()).collect();
        rji_plugins::updates_available(index, &installed)
            .into_iter()
            .map(|e| e.id.clone())
            .collect()
    }

    pub(super) fn render_plugins_panel(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
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
        let card = || {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
        };

        let has_editor = self.active_tab.is_some();
        let mut installed = div().flex().flex_col().gap_2();
        if self.plugin_state.plugins.is_empty() {
            installed = installed.child(
                div().child("No plugins installed. Use “Install from Folder…” (try plugins/java-tools in the source tree)."),
            );
        }
        for (i, plugin) in self.plugin_state.plugins.iter().enumerate() {
            let m = &plugin.manifest;
            let id = m.id.clone();
            installed = installed.child(
                card()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().text_color(theme.foreground).child(m.name.clone()))
                            .child(div().text_size(px(11.)).child(m.version.clone()))
                            .child(div().flex_1())
                            .child(
                                button(("plugin-uninstall", i), "Uninstall".into(), false)
                                    .on_click(cx.listener(move |this, _, _, cx| this.uninstall_plugin(id.clone(), cx))),
                            ),
                    )
                    .when(!m.description.is_empty(), |c| c.child(m.description.clone()))
                    // Commands run on the active editor's selection (or the
                    // whole file).
                    .child(
                        div().flex().flex_row().flex_wrap().gap_1().children(m.commands.iter().enumerate().map(|(j, command)| {
                            let command_id = command.id.clone();
                            let enabled = has_editor && !self.plugin_state.running;
                            button(("plugin-run", i * 100 + j), format!("▶ {}", command.title), false)
                                .when(enabled, |b| {
                                    b.on_click(cx.listener(move |this, _, _, cx| {
                                        this.show_plugins = false;
                                        this.run_plugin_command(i, command_id.clone(), cx);
                                    }))
                                })
                                .when(!enabled, |b| b.opacity(0.5))
                        })),
                    )
                    .when(!has_editor, |c| {
                        c.child(div().text_size(px(11.)).child("Open a file to run these on its selection (or the whole file)."))
                    }),
            );
        }
        for err in &self.plugin_state.errors {
            installed = installed.child(div().text_size(px(11.)).text_color(theme.warning).child(format!("Not loaded: {err}")));
        }

        let registry = match &self.plugin_state.registry {
            _ if REGISTRY_URL.is_none() || REGISTRY_KEY.is_none() => div()
                .text_size(px(11.))
                .child("No plugin registry is configured for this build — install plugins from a folder.")
                .into_any_element(),
            None => div().text_size(px(11.)).child("Registry not checked yet (Help ▸ Check for Updates).").into_any_element(),
            Some(Err(err)) => div()
                .text_size(px(11.))
                .text_color(theme.warning)
                .child(format!("Couldn't load the registry: {err}"))
                .into_any_element(),
            Some(Ok(index)) => {
                let mut list = div().flex().flex_col().gap_2();
                for (i, entry) in index.plugins.iter().enumerate() {
                    let installed_version = self
                        .plugin_state
                        .plugins
                        .iter()
                        .find(|p| p.manifest.id == entry.id)
                        .map(|p| p.manifest.version.clone());
                    let newer = installed_version.as_ref().is_none_or(|v| {
                        rji_updates::Version::parse(&entry.version) > rji_updates::Version::parse(v)
                    });
                    let label = match &installed_version {
                        None => format!("Install {}", entry.version),
                        Some(_) if newer => format!("Update to {}", entry.version),
                        Some(_) => "Installed".to_string(),
                    };
                    let id = entry.id.clone();
                    list = list.child(
                        card()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_color(theme.foreground).child(entry.name.clone()))
                                    .child(div().text_size(px(11.)).child(entry.version.clone()))
                                    .child(div().flex_1())
                                    .child(if newer {
                                        button(("registry-install", i), label, true)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.install_from_registry(vec![id.clone()], cx)
                                            }))
                                            .into_any_element()
                                    } else {
                                        div().text_size(px(11.)).child(label).into_any_element()
                                    }),
                            )
                            .when(!entry.description.is_empty(), |c| c.child(entry.description.clone())),
                    );
                }
                list.into_any_element()
            }
        };

        let auto = self.app_settings.updates.auto_update_plugins;
        deferred(
            div()
                .id("plugins-backdrop")
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
                        .id("plugins-panel")
                        .w(px(560.))
                        .max_w_full()
                        .max_h(px(580.))
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
                                .child(div().text_size(px(16.)).text_color(theme.foreground).child("Plugins"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap_1()
                                        .child(button(("plugins-folder", 0), "Install from Folder…".into(), false).on_click(
                                            cx.listener(|this, _, window, cx| this.install_plugin_from_folder(window, cx)),
                                        ))
                                        .child(button(("plugins-reload", 0), "Reload".into(), false)
                                            .on_click(cx.listener(|this, _, _, cx| this.reload_plugins(cx))))
                                        .child(button(("plugins-close", 0), "Close".into(), false).on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.show_plugins = false;
                                                cx.notify();
                                            },
                                        ))),
                                ),
                        )
                        .child(div().text_size(px(11.)).child(
                            "Plugins run sandboxed: they can't access files, the network, or other programs — \
                             only the text you run them on. Runaway plugins are stopped automatically.",
                        ))
                        .child(div().text_color(theme.foreground).child("Installed"))
                        .child(installed)
                        .child(div().h(px(1.)).bg(theme.border))
                        .child(div().text_color(theme.foreground).child("Registry"))
                        .child(registry)
                        .child(
                            div()
                                .id("plugins-auto")
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
                                        .border_color(if auto { theme.accent } else { theme.border })
                                        .when(auto, |d| d.bg(theme.accent).text_color(theme.background).text_size(px(10.)).child("✓")),
                                )
                                .child("Update plugins automatically (signed and verified)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.app_settings.updates.auto_update_plugins ^= true;
                                    this.save_app_settings();
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .with_priority(3)
    }
}
