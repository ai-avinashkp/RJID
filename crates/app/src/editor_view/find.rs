//! The editor's find / replace / go-to-line bar (Ctrl+F, Ctrl+H, Ctrl+G).
//! Matching itself lives in `rji_editor::search` (unit-tested); this is
//! the UI state and wiring.

use std::ops::Range;

use gpui::{Context, Entity, Focusable, Subscription, Window, div, prelude::*, px};
use rji_editor::search::{self, SearchOptions};

use super::CodeEditorView;
use crate::text_input::{TextInput, TextInputEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FindMode {
    Find,
    GoToLine,
}

pub(super) struct FindBar {
    pub mode: FindMode,
    pub query: Entity<TextInput>,
    pub replacement: Entity<TextInput>,
    pub show_replace: bool,
    /// Copy of the query input's text, so matches can be recomputed after
    /// any edit without a GPUI context.
    pub query_text: String,
    pub options: SearchOptions,
    pub matches: Vec<Range<usize>>,
    /// Index into `matches` of the highlighted ("current") match.
    pub current: Option<usize>,
    pub error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl CodeEditorView {
    pub(super) fn open_find(&mut self, mode: FindMode, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme;
        if self.find.as_ref().is_none_or(|bar| bar.mode != mode) {
            let placeholder = match mode {
                FindMode::Find => "Find".to_string(),
                FindMode::GoToLine => format!("Go to line (1–{}), or line:column", self.buffer.line_count()),
            };
            let query = cx.new(|cx| TextInput::new(placeholder, theme, cx));
            let replacement = cx.new(|cx| TextInput::new("Replace", theme, cx));
            let subscriptions = vec![
                cx.subscribe_in(&query, window, |this, _, event: &TextInputEvent, window, cx| {
                    this.on_query_event(event, window, cx)
                }),
                cx.subscribe_in(&replacement, window, |this, _, event: &TextInputEvent, window, cx| match event {
                    TextInputEvent::Submit { .. } => this.replace_current(window, cx),
                    TextInputEvent::Cancel => this.close_find(window, cx),
                    TextInputEvent::Changed => {}
                }),
            ];
            self.find = Some(FindBar {
                mode,
                query,
                replacement,
                show_replace: false,
                query_text: String::new(),
                options: SearchOptions::default(),
                matches: Vec::new(),
                current: None,
                error: None,
                _subscriptions: subscriptions,
            });
        }
        self.completion = None;

        let Some(bar) = self.find.as_mut() else {
            return;
        };
        bar.show_replace = replace && mode == FindMode::Find;
        // Seed the query with a single-line selection (the usual editor
        // behavior); otherwise keep the previous query, all selected.
        let seed = match mode {
            FindMode::Find => self
                .buffer
                .selected_text()
                .filter(|t| !t.is_empty() && !t.contains('\n'))
                .map(str::to_string),
            FindMode::GoToLine => Some(String::new()),
        };
        let query = bar.query.clone();
        let text = query.update(cx, |input, cx| {
            let text = seed.unwrap_or_else(|| input.text().to_string());
            input.set_text(&text, cx);
            text
        });
        bar.query_text = text;
        let handle = query.focus_handle(cx);
        window.focus(&handle, cx);
        self.recompute_matches();
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn on_query_event(&mut self, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.find.as_ref().map(|bar| bar.mode);
        match (event, mode) {
            (TextInputEvent::Changed, Some(FindMode::Find)) => {
                if let Some(bar) = self.find.as_mut() {
                    bar.query_text = bar.query.read(cx).text().to_string();
                }
                self.recompute_matches();
                // Jump to the first match at/after the caret as you type.
                if let Some(bar) = &self.find
                    && let Some(current) = bar.current
                {
                    let range = bar.matches[current].clone();
                    self.reveal_range(range);
                }
                cx.notify();
            }
            (TextInputEvent::Submit { shift }, Some(FindMode::Find)) => self.find_step(!shift, window, cx),
            (TextInputEvent::Submit { .. }, Some(FindMode::GoToLine)) => self.submit_go_to_line(window, cx),
            (TextInputEvent::Cancel, _) => self.close_find(window, cx),
            _ => {}
        }
    }

    /// Re-runs the search (after the query, an option, or the text
    /// changes) and picks the first match at or after the caret.
    pub(super) fn recompute_matches(&mut self) {
        let Some(bar) = self.find.as_mut() else {
            return;
        };
        if bar.mode != FindMode::Find {
            return;
        }
        match search::find_all(self.buffer.text(), &bar.query_text, bar.options) {
            Ok(matches) => {
                bar.error = None;
                let anchor = self.buffer.selection().map_or(self.buffer.cursor(), |s| s.start);
                bar.current = if matches.is_empty() {
                    None
                } else {
                    Some(matches.iter().position(|m| m.start >= anchor).unwrap_or(0))
                };
                bar.matches = matches;
            }
            Err(message) => {
                bar.error = Some(message);
                bar.matches.clear();
                bar.current = None;
            }
        }
    }

    pub(super) fn find_step(&mut self, forward: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_mut() else {
            return;
        };
        if bar.matches.is_empty() {
            return;
        }
        let count = bar.matches.len();
        let caret = self.buffer.cursor();
        let next = if forward {
            // First match at/after the caret (the caret sits at the end of
            // a selected match, so this moves past it), wrapping around.
            bar.matches.iter().position(|m| m.start >= caret).unwrap_or(0)
        } else {
            let start = self.buffer.selection().map_or(caret, |s| s.start);
            bar.matches.iter().rposition(|m| m.start < start).unwrap_or(count - 1)
        };
        bar.current = Some(next);
        let range = bar.matches[next].clone();
        self.reveal_range(range);
        cx.notify();
    }

    /// Selects a range and scrolls it into view (centered).
    fn reveal_range(&mut self, range: Range<usize>) {
        self.buffer.select_range(range.clone());
        let (line, _) = self.buffer.line_col(range.start);
        self.scroll_handle
            .scroll_to_item_strict(line, gpui::ScrollStrategy::Center);
    }

    pub(super) fn replace_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        let query = bar.query.read(cx).text().to_string();
        let replacement = bar.replacement.read(cx).text().to_string();
        let options = bar.options;
        // Only replace when the selection *is* a match (i.e. the user has
        // just navigated to it); otherwise the first press just finds.
        let target = self
            .buffer
            .selection()
            .filter(|sel| bar.matches.contains(sel));
        match target {
            Some(range) => {
                let text = search::replacement_for(self.buffer.text(), range.clone(), &query, &replacement, options);
                self.buffer.replace_range(range, &text);
                self.after_edit();
                self.find_step(true, window, cx);
            }
            None => self.find_step(true, window, cx),
        }
        cx.notify();
    }

    pub(super) fn replace_all_matches(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        let query = bar.query.read(cx).text().to_string();
        let replacement = bar.replacement.read(cx).text().to_string();
        let options = bar.options;
        if let Ok((new_text, count)) = search::replace_all(self.buffer.text(), &query, &replacement, options)
            && count > 0
        {
            let caret = self.buffer.cursor().min(new_text.len());
            // One undo step for the whole replace-all.
            self.buffer.replace_all(&new_text);
            self.buffer.set_cursor(caret, false);
            self.after_edit();
        }
        cx.notify();
    }

    fn toggle_option(&mut self, option: fn(&mut SearchOptions), cx: &mut Context<Self>) {
        if let Some(bar) = self.find.as_mut() {
            option(&mut bar.options);
        }
        self.recompute_matches();
        cx.notify();
    }

    fn submit_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_ref() else {
            return;
        };
        let text = bar.query.read(cx).text().trim().to_string();
        let mut parts = text.split(':');
        let line = parts.next().and_then(|l| l.trim().parse::<usize>().ok());
        let column = parts.next().and_then(|c| c.trim().parse::<usize>().ok());
        if let Some(line) = line.filter(|l| *l >= 1) {
            let line = (line - 1).min(self.buffer.line_count() - 1);
            let range = self.buffer.line_range(line);
            let offset = match column {
                Some(col) if col >= 1 => self.buffer.text()[range.clone()]
                    .char_indices()
                    .nth(col - 1)
                    .map_or(range.end, |(i, _)| range.start + i),
                _ => range.start,
            };
            self.buffer.set_cursor(offset, false);
            self.scroll_handle
                .scroll_to_item_strict(line, gpui::ScrollStrategy::Center);
            self.close_find(window, cx);
        }
    }

    /// Byte-column highlight spans on `line` for the find matches:
    /// `(start, end, is_current)`.
    pub(super) fn match_spans_on_line(&self, line_range: Range<usize>) -> Vec<(usize, usize, bool)> {
        let Some(bar) = self.find.as_ref().filter(|b| b.mode == FindMode::Find) else {
            return Vec::new();
        };
        // Matches are sorted; binary-search to the first that can overlap.
        let first = bar.matches.partition_point(|m| m.end <= line_range.start);
        bar.matches[first..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.start <= line_range.end)
            .map(|(i, m)| {
                (
                    m.start.max(line_range.start) - line_range.start,
                    m.end.min(line_range.end) - line_range.start,
                    bar.current == Some(first + i),
                )
            })
            .collect()
    }

    pub(super) fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let bar = self.find.as_ref()?;
        let theme = self.theme;
        let mode = bar.mode;

        let status = match mode {
            FindMode::GoToLine => String::new(),
            FindMode::Find => match (&bar.error, bar.current) {
                (Some(err), _) => err.clone(),
                (None, _) if bar.query_text.is_empty() => String::new(),
                (None, _) if bar.matches.is_empty() => "No results".to_string(),
                (None, Some(i)) => {
                    let plus = if bar.matches.len() >= search::MAX_MATCHES { "+" } else { "" };
                    format!("{} of {}{plus}", i + 1, bar.matches.len())
                }
                (None, None) => String::new(),
            },
        };
        let has_error = bar.error.is_some();
        let options = bar.options;

        let toggle = |id: &'static str, label: &'static str, on: bool, set: fn(&mut SearchOptions), cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_size(px(12.))
                .cursor_pointer()
                .text_color(if on { theme.foreground } else { theme.foreground_muted })
                .when(on, |s| s.bg(theme.accent.opacity(0.35)))
                .hover(|s| s.bg(theme.accent.opacity(0.18)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(set, cx)))
        };
        let button = |id: &'static str, label: &'static str, cx: &mut Context<Self>, action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .text_size(px(12.))
                .cursor_pointer()
                .text_color(theme.foreground_muted)
                .hover(|s| s.bg(theme.accent.opacity(0.18)).text_color(theme.foreground))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
        };

        let mut find_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(div().w(px(230.)).child(bar.query.clone()));
        if mode == FindMode::Find {
            find_row = find_row
                .child(toggle("find-case", "Aa", options.case_sensitive, |o| o.case_sensitive = !o.case_sensitive, cx))
                .child(toggle("find-word", "W", options.whole_word, |o| o.whole_word = !o.whole_word, cx))
                .child(toggle("find-regex", ".*", options.regex, |o| o.regex = !o.regex, cx))
                .child(
                    div()
                        .min_w(px(70.))
                        .text_size(px(12.))
                        .text_color(if has_error { theme.error } else { theme.foreground_muted })
                        .child(status),
                )
                .child(button("find-prev", "↑", cx, |this, window, cx| this.find_step(false, window, cx)))
                .child(button("find-next", "↓", cx, |this, window, cx| this.find_step(true, window, cx)))
                .child(button("find-toggle-replace", if bar.show_replace { "▴" } else { "▾" }, cx, |this, _, cx| {
                    if let Some(bar) = this.find.as_mut() {
                        bar.show_replace = !bar.show_replace;
                    }
                    cx.notify();
                }));
        }
        find_row = find_row.child(button("find-close", "×", cx, |this, window, cx| this.close_find(window, cx)));

        let replace_row = (mode == FindMode::Find && bar.show_replace).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(div().w(px(230.)).child(bar.replacement.clone()))
                .child(button("replace-one", "Replace", cx, |this, window, cx| this.replace_current(window, cx)))
                .child(button("replace-all", "All", cx, |this, _, cx| this.replace_all_matches(cx)))
        });

        Some(
            div()
                .id("find-bar")
                .absolute()
                .top(px(4.))
                .right(px(18.))
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_md()
                .text_color(theme.foreground)
                // Clicks on the bar mustn't reach the text lines beneath.
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(find_row)
                .children(replace_row),
        )
    }
}
