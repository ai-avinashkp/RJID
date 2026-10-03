//! Starts the default shell in a PTY and resizes it like the IDE's panel
//! does, printing the emulator screen after each step (to see where blank
//! rows appear).
//!
//! `cargo run -p rji-terminal --example resize_repro -- [start_rows] [rows...]`

use std::time::{Duration, Instant};

use rji_terminal::{Emulator, PtySession, default_shell};

fn pump(session: &mut PtySession, emu: &mut Emulator, for_ms: u64) {
    let until = Instant::now() + Duration::from_millis(for_ms);
    while Instant::now() < until {
        while let Ok(chunk) = session.output_rx.try_recv() {
            let reply = emu.feed(&chunk);
            if !reply.is_empty() {
                let _ = session.write_input(&reply);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn dump(label: &str, emu: &Emulator) {
    let screen = emu.screen();
    println!("--- {label}: {}x{} cursor={:?}", emu.columns(), emu.lines(), screen.cursor);
    for (i, row) in screen.rows.iter().enumerate() {
        let text: String = row.iter().map(|r| r.text.as_str()).collect();
        println!("{i:2}|{}", text.trim_end());
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let start = args.first().copied().unwrap_or(32);
    let cols = 100;
    let mut emu = Emulator::new(cols, start);
    let mut session = PtySession::spawn(&default_shell(), &[], &std::env::current_dir()?)?;
    session.resize(start as u16, cols as u16)?;
    if let Ok(early) = std::env::var("EARLY_ROWS") {
        // The panel measures itself right after the shell starts.
        for rows in early.split(',').filter_map(|r| r.parse::<usize>().ok()) {
            pump(&mut session, &mut emu, 120);
            emu.resize(cols, rows);
            session.resize(rows as u16, cols as u16)?;
        }
    }
    pump(&mut session, &mut emu, 7000);
    dump(&format!("start {start}"), &emu);
    for &rows in args.iter().skip(1) {
        emu.resize(cols, rows);
        session.resize(rows as u16, cols as u16)?;
        pump(&mut session, &mut emu, 2500);
        dump(&format!("resized to {rows}"), &emu);
    }
    let _ = session.kill();
    Ok(())
}
