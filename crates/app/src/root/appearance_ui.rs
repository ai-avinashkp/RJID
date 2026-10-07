//! View ▸ Theme & Fonts…: pick the color theme, and the code and terminal
//! fonts and sizes, with a live preview on the whole window. Done saves;
//! Cancel / Esc puts everything back. Sized to the window: one or two
//! theme columns depending on width, scrolling when the window is short.

use gpui::{Context, MouseButton, SharedString, Window, deferred, div, prelude::*, px};
use rji_settings::{DEFAULT_EDITOR_FONT_SIZE, DEFAULT_TERMINAL_FONT_SIZE, clamp_font_size};
use rji_theme::{Theme, ThemeKind};

use super::RootView;
use crate::fonts::{CODING_FONTS, MONO};

#[derive(Clone, Copy, PartialEq, Eq)]
enum FontTarget {
    Editor,
    Terminal,
}

pub(super) struct AppearancePicker {
    /// Settings when opened, restored on Cancel.
    original: (ThemeKind, Option<String>, f32, Option<String>, f32),
    /// Well-known coding fonts installed here, and the free ones that aren't.
    installed: Vec<&'static str>,
    missing: Vec<(&'static str, &'static str)>,
}

impl RootView {
    pub(super) fn open_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let available: Vec<String> = window.text_system().all_font_names().into_iter().map(|n| n.to_lowercase()).collect();
        let has = |name: &str| available.iter().any(|a| a == &name.to_lowercase());
        let installed = CODING_FONTS.iter().filter(|(n, _)| has(n)).map(|(n, _)| *n).collect();
        let missing = CODING_FONTS.iter().filter(|(n, _)| !has(n)).filter_map(|(n, url)| url.map(|u| (*n, u))).collect();
        let s = &self.app_settings;
        self.appearance = Some(AppearancePicker {
            original: (s.theme, s.editor_font_family.clone(), s.editor_font_size, s.terminal_font_family.clone(), s.terminal_font_size),
            installed,
            missing,
        });
        self.open_menu = None;
        cx.notify();
    }

    /// Done: keep and save.
    pub(super) fn close_appearance(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.appearance.take() else { return };
        if keep {
            self.save_app_settings();
        } else {
            let (theme, ef, es, tf, ts) = picker.original;
            let s = &mut self.app_settings;
            (s.theme, s.editor_font_family, s.editor_font_size, s.terminal_font_family, s.terminal_font_size) = (theme, ef, es, tf, ts);
            self.apply_appearance(cx);
        }
        cx.notify();
    }

    fn preview_theme(&mut self, kind: ThemeKind, cx: &mut Context<Self>) {
        self.app_settings.theme = kind;
        self.apply_appearance(cx);
    }

    /// Up/Down in the theme list. Returns true if handled.
    pub(super) fn appearance_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let all = ThemeKind::ALL;
        let index = all.iter().position(|k| *k == self.app_settings.theme).unwrap_or(0);
        let next = match key {
            "down" | "right" => (index + 1) % all.len(),
            "up" | "left" => (index + all.len() - 1) % all.len(),
            "enter" => {
                self.close_appearance(true, cx);
                return true;
            }
            _ => return false,
        };
        self.preview_theme(all[next], cx);
        true
    }

    fn set_font(&mut self, target: FontTarget, family: Option<&str>, cx: &mut Context<Self>) {
        let family = family.map(str::to_string);
        match target {
            FontTarget::Editor => self.app_settings.editor_font_family = family,
            FontTarget::Terminal => self.app_settings.terminal_font_family = family,
        }
        self.apply_appearance(cx);
    }

    fn step_font_size(&mut self, target: FontTarget, delta: f32, cx: &mut Context<Self>) {
        let size = match target {
            FontTarget::Editor => &mut self.app_settings.editor_font_size,
            FontTarget::Terminal => &mut self.app_settings.terminal_font_size,
        };
        *size = clamp_font_size(*size + delta);
        self.apply_appearance(cx);
    }

    fn reset_appearance(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.app_settings;
        s.theme = ThemeKind::default();
        s.editor_font_family = None;
        s.editor_font_size = DEFAULT_EDITOR_FONT_SIZE;
        s.terminal_font_family = None;
        s.terminal_font_size = DEFAULT_TERMINAL_FONT_SIZE;
        self.apply_appearance(cx);
    }

    pub(super) fn render_appearance(&self, theme: Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let picker = self.appearance.as_ref()?;
        let width = (self.viewport_width - 24.0).clamp(280.0, 720.0);
        let max_h = (self.viewport_height - 72.0).max(260.0);
        let two_columns = width >= 560.0;
        let current = self.app_settings.theme;

        let section = |title: &'static str| {
            div().pt_1().text_size(px(11.)).text_color(theme.accent).child(title)
        };
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

        // --- themes, grouped dark / light ---
        let theme_card = |kind: ThemeKind| {
            let t = Theme::for_kind(kind);
            let selected = kind == current;
            let dot = |c: gpui::Rgba| div().size(px(8.)).rounded_full().bg(c);
            div()
                .id(("theme-card", kind as usize))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .p(px(6.))
                .rounded_md()
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .when(selected, |c| c.bg(theme.accent.opacity(0.12)))
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent.opacity(0.08)))
                // Mini preview: the theme's own background and code colors.
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(3.))
                        .px(px(6.))
                        .h(px(22.))
                        .rounded_sm()
                        .bg(t.background)
                        .border_1()
                        .border_color(t.border)
                        .child(dot(t.syntax.keyword))
                        .child(dot(t.syntax.string))
                        .child(dot(t.syntax.function))
                        .child(dot(t.syntax.type_name))
                        .child(dot(t.accent)),
                )
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(theme.foreground).child(kind.display_name()))
                .when(selected, |c| c.child(div().text_color(theme.accent).child("✓")))
                .on_click(cx.listener(move |this, _, _, cx| this.preview_theme(kind, cx)))
        };
        let theme_grid = |light: bool| {
            let cards = ThemeKind::ALL.iter().filter(|k| k.is_light() == light).map(|k| theme_card(*k).into_any_element());
            if two_columns {
                div().grid().grid_cols(2).gap_1().children(cards)
            } else {
                div().flex().flex_col().gap_1().children(cards)
            }
        };

        // --- fonts ---
        let font_row = |target: FontTarget, current: Option<&str>, size: f32| {
            let id_base = if target == FontTarget::Editor { "editor" } else { "terminal" };
            let chip = |index: usize, label: SharedString, family: Option<&'static str>| {
                let selected = current.map(str::to_lowercase) == family.map(str::to_lowercase);
                div()
                    .id((SharedString::from(format!("{id_base}-font")), index))
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { theme.accent } else { theme.border })
                    .when(selected, |c| c.bg(theme.accent.opacity(0.12)))
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .hover(|s| s.bg(theme.accent.opacity(0.08)))
                    // Each name is shown in its own font.
                    .font_family(SharedString::from(family.unwrap_or(MONO)))
                    .text_color(theme.foreground)
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_font(target, family, cx)))
            };
            let mut chips = vec![chip(0, format!("Default ({MONO})").into(), None).into_any_element()];
            chips.extend(
                picker
                    .installed
                    .iter()
                    .filter(|f| !f.eq_ignore_ascii_case(MONO))
                    .enumerate()
                    .map(|(i, f)| chip(i + 1, (*f).into(), Some(*f)).into_any_element()),
            );
            let stepper = div()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(button(if target == FontTarget::Editor { "editor-size-down" } else { "terminal-size-down" }, "−", false).on_click(cx.listener(move |this, _, _, cx| this.step_font_size(target, -1.0, cx))))
                .child(div().w(px(44.)).flex().justify_center().text_color(theme.foreground).child(format!("{size:.0} px")))
                .child(button(if target == FontTarget::Editor { "editor-size-up" } else { "terminal-size-up" }, "+", false).on_click(cx.listener(move |this, _, _, cx| this.step_font_size(target, 1.0, cx))));
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().flex().flex_row().flex_wrap().gap_1().children(chips))
                .child(div().flex().flex_row().items_center().gap_2().child(div().text_size(px(12.)).text_color(theme.foreground_muted).child("Size")).child(stepper))
        };

        let (editor_family, editor_size) = self.editor_font();
        let (terminal_family, terminal_size) = self.terminal_font();
        let sample = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .whitespace_nowrap()
                    .font_family(editor_family)
                    .text_size(px(editor_size))
                    .child(div().text_color(theme.syntax.keyword).child("public static void "))
                    .child(div().text_color(theme.syntax.function).child("main"))
                    .child(div().text_color(theme.syntax.punctuation).child("("))
                    .child(div().text_color(theme.syntax.type_name).child("String"))
                    .child(div().text_color(theme.syntax.punctuation).child("[] args) { "))
                    .child(div().text_color(theme.syntax.comment).child("// 0O 1lI")),
            )
            .child(
                div()
                    .whitespace_nowrap()
                    .font_family(terminal_family)
                    .text_size(px(terminal_size))
                    .text_color(theme.success)
                    .child("PS C:\\projects\\boot> mvn spring-boot:run  ✓ BUILD SUCCESS"),
            );

        let get_more = (!picker.missing.is_empty()).then(|| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_x_2()
                .text_size(px(11.))
                .text_color(theme.foreground_muted)
                .child("Get more free coding fonts (restart RJID after installing):")
                .children(picker.missing.iter().take(6).enumerate().map(|(i, (name, url))| {
                    let url = *url;
                    div()
                        .id(("get-font", i))
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(format!("{name} ↗"))
                        .on_click(move |_, _, cx| cx.open_url(url))
                }))
        });

        let body = div()
            .id("appearance-body")
            .flex()
            .flex_col()
            .gap_2()
            .min_h_0()
            .overflow_y_scroll()
            .child(section("DARK THEMES"))
            .child(theme_grid(false))
            .child(section("LIGHT THEMES"))
            .child(theme_grid(true))
            .child(section("PREVIEW"))
            .child(sample)
            .child(section("EDITOR FONT"))
            .child(font_row(FontTarget::Editor, self.app_settings.editor_font_family.as_deref(), editor_size))
            .child(section("TERMINAL FONT"))
            .child(font_row(FontTarget::Terminal, self.app_settings.terminal_font_family.as_deref(), terminal_size))
            .children(get_more);

        Some(
            deferred(
                div()
                    .id("appearance-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(40.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("appearance-panel")
                            .w(px(width))
                            .max_h(px(max_h))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .bg(theme.surface)
                            .border_1()
                            .border_color(theme.accent.opacity(0.5))
                            .rounded_lg()
                            .shadow_lg()
                            .text_size(px(13.))
                            .text_color(theme.foreground)
                            .occlude()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(div().text_size(px(16.)).child("Theme & Fonts"))
                                    .child(
                                        div()
                                            .id("appearance-reset")
                                            .text_size(px(11.))
                                            .text_color(theme.accent)
                                            .cursor_pointer()
                                            .hover(|s| s.underline())
                                            .child("Reset to defaults")
                                            .on_click(cx.listener(|this, _, _, cx| this.reset_appearance(cx))),
                                    ),
                            )
                            .child(div().text_size(px(11.)).text_color(theme.foreground_muted).child(
                                "Changes preview on the whole window. ↑↓ switch themes · Enter / Done keeps them · Esc / Cancel undoes.",
                            ))
                            .child(body)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_end()
                                    .gap_2()
                                    .pt_1()
                                    .child(button("appearance-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| this.close_appearance(false, cx))))
                                    .child(button("appearance-done", "Done", true).on_click(cx.listener(|this, _, _, cx| this.close_appearance(true, cx)))),
                            ),
                    ),
            )
            .with_priority(5),
        )
    }
}
