//! A plain text tooltip (GPUI shows any view as a tooltip after a short
//! hover delay; this is the small label used on icon buttons).

use gpui::{AnyView, App, Context, Render, SharedString, Window, div, prelude::*, px};
use rji_theme::Theme;

pub struct Tooltip {
    text: SharedString,
    theme: Theme,
}

impl Tooltip {
    /// Builder for `.tooltip(...)`: `.tooltip(Tooltip::text("New file", theme))`.
    pub fn text(text: impl Into<SharedString>, theme: Theme) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_, cx| cx.new(|_| Tooltip { text: text.clone(), theme }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        // Offset a little from the pointer so the label doesn't sit under it.
        div().pl(px(8.)).pt(px(14.)).child(
            div()
                .px_2()
                .py_1()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_md()
                .text_size(px(12.))
                .text_color(theme.foreground)
                .whitespace_nowrap()
                .child(self.text.clone()),
        )
    }
}
