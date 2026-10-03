//! A real PTY-backed shell session (portable-pty), decoupled from any GPUI
//! rendering so it can be unit-tested and reused for both the interactive
//! terminal panel and future one-shot task output (Maven/Gradle/adb).

use std::io::{Read, Write};
use std::path::Path;

use async_channel::{Receiver, Sender, unbounded};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

/// The shell to launch when the user hasn't configured one explicitly.
pub fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

/// A running shell/process backed by a real PTY. Output bytes arrive on
/// `output_rx` as they're produced by a background reader thread; send
/// keystrokes/commands via [`PtySession::write_input`].
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub output_rx: Receiver<Vec<u8>>,
}

impl PtySession {
    /// Spawns `command` (e.g. `default_shell()`, or `mvn.cmd spring-boot:run`)
    /// in `cwd` with a real PTY.
    pub fn spawn(command: &str, args: &[&str], cwd: &Path) -> anyhow::Result<PtySession> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        cmd.args(args);
        cmd.cwd(cwd);

        let child = pair.slave.spawn_command(cmd)?;
        // Drop our copy of the slave so the reader sees EOF once the child
        // (and any of its own children) truly exit.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = unbounded();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send_blocking(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(PtySession {
            master: pair.master,
            writer,
            child,
            output_rx: rx,
        })
    }

    /// Sends raw input bytes (keystrokes) to the shell. Never panics on a
    /// write failure (e.g. the shell already exited) — the caller can keep
    /// draining `output_rx` for the exit message instead.
    pub fn write_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resizes the PTY to match the panel's current size in terminal cells —
    /// keeps shell-side line wrapping sane when the panel is resized.
    pub fn resize(&self, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.child.kill()?;
        Ok(())
    }
}

impl Drop for PtySession {
    /// Replacing the terminal (opening another folder) or closing the app
    /// must not leave the old shell running in the background.
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
