//! File ▸ New Project…: pick a template (console Java, Spring Boot web,
//! JavaFX) and a build tool, name it, and the IDE writes the project and
//! opens it. Library versions are looked up on Maven Central at creation
//! time (latest stable that suits the installed JDK) and fall back to
//! known-good versions offline.

use std::path::PathBuf;

use gpui::{Context, Entity, Focusable, MouseButton, PathPromptOptions, Subscription, Window, deferred, div, prelude::*, px};
use rji_project_java::templates::{
    NewProjectSpec, Template, TemplateBuild, TemplateVersions, create_project, javafx_min_jdk, package_name,
    validate_group, validate_name,
};
use rji_theme::Theme;

use super::RootView;
use crate::text_input::{TextInput, TextInputEvent};

pub(super) struct NewProjectWizard {
    template: Template,
    build: TemplateBuild,
    name: Entity<TextInput>,
    group: Entity<TextInput>,
    location: PathBuf,
    error: Option<String>,
    creating: bool,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub(super) fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme();
        let name = cx.new(|cx| {
            let mut input = TextInput::new("my-app", theme, cx);
            input.set_text(&self.unused_project_name(), cx);
            input
        });
        let group = cx.new(|cx| {
            let mut input = TextInput::new("com.example", theme, cx);
            input.set_text("com.example", cx);
            input
        });
        let on_event = |this: &mut RootView, _: &Entity<TextInput>, event: &TextInputEvent, window: &mut Window, cx: &mut Context<RootView>| {
            match event {
                TextInputEvent::Submit { .. } => this.create_new_project(window, cx),
                TextInputEvent::Cancel => this.close_new_project(window, cx),
                TextInputEvent::Changed => {
                    if let Some(wizard) = this.new_project.as_mut() {
                        wizard.error = None;
                    }
                    cx.notify();
                }
            }
        };
        let subscriptions = vec![cx.subscribe_in(&name, window, on_event), cx.subscribe_in(&group, window, on_event)];
        let focus = name.focus_handle(cx);
        self.new_project = Some(NewProjectWizard {
            template: Template::Console,
            build: TemplateBuild::Maven,
            name,
            group,
            location: self.default_project_parent(),
            error: None,
            creating: false,
            _subscriptions: subscriptions,
        });
        self.open_menu = None;
        self.context_menu = None;
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn close_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_project = None;
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    /// Next to the open project, else the home folder.
    fn default_project_parent(&self) -> PathBuf {
        self.workspace_root
            .as_deref()
            .and_then(|root| root.parent())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn unused_project_name(&self) -> String {
        let parent = self.default_project_parent();
        (1..100)
            .map(|i| if i == 1 { "my-app".to_string() } else { format!("my-app-{i}") })
            .find(|name| !parent.join(name).exists())
            .unwrap_or_else(|| "my-app".into())
    }

    /// Major version of the JDK the IDE detected (what the project will be
    /// compiled for), if any.
    fn detected_jdk_major(&self) -> Option<u32> {
        let raw = super::short_jdk_version(self.jdk_candidates.first()?.version.as_deref());
        let mut parts = raw.split(['.', '-', '+', '_']);
        match parts.next()?.parse::<u32>().ok()? {
            1 => parts.next()?.parse().ok(), // "1.8.0_392" -> 8
            major => Some(major),
        }
    }

    fn pick_project_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Create Here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    if let Some(wizard) = this.new_project.as_mut() {
                        wizard.location = path;
                        wizard.error = None;
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn create_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let jdk_major = self.detected_jdk_major();
        let Some(wizard) = self.new_project.as_mut() else { return };
        if wizard.creating {
            return;
        }
        let name = wizard.name.read(cx).text().trim().to_string();
        let group = wizard.group.read(cx).text().trim().to_string();
        if let Some(problem) = validate_name(&name).or_else(|| validate_group(&group)) {
            wizard.error = Some(problem.into());
            cx.notify();
            return;
        }
        let target = wizard.location.join(&name);
        if target.exists() {
            wizard.error = Some(format!("{} already exists", target.display()));
            cx.notify();
            return;
        }
        let release = jdk_major.unwrap_or(21);
        if wizard.template == Template::SpringBootWeb && release < 17 {
            wizard.error = Some(format!("Spring Boot needs JDK 17 or newer (found JDK {release})"));
            cx.notify();
            return;
        }
        wizard.creating = true;
        let spec = NewProjectSpec {
            template: wizard.template,
            build: wizard.build,
            name,
            group,
            java_release: release,
            versions: TemplateVersions::fallback(release),
        };
        let parent = wizard.location.clone();
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            let spec = NewProjectSpec { versions: latest_template_versions(release), ..spec };
            create_project(&parent, &spec)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(created) => {
                    this.new_project = None;
                    let main_file = created.main_file.clone();
                    this.confirm_unsaved(None, "Open the new project?", window, cx, move |this, window, cx| {
                        this.switch_workspace(created.root.clone(), window, cx);
                        if let Some(root) = this.workspace_root.clone()
                            && let Some(dir) = main_file.parent()
                        {
                            for ancestor in dir.ancestors().take_while(|a| a.starts_with(&root) && *a != root) {
                                this.expanded_dirs.insert(ancestor.to_path_buf());
                            }
                        }
                        if main_file.is_file() {
                            this.open_file(main_file.clone(), window, cx);
                        }
                        this.notify_user("Project created — press ▶ Run to start it");
                    });
                    cx.notify();
                }
                Err(err) => {
                    if let Some(wizard) = this.new_project.as_mut() {
                        wizard.creating = false;
                        wizard.error = Some(format!("{err:#}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_new_project_panel(&self, theme: Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let wizard = self.new_project.as_ref()?;
        let jdk = self.detected_jdk_major();

        let chip = |id: (&'static str, usize), label: &'static str, selected: bool, enabled: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .whitespace_nowrap()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(if selected { theme.accent.opacity(0.18) } else { theme.surface })
                .text_color(if !enabled {
                    theme.foreground_muted.opacity(0.5)
                } else if selected {
                    theme.foreground
                } else {
                    theme.foreground_muted
                })
                .when(enabled && !selected, |c| c.cursor_pointer().hover(|s| s.bg(theme.accent.opacity(0.1))))
                .child(label)
        };
        let section = |label: &'static str| div().text_size(px(12.)).text_color(theme.foreground).child(label);

        let templates = div().flex().flex_row().flex_wrap().gap_2().children(Template::ALL.iter().enumerate().map(|(i, &t)| {
            chip(("np-template", i), t.label(), wizard.template == t, true).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(w) = this.new_project.as_mut() {
                    w.template = t;
                    if t != Template::Console && w.build == TemplateBuild::None {
                        w.build = TemplateBuild::Maven;
                    }
                }
                cx.notify();
            }))
        }));
        let builds = div().flex().flex_row().flex_wrap().gap_2().children(
            [TemplateBuild::Maven, TemplateBuild::Gradle, TemplateBuild::None].into_iter().enumerate().map(|(i, b)| {
                let enabled = b != TemplateBuild::None || wizard.template == Template::Console;
                chip(("np-build", i), b.label(), wizard.build == b, enabled).when(enabled, |c| {
                    c.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(w) = this.new_project.as_mut() {
                            w.build = b;
                        }
                        cx.notify();
                    }))
                })
            }),
        );

        let name = wizard.name.read(cx).text().trim().to_string();
        let group = wizard.group.read(cx).text().trim().to_string();
        let package = if validate_name(&name).is_none() && validate_group(&group).is_none() {
            package_name(&group, &name)
        } else {
            "—".into()
        };
        let target = wizard.location.join(if name.is_empty() { "…" } else { &name });

        let mut notes: Vec<(String, bool)> = Vec::new();
        match jdk {
            Some(major) => notes.push((format!("Compiles for Java {major} (the detected JDK)."), false)),
            None => notes.push(("No JDK detected — the project targets Java 21; install a JDK to build it.".into(), true)),
        }
        if wizard.build == TemplateBuild::Gradle {
            notes.push(("Gradle projects need `gradle` on PATH (run `gradle wrapper` once to add a wrapper).".into(), false));
        }
        if wizard.template == Template::SpringBootWeb {
            notes.push(("Serves http://localhost:8080/ when run.".into(), false));
        }
        if wizard.template == Template::JavaFx && jdk.is_some_and(|j| u64::from(j) < javafx_min_jdk(23)) {
            notes.push(("An older JavaFX release that supports this JDK will be used.".into(), false));
        }

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

        let field = |label: &'static str, input: Entity<TextInput>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w(px(160.))
                .child(section(label))
                .child(input)
        };

        let panel = div()
            .id("new-project-panel")
            .w(px(560.))
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
            .child(div().text_size(px(16.)).text_color(theme.foreground).child("New Project"))
            .child(section("Project type"))
            .child(templates)
            .child(div().text_size(px(12.)).child(wizard.template.description()))
            .child(section("Build tool"))
            .child(builds)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_3()
                    .child(field("Name", wizard.name.clone()))
                    .child(field("Group", wizard.group.clone())),
            )
            .child(div().text_size(px(12.)).child(format!("Package: {package}")))
            .child(section("Location"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .child(target.display().to_string()),
                    )
                    .child(
                        button("np-browse", "Browse…", false)
                            .on_click(cx.listener(|this, _, window, cx| this.pick_project_location(window, cx))),
                    ),
            )
            .children(notes.into_iter().map(|(text, warn)| {
                div()
                    .text_size(px(11.))
                    .text_color(if warn { theme.warning } else { theme.foreground_muted })
                    .child(text)
            }))
            .children(wizard.error.clone().map(|e| div().text_color(theme.error).child(e)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("np-cancel", "Cancel", false)
                            .on_click(cx.listener(|this, _, window, cx| this.close_new_project(window, cx))),
                    )
                    .child(
                        button("np-create", if wizard.creating { "Creating…" } else { "Create" }, true)
                            .on_click(cx.listener(|this, _, window, cx| this.create_new_project(window, cx))),
                    ),
            );

        Some(
            deferred(
                div()
                    .id("new-project-backdrop")
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
                    .child(panel),
            )
            .with_priority(3),
        )
    }
}

/// Latest stable Spring Boot / JavaFX / JUnit releases that work with
/// `jdk_major`, each falling back independently when offline.
fn latest_template_versions(jdk_major: u32) -> TemplateVersions {
    let fallback = TemplateVersions::fallback(jdk_major);
    let latest = |group: &str, artifact: &str, accept: &dyn Fn(&rji_updates::Version) -> bool| {
        let versions = rji_updates::maven_central_versions(group, artifact).ok()?;
        versions
            .iter()
            .filter(|v| rji_updates::is_stable(v))
            .filter_map(|v| rji_updates::Version::parse(v))
            .filter(|v| accept(v))
            .max()
            .map(|v| v.raw)
    };
    let jdk = u64::from(jdk_major);
    TemplateVersions {
        spring_boot: latest("org.springframework.boot", "spring-boot-starter-parent", &|_| true)
            .unwrap_or(fallback.spring_boot),
        javafx: latest("org.openjfx", "javafx-controls", &|v| javafx_min_jdk(v.major()) <= jdk)
            .unwrap_or(fallback.javafx),
        junit: if jdk >= 17 {
            latest("org.junit.jupiter", "junit-jupiter", &|_| true).unwrap_or(fallback.junit)
        } else {
            fallback.junit
        },
    }
}
