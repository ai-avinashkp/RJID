//! Moving panels: drag the file tree's or terminal's header onto a
//! highlighted edge (or use View ▸ Move …). The choice is saved.

use gpui::{Context, div, prelude::*, px};
use rji_settings::{PanelSide, TerminalDock};
use rji_theme::Theme;

use super::{RootView, STATUS_BAR_HEIGHT, TITLEBAR_HEIGHT, TOOLBAR_HEIGHT};
use crate::docking::{DockPanel, PanelDrag, dragging_panel};

/// Where a dragged panel can be dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Left,
    Right,
    Bottom,
}

impl RootView {
    pub(super) fn dock_tree(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        self.app_settings.tree_side = side;
        self.app_settings.show_tree = true;
        self.save_app_settings();
        cx.notify();
    }

    pub(super) fn dock_terminal(&mut self, dock: TerminalDock, cx: &mut Context<Self>) {
        self.app_settings.terminal_dock = dock;
        self.app_settings.show_terminal = true;
        self.save_app_settings();
        cx.notify();
    }

    fn drop_panel(&mut self, drag: &PanelDrag, zone: Zone, cx: &mut Context<Self>) {
        match (drag.panel, zone) {
            (DockPanel::Tree, Zone::Left) => self.dock_tree(PanelSide::Left, cx),
            (DockPanel::Tree, Zone::Right) => self.dock_tree(PanelSide::Right, cx),
            (DockPanel::Terminal, Zone::Bottom) => self.dock_terminal(TerminalDock::Bottom, cx),
            (DockPanel::Terminal, Zone::Right) => self.dock_terminal(TerminalDock::Right, cx),
            _ => {}
        }
    }

    /// Drop targets shown only while a panel header is being dragged.
    pub(super) fn render_dock_zones(&self, theme: Theme, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let panel = dragging_panel(cx)?;
        let zones: &[(Zone, &str)] = match panel {
            DockPanel::Tree => &[(Zone::Left, "Dock File Tree on the left"), (Zone::Right, "Dock File Tree on the right")],
            DockPanel::Terminal => &[(Zone::Bottom, "Dock Terminal at the bottom"), (Zone::Right, "Dock Terminal on the right")],
        };
        let current = match panel {
            DockPanel::Tree => match self.app_settings.tree_side {
                PanelSide::Left => Zone::Left,
                PanelSide::Right => Zone::Right,
            },
            DockPanel::Terminal => match self.app_settings.terminal_dock {
                TerminalDock::Bottom => Zone::Bottom,
                TerminalDock::Right => Zone::Right,
            },
        };

        let layer = div()
            .absolute()
            .top(px(TITLEBAR_HEIGHT + TOOLBAR_HEIGHT))
            .bottom(px(STATUS_BAR_HEIGHT))
            .left_0()
            .right_0()
            .children(zones.iter().map(|&(zone, label)| {
                let base = div()
                    .id(match zone {
                        Zone::Left => "dock-left",
                        Zone::Right => "dock-right",
                        Zone::Bottom => "dock-bottom",
                    })
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    .m_2()
                    .rounded_lg()
                    .border_2()
                    .border_color(theme.accent.opacity(0.6))
                    .bg(theme.accent.opacity(0.08))
                    .text_color(theme.foreground)
                    .text_size(px(13.))
                    .child(if zone == current { format!("{label} (current)") } else { label.to_string() })
                    .drag_over::<PanelDrag>(move |style, _, _, _| style.bg(theme.accent.opacity(0.28)).border_color(theme.accent))
                    .on_drop(cx.listener(move |this, drag: &PanelDrag, _, cx| this.drop_panel(drag, zone, cx)));
                match zone {
                    Zone::Left => base.top_0().bottom_0().left_0().w(px(220.)),
                    Zone::Right => base.top_0().bottom_0().right_0().w(px(220.)),
                    Zone::Bottom => base.left_0().right_0().bottom_0().h(px(160.)),
                }
            }));
        Some(layer)
    }
}
