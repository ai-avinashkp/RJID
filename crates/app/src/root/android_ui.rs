//! Android devices in the UI: SDK detection, the Devices panel (USB
//! phones and emulators), launching emulators, and build / install / run /
//! logcat — the latter run in the terminal so their output is visible.

use gpui::{Context, MouseButton, PathPromptOptions, Window, deferred, div, prelude::*, px};
use rji_android::{AndroidProject, Device, Sdk};
use rji_theme::Theme;

use super::RootView;

#[derive(Default)]
pub(super) struct AndroidState {
    /// `None` until first looked up; `Some(None)` = no SDK found.
    pub sdk: Option<Option<Sdk>>,
    pub devices: Vec<Device>,
    pub avds: Vec<String>,
    pub selected: Option<String>,
    pub project: Option<AndroidProject>,
    pub refreshing: bool,
    pub error: Option<String>,
}

impl RootView {
    pub(super) fn open_android_panel(&mut self, cx: &mut Context<Self>) {
        self.show_android = true;
        self.refresh_android(cx);
    }

    /// Detects the SDK (first time) and lists devices + emulators, off the
    /// UI thread (adb can take a moment to start its server).
    pub(super) fn refresh_android(&mut self, cx: &mut Context<Self>) {
        if self.android.refreshing {
            return;
        }
        self.android.refreshing = true;
        self.android.project = self.workspace_root.as_deref().and_then(rji_android::detect_android_project);
        let known_sdk = self.android.sdk.clone();
        cx.spawn(async move |this, cx| {
            let (sdk, devices, avds) = cx
                .background_spawn(async move {
                    let sdk = known_sdk.flatten().or_else(rji_android::detect_sdk);
                    let devices = sdk.as_ref().map(rji_android::list_devices);
                    let avds = sdk.as_ref().map(rji_android::list_avds);
                    (sdk, devices, avds)
                })
                .await;
            this.update(cx, |this, cx| {
                let state = &mut this.android;
                state.refreshing = false;
                state.error = None;
                match devices {
                    Some(Ok(devices)) => state.devices = devices,
                    Some(Err(err)) => state.error = Some(format!("{err:#}")),
                    None => state.devices.clear(),
                }
                state.avds = avds.and_then(Result::ok).unwrap_or_default();
                state.sdk = Some(sdk);
                // Keep the selection if that device is still there; else pick
                // the first ready one.
                if !state.selected.as_ref().is_some_and(|s| state.devices.iter().any(|d| &d.serial == s)) {
                    state.selected = state.devices.iter().find(|d| d.is_ready()).map(|d| d.serial.clone());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn android_sdk(&self) -> Option<Sdk> {
        self.android.sdk.clone().flatten()
    }

    fn selected_device(&self) -> Option<&Device> {
        let serial = self.android.selected.as_ref()?;
        self.android.devices.iter().find(|d| &d.serial == serial && d.is_ready())
    }

    fn launch_avd(&mut self, avd: String, cx: &mut Context<Self>) {
        let Some(sdk) = self.android_sdk() else {
            return;
        };
        match rji_android::launch_emulator(&sdk, &avd) {
            Ok(()) => self.notify_user(format!("Starting emulator {avd}… press Refresh once it has booted")),
            Err(err) => self.notify_user(format!("Couldn't start {avd}: {err:#}")),
        }
        cx.notify();
    }

    fn android_build_install_run(&mut self, cx: &mut Context<Self>) {
        let (Some(sdk), Some(device), Some(project)) =
            (self.android_sdk(), self.selected_device().cloned(), self.android.project.clone())
        else {
            return;
        };
        let gradle = match self.workspace_root.as_deref() {
            Some(root) if root.join(if cfg!(windows) { "gradlew.bat" } else { "gradlew" }).exists() => {
                if cfg!(windows) { ".\\gradlew.bat" } else { "./gradlew" }.to_string()
            }
            _ => "gradle".to_string(),
        };
        let command = rji_android::build_install_run_command(&sdk, &project, &device.serial, &gradle);
        self.show_android = false;
        self.run_in_terminal(&command, cx);
    }

    fn android_logcat(&mut self, cx: &mut Context<Self>) {
        let (Some(sdk), Some(device)) = (self.android_sdk(), self.selected_device().cloned()) else {
            return;
        };
        self.show_android = false;
        self.run_in_terminal(&rji_android::logcat_command(&sdk, &device.serial), cx);
        self.notify_user("Logcat is streaming in the terminal — Ctrl+C there to stop");
    }

    fn android_install_apk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(sdk), Some(device)) = (self.android_sdk(), self.selected_device().cloned()) else {
            return;
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Install APK".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(apk) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    if apk.extension().and_then(|e| e.to_str()) != Some("apk") {
                        this.notify_user("Choose an .apk file");
                    } else {
                        this.show_android = false;
                        this.run_in_terminal(&rji_android::install_apk_command(&sdk, &device.serial, &apk), cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn render_android_panel(&self, theme: Theme, cx: &Context<Self>) -> impl IntoElement {
        let state = &self.android;
        let button = |id: (&'static str, usize), label: String, primary: bool, enabled: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(if primary && enabled { theme.accent } else { theme.border })
                .text_color(if !enabled {
                    theme.foreground_muted.opacity(0.6)
                } else if primary {
                    theme.accent
                } else {
                    theme.foreground
                })
                .when(enabled, |b| b.cursor_pointer().hover(|s| s.bg(theme.accent.opacity(0.15))))
                .whitespace_nowrap()
                .child(label)
        };

        let mut body = div().flex().flex_col().gap_2();
        match &state.sdk {
            None => body = body.child("Looking for the Android SDK…"),
            Some(None) => {
                body = body.child(div().text_color(theme.warning).child(
                    "No Android SDK found. Install Android Studio (or the command-line tools) and/or set ANDROID_HOME.",
                ));
            }
            Some(Some(sdk)) => {
                body = body.child(div().text_size(px(11.)).child(format!(
                    "SDK: {}",
                    sdk.root.as_ref().map_or_else(|| sdk.adb.display().to_string(), |r| r.display().to_string())
                )));

                // Devices.
                body = body.child(div().text_color(theme.foreground).child("Devices"));
                if state.devices.is_empty() {
                    body = body.child(div().text_size(px(12.)).child(
                        "No devices. Connect a phone with USB debugging enabled (Developer options), or start an emulator below.",
                    ));
                }
                for (i, device) in state.devices.iter().enumerate() {
                    let selected = state.selected.as_deref() == Some(device.serial.as_str());
                    let serial = device.serial.clone();
                    let hint = match device.state.as_str() {
                        "device" => None,
                        "unauthorized" => Some("Accept the “Allow USB debugging” prompt on the phone, then Refresh"),
                        "offline" => Some("Offline — reconnect the cable or restart the emulator"),
                        _ => Some("Not ready"),
                    };
                    body = body.child(
                        div()
                            .id(("android-device", i))
                            .flex()
                            .flex_col()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(if selected { theme.accent } else { theme.border })
                            .when(device.is_ready(), |d| {
                                d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                    this.android.selected = Some(serial.clone());
                                    cx.notify();
                                }))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(div().text_color(theme.foreground).child(device.label()))
                                    .child(div().text_size(px(11.)).child(format!("{} · {}", device.serial, device.state))),
                            )
                            .children(hint.map(|h| div().text_size(px(11.)).text_color(theme.warning).child(h))),
                    );
                }

                // Actions on the selected device.
                let ready = self.selected_device().is_some();
                let android_project = state.project.is_some();
                body = body.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .when(android_project, |row| {
                            row.child(
                                button(("android-run", 0), "Build, Install & Run".into(), true, ready)
                                    .when(ready, |b| b.on_click(cx.listener(|this, _, _, cx| this.android_build_install_run(cx)))),
                            )
                        })
                        .child(
                            button(("android-apk", 0), "Install APK…".into(), !android_project, ready)
                                .when(ready, |b| b.on_click(cx.listener(|this, _, window, cx| this.android_install_apk(window, cx)))),
                        )
                        .child(
                            button(("android-logcat", 0), "Logcat".into(), false, ready)
                                .when(ready, |b| b.on_click(cx.listener(|this, _, _, cx| this.android_logcat(cx)))),
                        ),
                );
                if !android_project {
                    body = body.child(div().text_size(px(11.)).child(
                        "This folder isn't an Android app project (no com.android.application module) — Install APK and Logcat still work.",
                    ));
                }

                // Emulators.
                body = body.child(div().text_color(theme.foreground).child("Emulators"));
                if sdk.emulator.is_none() {
                    body = body.child(div().text_size(px(12.)).child("The Android Emulator isn't installed in this SDK."));
                } else if state.avds.is_empty() {
                    body = body.child(div().text_size(px(12.)).child("No virtual devices — create one in Android Studio's Device Manager."));
                }
                for (i, avd) in state.avds.iter().enumerate() {
                    let name = avd.clone();
                    body = body.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_color(theme.foreground).child(avd.replace('_', " ")))
                            .child(
                                button(("android-launch", i), "Launch".into(), false, true)
                                    .on_click(cx.listener(move |this, _, _, cx| this.launch_avd(name.clone(), cx))),
                            ),
                    );
                }
            }
        }
        if let Some(err) = &state.error {
            body = body.child(div().text_size(px(11.)).text_color(theme.warning).child(err.clone()));
        }

        deferred(
            div()
                .id("android-backdrop")
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
                        .id("android-panel")
                        .w(px(560.))
                        .max_w_full()
                        .max_h(px(600.))
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
                                .child(div().text_size(px(16.)).text_color(theme.foreground).child("Android devices"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap_1()
                                        .child(
                                            button(
                                                ("android-refresh", 0),
                                                if state.refreshing { "Refreshing…".into() } else { "Refresh".into() },
                                                false,
                                                !state.refreshing,
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| this.refresh_android(cx))),
                                        )
                                        .child(button(("android-close", 0), "Close".into(), false, true).on_click(cx.listener(
                                            |this, _, _, cx| {
                                                this.show_android = false;
                                                cx.notify();
                                            },
                                        ))),
                                ),
                        )
                        .child(body),
                ),
        )
        .with_priority(3)
    }
}
