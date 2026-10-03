//! Little helpers for building JDWP command data and parsing reply/event
//! data — everything in the wire format is big-endian, fixed-width ints
//! plus JDWP "object ID" fields whose byte width is only known at runtime
//! (from `VirtualMachine.IDSizes`), so a plain `byteorder`-style reader
//! isn't quite enough on its own.

use anyhow::{Context, ensure};

pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Writes an ID whose byte width depends on the target VM
    /// (`VirtualMachine.IDSizes`) — object/thread/reference-type/etc IDs are
    /// all this shape. `width` is bytes (4 or 8 in practice).
    pub fn id(&mut self, value: u64, width: u8) -> &mut Self {
        let bytes = value.to_be_bytes();
        self.buf.extend_from_slice(&bytes[8 - width as usize..]);
        self
    }

    pub fn utf8(&mut self, s: &str) -> &mut Self {
        self.i32(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        ensure!(
            self.pos + n <= self.data.len(),
            "JDWP reply/event truncated: wanted {n} more bytes at offset {}, have {}",
            self.pos,
            self.data.len()
        );
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    #[allow(dead_code)]
    pub fn u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> anyhow::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Reads an ID of the given byte width (see `Writer::id`), zero-extended
    /// into a `u64` for convenient storage/comparison regardless of the
    /// target VM's actual ID size.
    pub fn id(&mut self, width: u8) -> anyhow::Result<u64> {
        let bytes = self.take(width as usize)?;
        let mut out = [0u8; 8];
        out[8 - width as usize..].copy_from_slice(bytes);
        Ok(u64::from_be_bytes(out))
    }

    pub fn utf8(&mut self) -> anyhow::Result<String> {
        let len = self.i32().context("reading utf8 string length")? as usize;
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
}
