//! Raw JDWP packet framing over a TCP socket — the wire-level foundation
//! everything else in this crate builds on. See the [JDWP spec's protocol
//! overview](https://docs.oracle.com/javase/8/docs/platform/jpda/jdwp/jdwp-protocol.html)
//! for the exact byte layout this mirrors.

use std::io::{Read, Write};
use std::net::TcpStream;

use anyhow::{Context, bail, ensure};

const HANDSHAKE: &[u8] = b"JDWP-Handshake";
/// Set on a packet's flags byte to mark it as a reply rather than a command.
pub const FLAG_REPLY: u8 = 0x80;

/// One JDWP packet, command or reply. For a command: `error_or_command_set`
/// holds the command set and `command` the command id. For a reply:
/// `error_or_command_set` holds the low byte of the error code (0 = no
/// error) and `command` is unused (0) — JDWP's error code is 2 bytes, but in
/// practice every defined error code fits in one byte, and callers that care
/// about the exact code read `error_code` directly instead.
#[derive(Debug, Clone)]
pub struct Packet {
    pub id: i32,
    pub flags: u8,
    pub error_code: u16,
    pub command_set: u8,
    pub command: u8,
    pub data: Vec<u8>,
}

impl Packet {
    pub fn is_reply(&self) -> bool {
        self.flags & FLAG_REPLY != 0
    }

    pub fn command(id: i32, command_set: u8, command: u8, data: Vec<u8>) -> Self {
        Packet {
            id,
            flags: 0,
            error_code: 0,
            command_set,
            command,
            data,
        }
    }
}

/// Performs the JDWP handshake: send the literal ASCII string, expect it
/// echoed back exactly. Must happen before any packet traffic.
pub fn handshake(stream: &mut TcpStream) -> anyhow::Result<()> {
    stream
        .write_all(HANDSHAKE)
        .context("writing JDWP handshake")?;
    let mut reply = [0u8; 14];
    stream
        .read_exact(&mut reply)
        .context("reading JDWP handshake reply")?;
    ensure!(
        reply == *HANDSHAKE,
        "target did not echo the JDWP handshake string (got {:?})",
        String::from_utf8_lossy(&reply)
    );
    Ok(())
}

pub fn write_packet(stream: &mut TcpStream, packet: &Packet) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(11 + packet.data.len());
    let length = 11 + packet.data.len() as u32;
    buf.extend_from_slice(&length.to_be_bytes());
    buf.extend_from_slice(&packet.id.to_be_bytes());
    buf.push(packet.flags);
    if packet.is_reply() {
        buf.extend_from_slice(&packet.error_code.to_be_bytes());
    } else {
        buf.push(packet.command_set);
        buf.push(packet.command);
    }
    buf.extend_from_slice(&packet.data);
    stream.write_all(&buf).context("writing JDWP packet")?;
    Ok(())
}

pub fn read_packet(stream: &mut TcpStream) -> anyhow::Result<Packet> {
    let mut header = [0u8; 11];
    stream
        .read_exact(&mut header)
        .context("reading JDWP packet header")?;
    let length = u32::from_be_bytes(header[0..4].try_into().unwrap());
    let id = i32::from_be_bytes(header[4..8].try_into().unwrap());
    let flags = header[8];
    bail_if_short(length)?;
    let data_len = length as usize - 11;
    let mut data = vec![0u8; data_len];
    if data_len > 0 {
        stream
            .read_exact(&mut data)
            .context("reading JDWP packet body")?;
    }

    if flags & FLAG_REPLY != 0 {
        let error_code = u16::from_be_bytes(header[9..11].try_into().unwrap());
        Ok(Packet {
            id,
            flags,
            error_code,
            command_set: 0,
            command: 0,
            data,
        })
    } else {
        Ok(Packet {
            id,
            flags,
            error_code: 0,
            command_set: header[9],
            command: header[10],
            data,
        })
    }
}

fn bail_if_short(length: u32) -> anyhow::Result<()> {
    if length < 11 {
        bail!("JDWP packet length {length} is smaller than the 11-byte header");
    }
    Ok(())
}
