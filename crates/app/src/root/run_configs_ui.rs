//! Run ▸ Run Configurations…: what ▶ Run executes.
//!
//! A run configuration is a named shell command saved in the project's
//! `.rji/settings.json` — e.g. the app with program arguments, a Spring
//! profile, or one particular test class. Without any, ▶ Run uses the task
//! detected for the project type ("automatic"). This panel lists both,
//! picks the active one, and adds/removes configurations without editing
//! JSON by hand.

use gpui::{Context, Entity, Focusable, MouseButton, Subscription, Window, deferred, div, prelude::*, px};
use rji_project_java::{TaskGroup, project_tasks};
use rji_settings::RunConfig;
use rji_theme::Theme;

use super::RootView;
use crate::text_input::{TextInput, TextInputEvent};

/// `active_run_config` value meaning "use the detected task".
pub(super) const AUTOMATIC: &str = "";

pub(super) struct RunConfigsPanel {
    name: Entity<TextInput>,
    command: Entity<TextInput>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub(super) fn open_run_configs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace_root.is_none() {
            return;
        }
        let theme = self.theme();
        let suggested = self.auto_run_config().map(|c| c.command).unwrap_or_default();
        let count = self.workspace_settings().run_configs.len();
        let name = cx.new(|cx| {
            let mut input = TextInput::new("Name, e.g. App with args", theme, cx);
            input.set_text(&format!("Config {}", count + 1), cx);
            input
        });
        let command = cx.new(|cx| {
            let mut input = TextInput::new("Command, e.g. mvn compile exec:java \"-Dexec.args=--port 9000\"", theme, cx);
            input.set_text(&suggested, cx);
            input
        });
        let on_event = |this: &mut RootView, _: &Entity<TextInput>, event: &TextInputEvent, window: &mut Window, cx: &mut Context<RootView>| {
            match event {
                TextInputEvent::Submit { .. } => this.add_run_config_from_panel(cx),
                TextInputEvent::Cancel => this.close_run_configs(window, cx),
                TextInputEvent::Changed => {
                    if let Some(panel) = this.run_configs.as_mut() {
                        panel.error = None;
                    }
                    cx.notify();
                }
            }
        };
        let subscriptions = vec![cx.subscribe_in(&name, window, on_event), cx.subscribe_in(&command, window, on_event)];
        let focus = name.focus_handle(cx);
        self.run_configs = Some(RunConfigsPanel { name, command, error: None, _subscriptions: subscriptions });
        self.open_menu = None;
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn close_run_configs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_configs = None;
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    fn set_active_run_config(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut settings = self.workspace_settings();
        settings.active_run_config = Some(name.to_string());
        self.write_workspace_settings(&settings);
        cx.notify();
    }

    fn delete_run_config(&mut self, name: &str, cx: &mut Context<Self>) {
        let mut settings = self.workspace_settings();
        settings.run_configs.retain(|c| c.name != name);
        if settings.active_run_config.as_deref() == Some(name) {
            settings.active_run_config = None;
        }
        self.write_workspace_settings(&settings);
        cx.notify();
    }

    fn add_run_config_from_panel(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.run_configs.as_ref() else { return };
        let name = panel.name.read(cx).text().trim().to_string();
        let command = panel.command.read(cx).text().trim().to_string();
        let mut settings = self.workspace_settings();
        let error = if name.is_empty() {
            Some("Give the configuration a name".to_string())
        } else if command.is_empty() {
            Some("Enter the command ▶ Run should execute".to_string())
        } else if settings.run_configs.iter().any(|c| c.name == name) {
            Some(format!("There's already a configuration named \"{name}\""))
        } else {
            None
        };
        if let Some(error) = error {
            if let Some(panel) = self.run_configs.as_mut() {
                panel.error = Some(error);
            }
            cx.notify();
            return;
        }
        settings.run_configs.push(RunConfig { name: name.clone(), command });
        settings.active_run_config = Some(name.clone());
        self.write_workspace_settings(&settings);
        let next = format!("Config {}", settings.run_configs.len() + 1);
        if let Some(panel) = self.run_configs.as_ref() {
            panel.name.update(cx, |input, cx| input.set_text(&next, cx));
        }
        self.notify_user(format!("Saved \"{name}\" — ▶ Run now uses it"));
        cx.notify();
    }

    fn prefill_command(&mut self, command: String, cx: &mut Context<Self>) {
        if let Some(panel) = self.run_configs.as_ref() {
            panel.command.update(cx, |input, cx| input.set_text(&command, cx));
        }
        cx.notify();
    }

    pub(super) fn render_run_configs_panel(&self, theme: Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let panel = self.run_configs.as_ref()?;
        let settings = self.workspace_settings();
        let auto = self.auto_run_config();
        let active = self.active_run_config().map(|c| c.name);
        let automatic_active = settings.run_configs.is_empty() || settings.active_run_config.as_deref() == Some(AUTOMATIC);

        let button = |id: (&'static str, usize), label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .whitespace_nowrap()
                .border_color(if primary { theme.accent } else { theme.border })
                .text_color(if primary { theme.accent } else { theme.foreground })
                .hover(|s| s.bg(theme.accent.opacity(0.15)))
                .child(label)
        };
        let row = |id: (&'static str, usize), selected: bool| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(if selected { theme.accent.opacity(0.1) } else { theme.surface })
        };
        let texts = |name: String, command: String| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().text_color(theme.foreground).child(name))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_family(crate::fonts::MONO)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(command),
                )
        };

        let mut list = div().flex().flex_col().gap_1();
        // The automatic entry.
        let auto_label = auto.as_ref().map_or("Automatic — no runnable task detected".to_string(), |c| format!("Automatic: {}", c.name));
        let auto_command = auto.as_ref().map_or(String::new(), |c| c.command.clone());
        list = list.child(
            row(("rc-auto", 0), automatic_active)
                .when(!automatic_active, |r| {
                    r.cursor_pointer().on_click(cx.listener(|this, _, _, cx| this.set_active_run_config(AUTOMATIC, cx)))
                })
                .child(texts(auto_label, auto_command)),
        );
        for (i, config) in settings.run_configs.iter().enumerate() {
            let selected = !automatic_active && active.as_deref() == Some(config.name.as_str());
            let pick = config.name.clone();
            let delete = config.name.clone();
            list = list.child(
                row(("rc-row", i), selected)
                    .when(!selected, |r| r.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.set_active_run_config(&pick, cx))))
                    .child(texts(config.name.clone(), config.command.clone()))
                    .child(button(("rc-delete", i), "Delete", false).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_run_config(&delete, cx);
                    }))),
            );
        }

        // Detected tasks as one-click starting points for a new command.
        let chips = self
            .project
            .as_ref()
            .map(|p| project_tasks(p).into_iter().filter(|t| matches!(t.group, TaskGroup::Run | TaskGroup::Test)).collect::<Vec<_>>())
            .unwrap_or_default();

        let field = |label: &'static str, input: Entity<TextInput>| {
            div().flex().flex_col().gap_1().child(div().text_size(px(12.)).text_color(theme.foreground).child(label)).child(input)
        };

        let body = div()
            .id("run-configs-panel")
            .w(px(600.))
            .max_w_full()
            .max_h(px(640.))
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
                    .child(div().text_size(px(16.)).text_color(theme.foreground).child("Run Configurations"))
                    .child(button(("rc-close", 0), "Close", false).on_click(cx.listener(|this, _, window, cx| this.close_run_configs(window, cx)))),
            )
            .child(div().text_size(px(12.)).child(
                "A run configuration is a saved command that ▶ Run (and Debug) executes — for example your app with program \
                 arguments, a Spring profile, or a single test class. \"Automatic\" uses the task detected for this project. \
                 Click one to make it active.",
            ))
            .child(list)
            .child(div().h(px(1.)).bg(theme.border))
            .child(div().text_size(px(14.)).text_color(theme.foreground).child("New configuration"))
            .child(field("Name", panel.name.clone()))
            .child(field("Command (runs in the terminal, from the project folder)", panel.command.clone()))
            .when(!chips.is_empty(), |d| {
                d.child(div().text_size(px(11.)).child("Start from a detected task:")).child(
                    div().flex().flex_row().flex_wrap().gap_1().children(chips.into_iter().enumerate().map(|(i, task)| {
                        let command = task.command.clone();
                        div()
                            .id(("rc-chip", i))
                            .px_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .text_size(px(12.))
                            .hover(|s| s.bg(theme.accent.opacity(0.12)))
                            .child(task.label)
                            .on_click(cx.listener(move |this, _, _, cx| this.prefill_command(command.clone(), cx)))
                    })),
                )
            })
            .children(panel.error.clone().map(|e| div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .id("rc-json")
                            .text_size(px(11.))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("Edit .rji/settings.json")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(root) = this.workspace_root.clone() {
                                    let path = root.join(".rji").join("settings.json");
                                    let settings = this.workspace_settings();
                                    this.write_workspace_settings(&settings);
                                    this.run_configs = None;
                                    this.open_file(path, window, cx);
                                }
                            })),
                    )
                    .child(button(("rc-add", 0), "Save configuration", true).on_click(cx.listener(|this, _, _, cx| this.add_run_config_from_panel(cx)))),
            );

        Some(
            deferred(
                div()
                    .id("run-configs-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .bg(theme.background.opacity(0.55))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            )
            .with_priority(3),
        )
    }
}
