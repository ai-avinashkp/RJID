//! Several terminals in one panel, as tabs.
//!
//! Each tab is its own shell (and its own server-port watcher). Typing
//! `exit` closes that tab; closing the last one hides the panel, and the
//! next Run / Ctrl+` starts a fresh shell. One shared header (tabs, +,
//! Clear, Restart) replaces each terminal's own header and is also the
//! drag handle for docking the panel.

use gpui::{Context, Entity, Focusable, MouseButton, Subscription, Window, div, prelude::*, px};
use rji_theme::Theme;

use super::RootView;
use crate::docking::{DockPanel, PanelDrag, start_drag};
use crate::terminal_view::{HEADER_HEIGHT, TerminalEvent, TerminalView};

pub(super) struct TerminalTab {
    pub(super) view: Entity<TerminalView>,
    name: String,
    /// Result of the last build/run in this terminal (green / red dot).
    pub(super) status: Option<bool>,
    _subscription: Subscription,
}

/// "powershell", "bash", … — the shell's file name without extension.
fn shell_name() -> String {
    let shell = rji_terminal::default_shell();
    std::path::Path::new(&shell)
        .file_stem()
        .map_or(shell.clone(), |s| s.to_string_lossy().into_owned())
}

impl RootView {
    /// The terminal shown in the panel, if any.
    pub(super) fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        self.terminals.get(self.active_terminal).map(|t| &t.view)
    }

    /// Opens a new terminal tab in the workspace folder and makes it active.
    pub(super) fn new_terminal(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let cwd = self.workspace_root.clone().unwrap_or_else(|| std::path::PathBuf::from("."));
        let (theme, zoom) = (self.theme(), self.app_settings.ui_zoom);
        let font = self.terminal_font();
        let view = cx.new(|cx| {
            let mut terminal = TerminalView::new(cwd, theme, cx);
            terminal.zoom = zoom;
            (terminal.font_family, terminal.font_size) = font.clone();
            terminal.show_header = false;
            terminal
        });
        let subscription = cx.subscribe(&view, |this, view, event: &TerminalEvent, cx| {
            match *event {
                TerminalEvent::ServerStarted(port) => {
                    this.server_port = Some(port);
                    this.notify_user(format!("Server running at http://localhost:{port}/"));
                }
                TerminalEvent::PortInUse(port) => {
                    this.server_port = None;
                    this.notify_user(format!(
                        "Port {port} is already in use — stop the other server, or set server.port in application.properties"
                    ));
                }
                TerminalEvent::BuildFinished(ok) => {
                    if let Some(tab) = this.terminals.iter_mut().find(|t| t.view == view) {
                        tab.status = Some(ok);
                    }
                    this.notify_user(if ok { "✓ Build succeeded" } else { "✗ Build failed — see the terminal" });
                }
                TerminalEvent::Exited => {
                    if let Some(index) = this.terminals.iter().position(|t| t.view == view) {
                        this.close_terminal(index, cx);
                    }
                }
            }
            cx.notify();
        });
        self.terminal_counter += 1;
        let name = format!("{} {}", shell_name(), self.terminal_counter);
        self.terminals.push(TerminalTab { view: view.clone(), name, status: None, _subscription: subscription });
        self.active_terminal = self.terminals.len() - 1;
        cx.notify();
        view
    }

    /// The active terminal, creating one if the panel has none.
    pub(super) fn ensure_terminal(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        match self.active_terminal() {
            Some(view) => view.clone(),
            None => self.new_terminal(cx),
        }
    }

    /// Closes a tab (killing its shell). Closing the last one hides the
    /// panel; it comes back with a fresh shell when needed.
    pub(super) fn close_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        let tab = self.terminals.remove(index);
        tab.view.update(cx, |t, _| t.shutdown());
        if self.active_terminal >= self.terminals.len() {
            self.active_terminal = self.terminals.len().saturating_sub(1);
        } else if index < self.active_terminal {
            self.active_terminal -= 1;
        }
        if self.terminals.is_empty() {
            // Hide for now without changing the saved preference.
            self.app_settings.show_terminal = false;
            self.server_port = None;
        }
        cx.notify();
    }

    /// Kills every shell (workspace switch, window close).
    pub(super) fn shutdown_terminals(&mut self, cx: &mut Context<Self>) {
        for tab in self.terminals.drain(..) {
            tab.view.update(cx, |t, _| t.shutdown());
        }
        self.active_terminal = 0;
    }

    pub(super) fn for_each_terminal(&self, cx: &mut Context<Self>, mut f: impl FnMut(&mut TerminalView)) {
        for tab in &self.terminals {
            tab.view.update(cx, |view, cx| {
                f(view);
                cx.notify();
            });
        }
    }

    pub(super) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.ensure_terminal(cx);
        let handle = view.focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// The panel: the shared header plus the active terminal, `height` px tall.
    pub(super) fn render_terminal_panel(&self, theme: Theme, height: f32, cx: &Context<Self>) -> impl IntoElement + use<> {
        let header_button = |id: (&'static str, usize), label: &'static str| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_2()
                .rounded_sm()
                .text_color(theme.foreground_muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent.opacity(0.16)).text_color(theme.foreground))
                .child(label)
        };

        let tabs = self.terminals.iter().enumerate().map(|(i, tab)| {
            let active = i == self.active_terminal;
            div()
                .id(("term-tab", i))
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .h_full()
                .pl_2()
                .pr_1()
                .cursor_pointer()
                .border_b_2()
                .border_color(if active { theme.accent } else { gpui::transparent_black().into() })
                .text_color(if active { theme.foreground } else { theme.foreground_muted })
                .hover(|s| s.text_color(theme.foreground))
                .children(tab.status.map(|ok| {
                    div().text_size(px(9.)).text_color(if ok { theme.success } else { theme.error }).child("●")
                }))
                .child(div().whitespace_nowrap().child(tab.name.clone()))
                .child(
                    div()
                        .id(("term-tab-close", i))
                        .px_1()
                        .rounded_sm()
                        .text_color(theme.foreground_muted)
                        .hover(|s| s.bg(theme.accent.opacity(0.2)).text_color(theme.foreground))
                        .child("×")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_terminal(i, cx);
                            }),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.active_terminal = i;
                    this.focus_terminal(window, cx);
                    cx.notify();
                }))
        });

        let scrolled_up = self.active_terminal().is_some_and(|t| t.read(cx).is_scrolled_up());
        let header = div()
            .id("terminal-panel-header")
            // Drag the header to dock the terminal at the bottom or right.
            .on_drag(PanelDrag { panel: DockPanel::Terminal, theme }, |drag, _, _, cx| start_drag(drag, cx))
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(px(HEADER_HEIGHT))
            .px_2()
            .bg(theme.surface)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(12.))
            .font_family(".SystemUIFont")
            .child(div().flex_shrink_0().pr_1().text_color(theme.foreground_muted).child("TERMINAL"))
            .child(
                div()
                    .id("terminal-tabs")
                    .flex()
                    .flex_row()
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(
                header_button(("term-new", 0), "+")
                    .tooltip(crate::tooltip::Tooltip::text("New terminal", theme))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_terminal(cx);
                        this.focus_terminal(window, cx);
                    })),
            )
            .child(div().flex_1())
            .when(scrolled_up, |h| {
                h.child(header_button(("term-latest", 0), "↓ Latest").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(view) = this.active_terminal().cloned() {
                        view.update(cx, |t, cx| t.scroll_to_latest(cx));
                    }
                })))
            })
            .child(header_button(("term-clear", 0), "Clear").on_click(cx.listener(|this, _, _, cx| {
                if let Some(view) = this.active_terminal().cloned() {
                    view.update(cx, |t, cx| t.clear(cx));
                }
            })))
            .child(header_button(("term-restart", 0), "Restart").on_click(cx.listener(|this, _, _, cx| {
                if let Some(view) = this.active_terminal().cloned() {
                    view.update(cx, |t, cx| t.restart(cx));
                }
            })));

        let body_h = (height - HEADER_HEIGHT).max(0.0);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.surface)
            .child(header)
            .child(
                div()
                    .w_full()
                    .h(px(body_h))
                    .overflow_hidden()
                    .children(self.active_terminal().cloned()),
            )
    }
}
