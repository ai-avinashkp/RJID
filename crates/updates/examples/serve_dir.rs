//! Minimal static file server for testing self-update locally:
//! `cargo run -p rji-updates --example serve_dir -- <dir> <port>`
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("dir"));
    let port = args.next().unwrap_or_else(|| "8765".into());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("bind");
    println!("serving {} on 127.0.0.1:{port}", dir.display());
    for stream in listener.incoming().flatten() {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).map_or(true, |n| n <= 2) {
                break;
            }
        }
        let name = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
        let mut stream = stream;
        // Plain file names only — no path traversal.
        match (!name.contains(['/', '\\']) && !name.contains("..")).then(|| std::fs::read(dir.join(name))) {
            Some(Ok(body)) => {
                println!("200 {name} ({} bytes)", body.len());
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
            _ => {
                println!("404 {name}");
                let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        }
    }
}
