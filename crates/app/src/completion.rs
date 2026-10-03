//! Code-completion popup: the filtered list model plus its rendering.
//!
//! Items come from two sources, merged: jdtls (`textDocument/completion`,
//! real semantic results, arriving asynchronously) and a local fallback
//! that's available instantly and also works with no language server at
//! all — Java keywords, common snippets, and identifiers already present
//! in the file.

use std::collections::HashSet;

use gpui::{Context, MouseButton, MouseDownEvent, div, prelude::*, px};
use rji_theme::Theme;

use crate::editor_view::CodeEditorView;

const VISIBLE_ROWS: usize = 10;
const ROW_HEIGHT: f32 = 24.0;
const MENU_WIDTH: f32 = 460.0;

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    /// Text to insert. A `$0` marks where the caret lands afterwards.
    pub insert_text: String,
    pub detail: Option<String>,
    pub kind: &'static str,
    /// Edits made on accept besides the insertion — the `import` for a
    /// class from another package (from the language server).
    pub additional_edits: Vec<rji_lsp_client::TextEdit>,
}

pub struct CompletionMenu {
    /// Document offset where the word being completed starts; accepting an
    /// item replaces `replace_start..caret`.
    pub replace_start: usize,
    /// True while a language-server request for this menu is in flight.
    pub loading: bool,
    items: Vec<CompletionItem>,
    filtered: Vec<usize>,
    selected: usize,
    filter: String,
}

impl CompletionMenu {
    pub fn new(replace_start: usize, prefix: &str) -> Self {
        CompletionMenu {
            replace_start,
            loading: false,
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            filter: prefix.to_string(),
        }
    }

    pub fn set_items(&mut self, items: Vec<CompletionItem>) {
        self.items = items;
        self.refilter();
    }

    /// Language-server items take priority; local fallback items stay only
    /// if the server didn't already offer the same insertion.
    pub fn merge_lsp_items(&mut self, lsp_items: Vec<CompletionItem>) {
        let seen: HashSet<String> = lsp_items.iter().map(|i| i.insert_text.clone()).collect();
        let fallback = std::mem::take(&mut self.items);
        self.items = lsp_items;
        self.items
            .extend(fallback.into_iter().filter(|i| !seen.contains(&i.insert_text)));
        self.refilter();
    }

    pub fn set_filter(&mut self, prefix: &str) {
        self.filter = prefix.to_string();
        self.refilter();
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + self.filtered.len() - 1) % self.filtered.len();
        }
    }

    pub fn select(&mut self, visible_index: usize) {
        if visible_index < self.filtered.len() {
            self.selected = visible_index;
        }
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.filtered.get(self.selected).map(|&i| &self.items[i])
    }

    /// Case-insensitive match on the item's leading identifier: prefix
    /// matches first, then substring matches, each keeping source order.
    fn refilter(&mut self) {
        let needle = self.filter.to_lowercase();
        let mut prefix_hits = Vec::new();
        let mut substring_hits = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            let key = filter_key(&item.label).to_lowercase();
            if needle.is_empty() || key.starts_with(&needle) {
                prefix_hits.push(index);
            } else if key.contains(&needle) {
                substring_hits.push(index);
            }
        }
        prefix_hits.extend(substring_hits);
        self.filtered = prefix_hits;
        self.selected = 0;
    }
}

/// The identifier an item is matched on: `println(String x) : void` is
/// matched as `println`.
fn filter_key(label: &str) -> &str {
    let end = label
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(label.len());
    &label[..end]
}

const JAVA_KEYWORDS: &[&str] = &[
    "abstract", "assert", "boolean", "break", "byte", "case", "catch", "char", "class",
    "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally",
    "float", "for", "if", "implements", "import", "instanceof", "int", "interface", "long",
    "new", "null", "package", "private", "protected", "public", "record", "return", "short",
    "static", "super", "switch", "synchronized", "this", "throw", "throws", "try", "var",
    "void", "volatile", "while", "true", "false", "String",
];

/// `(trigger, description, body)`. Bodies use `$0` for the caret and are
/// re-indented to the current line when inserted.
const SNIPPETS: &[(&str, &str, &str)] = &[
    ("psvm", "main method", "public static void main(String[] args) {\n    $0\n}"),
    ("main", "main method", "public static void main(String[] args) {\n    $0\n}"),
    ("sout", "print to stdout", "System.out.println($0);"),
    ("serr", "print to stderr", "System.err.println($0);"),
    ("fori", "indexed for loop", "for (int i = 0; i < $0; i++) {\n    \n}"),
    ("foreach", "for-each loop", "for (var item : $0) {\n    \n}"),
    ("ifelse", "if / else", "if ($0) {\n    \n} else {\n    \n}"),
    ("trycatch", "try / catch", "try {\n    $0\n} catch (Exception e) {\n    e.printStackTrace();\n}"),
    ("while", "while loop", "while ($0) {\n    \n}"),
    ("prsf", "private static final", "private static final $0"),
    ("psf", "public static final", "public static final $0"),
    ("ctor", "constructor", "public $0() {\n    \n}"),
];

/// Instant, server-free suggestions. `include_keywords` is false right
/// after a `.` (member access), where keywords/snippets make no sense.
pub fn fallback_items(document: &str, prefix: &str, include_keywords: bool) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    if include_keywords {
        for (trigger, description, body) in SNIPPETS {
            items.push(CompletionItem {
                label: (*trigger).to_string(),
                insert_text: (*body).to_string(),
                detail: Some((*description).to_string()),
                kind: "snippet",
                additional_edits: Vec::new(),
            });
        }
        for keyword in JAVA_KEYWORDS {
            items.push(CompletionItem {
                label: (*keyword).to_string(),
                insert_text: (*keyword).to_string(),
                detail: None,
                kind: "keyword",
                additional_edits: Vec::new(),
            });
        }
    }

    // Words already in this file (at least 3 chars, not the word being
    // typed itself), most frequent first.
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for word in document
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| w.len() >= 3 && !w.starts_with(|c: char| c.is_ascii_digit()))
    {
        *counts.entry(word).or_default() += 1;
    }
    let mut words: Vec<(&str, usize)> = counts
        .into_iter()
        .filter(|(w, _)| *w != prefix && !JAVA_KEYWORDS.contains(w))
        .collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    items.extend(words.into_iter().take(300).map(|(w, _)| CompletionItem {
        label: w.to_string(),
        insert_text: w.to_string(),
        detail: None,
        kind: "text",
        additional_edits: Vec::new(),
    }));
    items
}

/// Where the popup may go, in the editor view's own coordinates.
pub struct MenuPlacement {
    /// Caret position: left edge, and top/bottom of the caret's line.
    pub caret_x: f32,
    pub line_top: f32,
    pub line_bottom: f32,
    /// Editor area the popup must stay inside.
    pub min_x: f32,
    pub view_w: f32,
    pub view_h: f32,
    pub zoom: f32,
}

/// Splits a label into its name and a muted remainder:
/// `println(String x) : void` -> (`println`, `(String x) : void`),
/// `ArrayList - java.util` -> (`ArrayList`, `java.util`).
pub fn split_label(label: &str) -> (&str, &str) {
    if let Some((name, rest)) = label.split_once(" - ") {
        return (name, rest);
    }
    match label.find('(') {
        Some(at) if at > 0 => (&label[..at], &label[at..]),
        _ => (label, ""),
    }
}

/// One-letter badge for an item kind.
fn kind_badge(kind: &str) -> &'static str {
    match kind {
        "method" | "constructor" => "m",
        "class" => "C",
        "interface" => "I",
        "enum" => "E",
        "enum const" | "constant" => "K",
        "field" => "f",
        "variable" => "v",
        "package" => "P",
        "keyword" => "k",
        "snippet" => "S",
        _ => "w",
    }
}

/// The popup: single-line rows (kind badge, name, muted signature) and a
/// footer with the selected item's full detail. Rows are clickable;
/// keyboard navigation lives in the editor.
pub fn render_completion_menu(
    menu: &CompletionMenu,
    place: MenuPlacement,
    theme: Theme,
    cx: &mut Context<CodeEditorView>,
) -> impl IntoElement + use<> {
    let z = place.zoom;
    let row_h = ROW_HEIGHT * z;
    let font = 13.0 * z;
    let small = 11.5 * z;
    let width = (MENU_WIDTH * z).min((place.view_w - place.min_x - 8.0).max(200.0));

    // Window of rows scrolled so the selection stays visible.
    let first = menu
        .selected
        .saturating_sub(VISIBLE_ROWS - 1)
        .min(menu.filtered.len().saturating_sub(VISIBLE_ROWS));
    let visible = menu.filtered.len().saturating_sub(first).min(VISIBLE_ROWS);
    let rows = menu
        .filtered
        .iter()
        .enumerate()
        .skip(first)
        .take(VISIBLE_ROWS)
        .map(|(visible_index, &item_index)| {
            let item = &menu.items[item_index];
            let is_selected = visible_index == menu.selected;
            let (name, rest) = split_label(&item.label);
            let color = kind_color(item.kind, theme);
            div()
                .id(("completion", visible_index))
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap(px(8. * z))
                .h(px(row_h))
                .px(px(6. * z))
                .overflow_hidden()
                .when(is_selected, |s| s.bg(theme.accent.opacity(0.28)))
                .hover(|s| s.bg(theme.accent.opacity(0.14)))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .size(px(16. * z))
                        .rounded_sm()
                        .bg(color.opacity(0.18))
                        .text_color(color)
                        .text_size(px(small))
                        .child(kind_badge(item.kind)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .items_baseline()
                        .gap(px(6. * z))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().flex_shrink_0().text_color(theme.foreground).child(name.to_string()))
                        .when(!rest.is_empty(), |s| {
                            s.child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(small))
                                    .text_color(theme.foreground_muted)
                                    .child(rest.to_string()),
                            )
                        }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                        this.pick_completion(visible_index, cx);
                        cx.stop_propagation();
                    }),
                )
        })
        .collect::<Vec<_>>();

    // Footer: the selected item's detail (or snippet body), unless it
    // would just repeat the row.
    let footer = menu.selected_item().and_then(|item| {
        let text = if item.kind == "snippet" {
            let body = item.insert_text.replace("$0", "");
            let preview: Vec<&str> = body.lines().take(3).collect();
            format!("{}\n{}", item.detail.clone().unwrap_or_default(), preview.join("\n"))
        } else {
            item.detail.clone()?
        };
        let (name, rest) = split_label(&item.label);
        let redundant = text.trim().is_empty()
            || text == item.label
            || (!rest.is_empty() && text == format!("{rest}.{name}"));
        (!redundant).then_some(text)
    });
    let footer_lines = footer.as_ref().map_or(0, |f| f.lines().count().min(4));
    let height = visible as f32 * row_h
        + 8.0 * z
        + if footer.is_some() { footer_lines as f32 * small * 1.4 + 12.0 * z } else { 0.0 }
        + if menu.loading { row_h } else { 0.0 };

    // Below the caret line if it fits, otherwise above it.
    let below = place.line_bottom + height <= place.view_h || place.line_top < height;
    let top = if below { place.line_bottom + 2.0 } else { (place.line_top - height - 2.0).max(0.0) };
    let left = place.caret_x.min(place.view_w - width - 8.0).max(place.min_x);

    div()
        .id("completion-menu")
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .flex()
        .flex_col()
        .py(px(4. * z))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .shadow_lg()
        .overflow_hidden()
        .text_size(px(font))
        .occlude()
        .children(rows)
        .when(menu.loading, |s| {
            s.child(
                div()
                    .h(px(row_h))
                    .flex()
                    .items_center()
                    .px(px(8. * z))
                    .text_size(px(small))
                    .text_color(theme.foreground_muted)
                    .child("language server…"),
            )
        })
        .children(footer.map(|text| {
            div()
                .mt(px(4. * z))
                .px(px(8. * z))
                .pt(px(4. * z))
                .border_t_1()
                .border_color(theme.border)
                .text_size(px(small))
                .text_color(theme.foreground_muted)
                .max_h(px(small * 1.4 * 4.0 + 4.0))
                .overflow_hidden()
                .child(text)
        }))
}

fn kind_color(kind: &str, theme: Theme) -> gpui::Rgba {
    match kind {
        "method" | "constructor" => theme.syntax.keyword,
        "class" | "interface" | "enum" => theme.syntax.type_name,
        "field" | "variable" | "constant" | "enum const" => theme.syntax.number,
        "snippet" => theme.syntax.string,
        "keyword" => theme.accent,
        _ => theme.foreground_muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            insert_text: label.to_string(),
            detail: None,
            kind: "text",
            additional_edits: Vec::new(),
        }
    }

    #[test]
    fn prefix_matches_rank_before_substring_matches() {
        let mut menu = CompletionMenu::new(0, "count");
        menu.set_items(vec![item("getCount"), item("counter"), item("other")]);
        assert_eq!(menu.selected_item().unwrap().label, "counter");
        menu.select_next();
        assert_eq!(menu.selected_item().unwrap().label, "getCount");
        menu.select_next();
        assert_eq!(menu.selected_item().unwrap().label, "counter"); // wraps
    }

    #[test]
    fn filtering_uses_leading_identifier_of_method_labels() {
        let mut menu = CompletionMenu::new(0, "printl");
        menu.set_items(vec![item("println(String x) : void"), item("print(int i) : void")]);
        assert_eq!(menu.selected_item().unwrap().label, "println(String x) : void");
    }

    #[test]
    fn lsp_items_replace_duplicate_fallback_items() {
        let mut menu = CompletionMenu::new(0, "");
        menu.set_items(vec![item("count"), item("local")]);
        menu.merge_lsp_items(vec![item("count")]);
        assert_eq!(menu.items.len(), 2);
    }

    #[test]
    fn labels_split_into_name_and_muted_rest() {
        assert_eq!(split_label("println(String x) : void"), ("println", "(String x) : void"));
        assert_eq!(split_label("ArrayStoreException - java.lang"), ("ArrayStoreException", "java.lang"));
        assert_eq!(split_label("count"), ("count", ""));
    }

    #[test]
    fn fallback_offers_snippets_and_document_words() {
        let items = fallback_items("int counter = 1; counter++;", "cou", true);
        assert!(items.iter().any(|i| i.label == "sout" && i.kind == "snippet"));
        assert!(items.iter().any(|i| i.label == "counter"));
        let after_dot = fallback_items("int counter = 1;", "", false);
        assert!(!after_dot.iter().any(|i| i.kind == "keyword"));
    }
}
