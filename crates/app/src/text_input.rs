//! A single-line text field (find/replace, go-to-line, prompts). Editing
//! rules come from the same unit-tested `TextBuffer` the code editor uses,
//! so it gets selection, word movement, clipboard, and undo for free.

use gpui::{
    App, Bounds, ClipboardItem, Context, ContentMask, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, PaintQuad, Pixels, Point, Render, ShapedLine, SharedString, Style, TextRun,
    Window, div, fill, point, prelude::*, px, relative, size,
};
use rji_editor::TextBuffer;
use rji_theme::Theme;

pub enum TextInputEvent {
    Changed,
    /// Enter (with whether Shift was held — e.g. "previous match").
    Submit { shift: bool },
    Cancel,
}

pub struct TextInput {
    buffer: TextBuffer,
    focus_handle: FocusHandle,
    placeholder: SharedString,
    pub theme: Theme,
    /// Last shaping + bounds, for mouse hit-testing.
    layout: Option<(ShapedLine, Bounds<Pixels>, Pixels)>,
    dragging: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, theme: Theme, cx: &mut Context<Self>) -> Self {
        TextInput {
            buffer: TextBuffer::new(String::new()),
            focus_handle: cx.focus_handle(),
            placeholder: placeholder.into(),
            theme,
            layout: None,
            dragging: false,
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Caret to the end, nothing selected (e.g. after a prefilled prefix).
    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        self.buffer.move_doc_end(false);
        cx.notify();
    }

    /// Replaces the content and selects all of it (so typing overwrites).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let single_line = text.lines().next().unwrap_or("");
        self.buffer = TextBuffer::new(single_line.to_string());
        self.buffer.select_all();
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed);
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let ctrl = k.modifiers.control || k.modifiers.platform;
        let shift = k.modifiers.shift;
        let mut edited = false;
        match k.key.as_str() {
            "enter" => {
                cx.emit(TextInputEvent::Submit { shift });
            }
            "escape" => cx.emit(TextInputEvent::Cancel),
            "left" if ctrl => self.buffer.move_word_left(shift),
            "right" if ctrl => self.buffer.move_word_right(shift),
            "left" => self.buffer.move_left(shift),
            "right" => self.buffer.move_right(shift),
            "home" => self.buffer.move_doc_start(shift),
            "end" => self.buffer.move_doc_end(shift),
            "backspace" if ctrl => {
                self.buffer.delete_word_back();
                edited = true;
            }
            "backspace" => {
                self.buffer.backspace();
                edited = true;
            }
            "delete" => {
                self.buffer.delete_forward();
                edited = true;
            }
            "a" if ctrl => self.buffer.select_all(),
            "c" if ctrl => {
                if let Some(text) = self.buffer.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                }
            }
            "x" if ctrl => {
                if let Some(text) = self.buffer.selected_text().map(str::to_string) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.buffer.backspace();
                    edited = true;
                }
            }
            "v" if ctrl => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    let line = text.lines().next().unwrap_or("").replace('\t', " ");
                    self.buffer.paste(&line);
                    edited = true;
                }
            }
            "z" if ctrl && shift => edited = self.buffer.redo(),
            "z" if ctrl => edited = self.buffer.undo(),
            "y" if ctrl => edited = self.buffer.redo(),
            _ => {
                if ctrl || k.modifiers.alt || k.modifiers.function {
                    return; // app shortcuts (F3, Ctrl+H, ...) bubble up
                }
                let Some(ch) = k.key_char.as_ref().filter(|c| !c.contains('\n') && !c.contains('\r')) else {
                    return;
                };
                self.buffer.insert(ch);
                edited = true;
            }
        }
        if edited {
            self.changed(cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn offset_for(&self, position: Point<Pixels>) -> usize {
        match &self.layout {
            Some((shaped, bounds, shift)) => {
                let x = position.x - bounds.left() + *shift;
                shaped.closest_index_for_x(x.max(px(0.))).min(self.buffer.text().len())
            }
            None => self.buffer.text().len(),
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let focused = self.focus_handle.is_focused(window);
        div()
            .id("text-input")
            .track_focus(&self.focus_handle)
            .key_context("TextInput")
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    let offset = this.offset_for(event.position);
                    if event.click_count >= 2 {
                        this.buffer.select_all();
                    } else {
                        this.buffer.set_cursor(offset, event.modifiers.shift);
                        this.dragging = true;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.dragging {
                    if event.pressed_button == Some(MouseButton::Left) {
                        let offset = this.offset_for(event.position);
                        this.buffer.set_cursor(offset, true);
                        cx.notify();
                    } else {
                        this.dragging = false;
                    }
                }
            }))
            .flex()
            .items_center()
            .h(px(26.))
            .px_1p5()
            .min_w_0()
            .overflow_hidden()
            .rounded_sm()
            .border_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .bg(theme.background)
            .cursor_text()
            .child(InputLineElement {
                input: cx.entity(),
                text: self.buffer.text().to_string(),
                placeholder: self.placeholder.clone(),
                caret: focused.then_some(self.buffer.cursor()),
                selection: self.buffer.selection(),
                theme,
            })
    }
}

/// Shapes the input's text and paints selection + caret over it, scrolled
/// horizontally so the caret stays visible in a narrow field.
struct InputLineElement {
    input: Entity<TextInput>,
    text: String,
    placeholder: SharedString,
    caret: Option<usize>,
    selection: Option<std::ops::Range<usize>>,
    theme: Theme,
}

impl IntoElement for InputLineElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputLineElement {
    type RequestLayoutState = ShapedLine;
    type PrepaintState = (Pixels, Option<PaintQuad>, Option<PaintQuad>);

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ShapedLine) {
        let style = window.text_style();
        let (text, color) = if self.text.is_empty() {
            (self.placeholder.to_string(), self.theme.foreground_muted)
        } else {
            (self.text.clone(), self.theme.foreground)
        };
        let display = if text.is_empty() { " ".to_string() } else { text };
        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let shaped = window
            .text_system()
            .shape_line(display.into(), font_size, &[run], None);
        let mut layout_style = Style::default();
        layout_style.size.width = relative(1.).into();
        layout_style.size.height = window.line_height().into();
        (window.request_layout(layout_style, [], cx), shaped)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut ShapedLine,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let has_text = !self.text.is_empty();
        let caret_x = match (self.caret, has_text) {
            (Some(offset), true) => shaped.x_for_index(offset),
            _ => px(0.),
        };
        let shift = (caret_x - bounds.size.width + px(4.)).max(px(0.));
        let height = bounds.size.height;
        let selection = self.selection.clone().filter(|_| has_text).map(|sel| {
            let x1 = shaped.x_for_index(sel.start) - shift;
            let x2 = shaped.x_for_index(sel.end) - shift;
            fill(
                Bounds::new(point(bounds.left() + x1, bounds.top()), size(x2 - x1, height)),
                self.theme.accent.opacity(0.35),
            )
        });
        let caret = self.caret.map(|_| {
            fill(
                Bounds::new(point(bounds.left() + caret_x - shift, bounds.top()), size(px(1.5), height)),
                self.theme.accent,
            )
        });
        (shift, selection, caret)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut ShapedLine,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let shift = prepaint.0;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.1.take() {
                window.paint_quad(selection);
            }
            let origin = point(bounds.left() - shift, bounds.top());
            shaped
                .paint(origin, bounds.size.height, gpui::TextAlign::Left, None, window, cx)
                .ok();
            if let Some(caret) = prepaint.2.take() {
                window.paint_quad(caret);
            }
        });
        let stored = shaped.clone();
        let has_text = !self.text.is_empty();
        self.input.update(cx, |input, _| {
            input.layout = has_text.then_some((stored, bounds, shift));
        });
    }
}
