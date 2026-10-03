//! Moving panels by dragging their headers: the file tree docks left or
//! right, the terminal at the bottom or on the right.
//!
//! A drag carries a `PanelDrag`; while one is in flight the window shows
//! drop zones (see `RootView::render_dock_zones`). GPUI doesn't expose the
//! type of the active drag, so the panel being dragged is recorded when
//! the drag starts and trusted only while a drag is actually active.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{App, Context, Entity, Render, Window, div, prelude::*, px};
use rji_theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPanel {
    Tree,
    Terminal,
}

impl DockPanel {
    pub fn label(self) -> &'static str {
        match self {
            DockPanel::Tree => "File Tree",
            DockPanel::Terminal => "Terminal",
        }
    }
}

/// Drag payload for a panel header.
#[derive(Clone, Copy)]
pub struct PanelDrag {
    pub panel: DockPanel,
    pub theme: Theme,
}

/// 0 = none, 1 = tree, 2 = terminal.
static DRAGGING: AtomicU8 = AtomicU8::new(0);

/// The panel currently being dragged, if any.
pub fn dragging_panel(cx: &App) -> Option<DockPanel> {
    if !cx.has_active_drag() {
        DRAGGING.store(0, Ordering::Relaxed);
        return None;
    }
    match DRAGGING.load(Ordering::Relaxed) {
        1 => Some(DockPanel::Tree),
        2 => Some(DockPanel::Terminal),
        _ => None,
    }
}

/// `on_drag` constructor: records the drag and returns the floating label.
pub fn start_drag(drag: &PanelDrag, cx: &mut App) -> Entity<DragGhost> {
    DRAGGING.store(if drag.panel == DockPanel::Tree { 1 } else { 2 }, Ordering::Relaxed);
    let (label, theme) = (drag.panel.label(), drag.theme);
    cx.new(|_| DragGhost { label, theme })
}

/// The small label that follows the pointer while dragging a panel.
pub struct DragGhost {
    label: &'static str,
    theme: Theme,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.accent)
            .shadow_lg()
            .text_size(px(12.))
            .text_color(theme.foreground)
            .child(format!("⠿ {} — drop on a highlighted edge", self.label))
    }
}
