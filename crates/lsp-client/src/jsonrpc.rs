//! LSP wire framing: `Content-Length: N\r\n\r\n<json>`, the same framing
//! every Language Server Protocol implementation uses over stdio.

use std::io::{BufRead, Write};

use anyhow::{Context, anyhow};
use serde_json::Value;

pub fn write_message<W: Write>(writer: &mut W, body: &Value) -> anyhow::Result<()> {
    let text = serde_json::to_string(body)?;
    write!(writer, "Content-Length: {}\r\n\r\n{}", text.len(), text)?;
    writer.flush()?;
    Ok(())
}

/// Reads one framed message, or `Ok(None)` on a clean EOF (the server
/// process exited) — never panics on a malformed frame, returns an error
/// instead so the caller can decide whether that's fatal.
pub fn read_message<R: BufRead>(reader: &mut R) -> anyhow::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, value)| value.trim())
        {
            content_length = value.parse::<usize>().ok();
        }
    }

    let len = content_length.ok_or_else(|| anyhow!("LSP frame missing Content-Length header"))?;
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .context("reading LSP message body")?;
    let value = serde_json::from_slice(&buf).context("parsing LSP message JSON")?;
    Ok(Some(value))
}
