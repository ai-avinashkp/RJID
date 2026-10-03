mod completion;
mod debug_session;
mod devtools_hook;
mod docking;
mod editor_view;
mod fonts;
mod lsp_shared;
mod root;
mod scrollbar;
mod terminal_view;
mod text_input;
mod tooltip;

use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_platform::application;

use root::RootView;

/// Installs an IDE update downloaded last session (already signature- and
/// checksum-verified), before any window opens; relaunches into it.
fn apply_pending_update() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match rji_updates::apply_staged(&exe) {
        Ok(true) => {
            let _ = std::process::Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .spawn();
            std::process::exit(0);
        }
        Ok(false) => rji_updates::cleanup_previous(&exe),
        Err(err) => eprintln!("rji: couldn't install the downloaded update: {err:#}"),
    }
}

fn main() {
    apply_pending_update();
    application().run(|cx: &mut App| {
        // 1280×820 when the screen has room, else 90% of it — a window
        // bigger than the work area gets treated as maximized by Windows.
        let (width, height) = cx
            .primary_display()
            .map(|display| {
                let screen = display.bounds().size;
                (
                    f32::from(screen.width * 0.9).min(1280.0),
                    f32::from(screen.height * 0.9).min(820.0),
                )
            })
            .unwrap_or((1280.0, 820.0));
        let bounds = Bounds::centered(None, size(px(width.max(400.0)), px(height.max(500.0))), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // The app draws its own title bar (menus + window
                    // buttons; see `root/titlebar.rs`).
                    titlebar: Some(TitlebarOptions {
                        title: Some("RJID".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_min_size: Some(size(px(400.0), px(500.0))),
                    ..Default::default()
                },
                |_, cx| cx.new(RootView::new),
            )
            .unwrap();

        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle_for_init(), cx);
                view.open_command_line_file(window, cx);
                // Closing the window first offers to save unsaved files and
                // stops child processes (jdtls, debugged program, shell).
                let root = cx.entity();
                window.on_window_should_close(cx, move |window, cx| {
                    root.update(cx, |root, cx| root.request_close(window, cx))
                });
                cx.activate(true);
            })
            .ok();

        // Single-window app: quit once it's gone.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
