//! A minimal, synchronous JDWP client: connect, resolve ID sizes, resume
//! the (suspended-at-VMStart) target, set breakpoints via
//! `ClassPrepare`-then-`LineTable` resolution, and report where the target
//! stopped. Deliberately not a general-purpose JDI-equivalent — no full
//! variable/object inspection yet (see `docs/ROADMAP.md`), just enough to
//! attach, break, and locate.
//!
//! Modeled on `rji_lsp_client::LspClient`'s shape: a background thread reads
//! every inbound packet and routes it either to a reply-matching channel
//! (for our own outstanding requests) or to an events channel (for
//! unsolicited `Event.Composite` packets the target VM sends us) — the
//! caller only ever calls blocking, synchronous methods, matching the rest
//! of this codebase's "one shared client driven from one place" posture
//! (see `lsp_shared.rs` in `crates/app`).

use std::net::TcpStream;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// `IdSizes` shared between the caller (which learns it during `attach`)
/// and the background reader thread (which needs it to parse `ClassPrepare`
/// / `Breakpoint` event data — both contain ID-width-dependent fields).
/// `None` until the initial `IDSizes` request completes; the only event
/// that can arrive before then is `VMStart`, whose sole field is inferred
/// from the packet's byte length instead (see `parse_composite`).
type SharedIdSizes = Arc<Mutex<Option<IdSizes>>>;

use anyhow::{Context, anyhow};
use async_channel::{Receiver, Sender};

use crate::consts::{
    array_cmd, cmdset, error, event_cmd, event_kind, event_request_cmd, frame_cmd, method_cmd,
    mod_kind, reftype_cmd, string_cmd, suspend_policy, thread_cmd, vm_cmd,
};

/// A command the target VM answered with a JDWP error code.
#[derive(Debug, Clone, Copy)]
pub struct JdwpError(pub u16);

impl std::fmt::Display for JdwpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JDWP error code {}", self.0)
    }
}

impl std::error::Error for JdwpError {}

/// JDWP `StepDepth` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDepth {
    Into = 0,
    Over = 1,
    Out = 2,
}

/// One stack frame of a suspended thread.
#[derive(Debug, Clone)]
pub struct Frame {
    pub frame_id: u64,
    pub location: Location,
}

/// A local variable's name and a display string for its current value.
#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub type_name: String,
    pub value: String,
}
use crate::cursor::{Reader, Writer};
use crate::wire::{Packet, handshake, read_packet, write_packet};

/// Byte widths of the various opaque IDs JDWP uses, as reported by the
/// target VM itself (`VirtualMachine.IDSizes`) — never assumed, since the
/// spec explicitly allows these to vary by VM/platform.
#[derive(Debug, Clone, Copy)]
pub struct IdSizes {
    pub field_id: u8,
    pub method_id: u8,
    pub object_id: u8,
    pub reference_type_id: u8,
    pub frame_id: u8,
}

#[derive(Debug, Clone)]
pub struct Location {
    pub class_id: u64,
    pub method_id: u64,
    pub index: u64,
}

#[derive(Debug, Clone)]
pub enum DebugEvent {
    VmStart,
    VmDeath,
    /// A class matching a `ClassPrepare` filter finished loading — carries
    /// the reference type ID needed to resolve breakpoints in it.
    ClassPrepare { reference_type_id: u64 },
    Breakpoint { thread_id: u64, location: Location },
    /// A single-step request (see `step_over`) completed.
    Step {
        thread_id: u64,
        location: Location,
        request_id: i32,
    },
}

pub struct JdwpClient {
    stream_write: Mutex<TcpStream>,
    next_id: AtomicI32,
    responses_rx: Receiver<Packet>,
    pub events_rx: Receiver<DebugEvent>,
    pub id_sizes: IdSizes,
}

impl JdwpClient {
    /// Connects to a target VM already listening for a debugger (i.e.
    /// started with `-agentlib:jdwp=transport=dt_socket,server=y,...`),
    /// performs the handshake, and resolves `IdSizes` — everything a caller
    /// needs before issuing any other command.
    pub fn attach(addr: &str) -> anyhow::Result<Self> {
        let mut stream = TcpStream::connect(addr)
            .with_context(|| format!("connecting to JDWP target at {addr}"))?;
        handshake(&mut stream).context("JDWP handshake")?;

        let reader_stream = stream.try_clone().context("cloning JDWP socket")?;
        let (responses_tx, responses_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let shared_id_sizes: SharedIdSizes = Arc::new(Mutex::new(None));
        spawn_reader_thread(reader_stream, responses_tx, events_tx, shared_id_sizes.clone());

        let mut client = JdwpClient {
            stream_write: Mutex::new(stream),
            next_id: AtomicI32::new(1),
            responses_rx,
            events_rx,
            // Placeholder, overwritten immediately below — IDSizes itself
            // doesn't need `id_sizes` to be known (it has no ID fields).
            id_sizes: IdSizes {
                field_id: 8,
                method_id: 8,
                object_id: 8,
                reference_type_id: 8,
                frame_id: 8,
            },
        };
        client.id_sizes = client.fetch_id_sizes()?;
        *shared_id_sizes.lock().unwrap() = Some(client.id_sizes);
        Ok(client)
    }

    fn send(&self, command_set: u8, command: u8, data: Vec<u8>) -> anyhow::Result<Packet> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let packet = Packet::command(id, command_set, command, data);
        {
            let mut stream = self.stream_write.lock().unwrap();
            write_packet(&mut stream, &packet)?;
        }
        loop {
            let reply = self
                .responses_rx
                .recv_blocking()
                .map_err(|_| anyhow!("JDWP connection closed while waiting for a reply"))?;
            if reply.id == id {
                if reply.error_code != 0 {
                    return Err(anyhow::Error::new(JdwpError(reply.error_code)).context(format!(
                        "JDWP command (set {command_set}, cmd {command}) failed"
                    )));
                }
                return Ok(reply);
            }
            // A reply to some earlier, already-abandoned request (shouldn't
            // happen given this client only ever has one in-flight request
            // at a time) — drop it rather than block forever on a mismatch.
        }
    }

    fn fetch_id_sizes(&self) -> anyhow::Result<IdSizes> {
        let reply = self.send(cmdset::VIRTUAL_MACHINE, vm_cmd::ID_SIZES, Vec::new())?;
        let mut r = Reader::new(&reply.data);
        Ok(IdSizes {
            field_id: r.i32()? as u8,
            method_id: r.i32()? as u8,
            object_id: r.i32()? as u8,
            reference_type_id: r.i32()? as u8,
            frame_id: r.i32()? as u8,
        })
    }

    /// Resumes every thread in the target VM — needed once at startup to
    /// un-suspend the initial thread (the VM starts suspended-at-`VMStart`
    /// under `suspend=y`), and again after handling any suspending event.
    pub fn resume(&self) -> anyhow::Result<()> {
        self.send(cmdset::VIRTUAL_MACHINE, vm_cmd::RESUME, Vec::new())?;
        Ok(())
    }

    /// Requests a `ClassPrepare` event for classes whose name matches
    /// `class_pattern` (JDWP allows a leading/trailing `*` wildcard, e.g.
    /// `com.example.*`), suspending only the thread that triggers it —
    /// the first step of setting a breakpoint in a class that may not be
    /// loaded yet.
    pub fn set_class_prepare_request(&self, class_pattern: &str) -> anyhow::Result<i32> {
        let mut w = Writer::new();
        w.u8(event_kind::CLASS_PREPARE)
            .u8(suspend_policy::EVENT_THREAD)
            .i32(1)
            .u8(mod_kind::CLASS_MATCH)
            .utf8(class_pattern);
        let reply = self.send(cmdset::EVENT_REQUEST, event_request_cmd::SET, w.buf)?;
        let mut r = Reader::new(&reply.data);
        r.i32()
    }

    /// Looks up the line table for every method on `reference_type_id` and
    /// returns the first `(method_id, code_index)` whose line table covers
    /// `line` — the bytecode location a `Breakpoint` event request needs.
    /// `None` if no method on this type covers that line (e.g. the line is
    /// blank/a comment, or belongs to a different class in the same file).
    pub fn resolve_line(
        &self,
        reference_type_id: u64,
        line: u32,
    ) -> anyhow::Result<Option<(u64, u64)>> {
        let mut w = Writer::new();
        w.id(reference_type_id, self.id_sizes.reference_type_id);
        let reply = self.send(cmdset::REFERENCE_TYPE, reftype_cmd::METHODS, w.buf)?;
        let mut r = Reader::new(&reply.data);
        let method_count = r.i32()?;
        let mut method_ids = Vec::with_capacity(method_count.max(0) as usize);
        for _ in 0..method_count {
            let method_id = r.id(self.id_sizes.method_id)?;
            let _name = r.utf8()?;
            let _signature = r.utf8()?;
            let _mod_bits = r.i32()?;
            method_ids.push(method_id);
        }

        for method_id in method_ids {
            let mut w = Writer::new();
            w.id(reference_type_id, self.id_sizes.reference_type_id)
                .id(method_id, self.id_sizes.method_id);
            let reply = self.send(cmdset::METHOD, method_cmd::LINE_TABLE, w.buf)?;
            let mut r = Reader::new(&reply.data);
            let _start = r.u64()?;
            let _end = r.u64()?;
            let line_count = r.i32()?;
            let mut best: Option<u64> = None;
            for _ in 0..line_count {
                let line_code_index = r.u64()?;
                let line_number = r.i32()? as u32;
                if line_number == line {
                    best = Some(line_code_index);
                    break;
                }
            }
            if let Some(code_index) = best {
                return Ok(Some((method_id, code_index)));
            }
        }
        Ok(None)
    }

    /// Sets a `Breakpoint` event request at an exact, already-resolved
    /// bytecode location (see `resolve_line`). Suspends only the thread
    /// that hits it, matching typical IDE breakpoint behavior (other
    /// threads keep running).
    pub fn set_breakpoint(&self, location: &Location) -> anyhow::Result<i32> {
        let mut w = Writer::new();
        w.u8(event_kind::BREAKPOINT)
            .u8(suspend_policy::EVENT_THREAD)
            .i32(1)
            .u8(mod_kind::LOCATION_ONLY)
            .u8(1) // type tag: 1 = class (vs. interface/array) — we only target classes
            .id(location.class_id, self.id_sizes.reference_type_id)
            .id(location.method_id, self.id_sizes.method_id)
            .u64(location.index);
        let reply = self.send(cmdset::EVENT_REQUEST, event_request_cmd::SET, w.buf)?;
        let mut r = Reader::new(&reply.data);
        r.i32()
    }

    pub fn clear_event_request(&self, event_kind: u8, request_id: i32) -> anyhow::Result<()> {
        let mut w = Writer::new();
        w.u8(event_kind).i32(request_id);
        self.send(cmdset::EVENT_REQUEST, event_request_cmd::CLEAR, w.buf)?;
        Ok(())
    }

    pub fn thread_name(&self, thread_id: u64) -> anyhow::Result<String> {
        let mut w = Writer::new();
        w.id(thread_id, self.id_sizes.object_id);
        let reply = self.send(cmdset::THREAD_REFERENCE, thread_cmd::NAME, w.buf)?;
        let mut r = Reader::new(&reply.data);
        r.utf8()
    }

    /// Terminates the target VM (the debugger's "Stop").
    pub fn exit(&self, exit_code: i32) -> anyhow::Result<()> {
        let mut w = Writer::new();
        w.i32(exit_code);
        self.send(cmdset::VIRTUAL_MACHINE, vm_cmd::EXIT, w.buf)?;
        Ok(())
    }

    /// JNI-style signature of a loaded type, e.g. `Lcom/example/App;`.
    pub fn class_signature(&self, reference_type_id: u64) -> anyhow::Result<String> {
        let mut w = Writer::new();
        w.id(reference_type_id, self.id_sizes.reference_type_id);
        let reply = self.send(cmdset::REFERENCE_TYPE, reftype_cmd::SIGNATURE, w.buf)?;
        Reader::new(&reply.data).utf8()
    }

    /// Reference type IDs of already-loaded classes with this JNI
    /// signature (usually zero or one).
    pub fn classes_by_signature(&self, signature: &str) -> anyhow::Result<Vec<u64>> {
        let mut w = Writer::new();
        w.utf8(signature);
        let reply = self.send(cmdset::VIRTUAL_MACHINE, vm_cmd::CLASSES_BY_SIGNATURE, w.buf)?;
        let mut r = Reader::new(&reply.data);
        let count = r.i32()?;
        let mut ids = Vec::new();
        for _ in 0..count {
            let _tag = r.u8()?;
            ids.push(r.id(self.id_sizes.reference_type_id)?);
            let _status = r.i32()?;
        }
        Ok(ids)
    }

    /// Name of a method on a class (e.g. `main`).
    pub fn method_name(&self, class_id: u64, method_id: u64) -> anyhow::Result<String> {
        let mut w = Writer::new();
        w.id(class_id, self.id_sizes.reference_type_id);
        let reply = self.send(cmdset::REFERENCE_TYPE, reftype_cmd::METHODS, w.buf)?;
        let mut r = Reader::new(&reply.data);
        for _ in 0..r.i32()? {
            let id = r.id(self.id_sizes.method_id)?;
            let name = r.utf8()?;
            let _signature = r.utf8()?;
            let _mod_bits = r.i32()?;
            if id == method_id {
                return Ok(name);
            }
        }
        Ok("?".to_string())
    }

    /// Source line (1-based, as `javac` records it) for a bytecode
    /// location: the line-table entry with the greatest code index not
    /// after `location.index`. `None` for methods without line info.
    pub fn line_for_location(&self, location: &Location) -> anyhow::Result<Option<u32>> {
        let mut w = Writer::new();
        w.id(location.class_id, self.id_sizes.reference_type_id)
            .id(location.method_id, self.id_sizes.method_id);
        let reply = match self.send(cmdset::METHOD, method_cmd::LINE_TABLE, w.buf) {
            Ok(reply) => reply,
            Err(err) if jdwp_error_code(&err).is_some() => return Ok(None), // native/abstract
            Err(err) => return Err(err),
        };
        let mut r = Reader::new(&reply.data);
        let _start = r.u64()?;
        let _end = r.u64()?;
        let mut best: Option<(u64, u32)> = None;
        for _ in 0..r.i32()? {
            let code_index = r.u64()?;
            let line = r.i32()? as u32;
            if code_index <= location.index && best.is_none_or(|(b, _)| code_index >= b) {
                best = Some((code_index, line));
            }
        }
        Ok(best.map(|(_, line)| line))
    }

    /// Call stack of a suspended thread, innermost frame first.
    pub fn frames(&self, thread_id: u64, max: i32) -> anyhow::Result<Vec<Frame>> {
        let mut w = Writer::new();
        w.id(thread_id, self.id_sizes.object_id).i32(0).i32(max);
        let reply = match self.send(cmdset::THREAD_REFERENCE, thread_cmd::FRAMES, w.buf) {
            Ok(reply) => reply,
            // Fewer frames than `max` exist: ask for all of them instead.
            Err(err) if jdwp_error_code(&err).is_some() && max != -1 => {
                return self.frames(thread_id, -1);
            }
            Err(err) => return Err(err),
        };
        let mut r = Reader::new(&reply.data);
        let mut frames = Vec::new();
        for _ in 0..r.i32()? {
            let frame_id = r.id(self.id_sizes.frame_id)?;
            let _tag = r.u8()?;
            let class_id = r.id(self.id_sizes.reference_type_id)?;
            let method_id = r.id(self.id_sizes.method_id)?;
            let index = r.u64()?;
            frames.push(Frame {
                frame_id,
                location: Location {
                    class_id,
                    method_id,
                    index,
                },
            });
        }
        Ok(frames)
    }

    /// Local variables in scope at `frame`, with display values. `None`
    /// when the class has no local-variable debug info (compiled without
    /// `javac -g`; Maven and Gradle include it by default).
    pub fn local_variables(&self, thread_id: u64, frame: &Frame) -> anyhow::Result<Option<Vec<Variable>>> {
        let loc = &frame.location;
        let mut w = Writer::new();
        w.id(loc.class_id, self.id_sizes.reference_type_id)
            .id(loc.method_id, self.id_sizes.method_id);
        let reply = match self.send(cmdset::METHOD, method_cmd::VARIABLE_TABLE, w.buf) {
            Ok(reply) => reply,
            Err(err) if jdwp_error_code(&err) == Some(error::ABSENT_INFORMATION) => return Ok(None),
            Err(err) => return Err(err),
        };
        let mut r = Reader::new(&reply.data);
        let _arg_count = r.i32()?;
        // (slot, name, signature) of each variable live at this location.
        let mut visible: Vec<(i32, String, String)> = Vec::new();
        for _ in 0..r.i32()? {
            let code_index = r.u64()?;
            let name = r.utf8()?;
            let signature = r.utf8()?;
            let length = r.i32()? as u64;
            let slot = r.i32()?;
            if code_index <= loc.index && loc.index < code_index + length {
                visible.push((slot, name, signature));
            }
        }
        visible.sort_by_key(|(slot, _, _)| *slot);
        if visible.is_empty() {
            return Ok(Some(Vec::new()));
        }

        let mut w = Writer::new();
        w.id(thread_id, self.id_sizes.object_id)
            .id(frame.frame_id, self.id_sizes.frame_id)
            .i32(visible.len() as i32);
        for (slot, _, signature) in &visible {
            w.i32(*slot).u8(signature.as_bytes().first().copied().unwrap_or(b'L'));
        }
        let reply = self.send(cmdset::STACK_FRAME, frame_cmd::GET_VALUES, w.buf)?;
        let mut r = Reader::new(&reply.data);
        let count = r.i32()? as usize;
        let mut variables = Vec::with_capacity(count);
        for (_, name, signature) in visible.into_iter().take(count) {
            let value = self.read_tagged_value(&mut r)?;
            variables.push(Variable {
                name,
                type_name: display_type(&signature),
                value,
            });
        }
        Ok(Some(variables))
    }

    /// Requests a single "step over" on a suspended thread; completion
    /// arrives as a `DebugEvent::Step`. The caller resumes the VM after.
    pub fn step_over(&self, thread_id: u64) -> anyhow::Result<i32> {
        self.step(thread_id, StepDepth::Over)
    }

    /// Requests one line-sized step of the given depth (into calls, over
    /// them, or out of the current method).
    pub fn step(&self, thread_id: u64, depth: StepDepth) -> anyhow::Result<i32> {
        const STEP_SIZE_LINE: i32 = 1;
        // Step Into would otherwise stop inside JDK internals (e.g.
        // `PrintStream.println`); like other IDEs, skip platform classes.
        const SKIPPED: &[&str] = &["java.*", "javax.*", "jdk.*", "sun.*", "com.sun.*"];
        let excludes: &[&str] = if depth == StepDepth::Into { SKIPPED } else { &[] };
        let mut w = Writer::new();
        w.u8(event_kind::SINGLE_STEP)
            .u8(suspend_policy::EVENT_THREAD)
            .i32(2 + excludes.len() as i32)
            .u8(mod_kind::STEP)
            .id(thread_id, self.id_sizes.object_id)
            .i32(STEP_SIZE_LINE)
            .i32(depth as i32);
        for pattern in excludes {
            w.u8(mod_kind::CLASS_EXCLUDE).utf8(pattern);
        }
        w.u8(mod_kind::COUNT).i32(1);
        let reply = self.send(cmdset::EVENT_REQUEST, event_request_cmd::SET, w.buf)?;
        Reader::new(&reply.data).i32()
    }

    /// Reads one JDWP tagged value and renders it for display.
    fn read_tagged_value(&self, r: &mut Reader) -> anyhow::Result<String> {
        let tag = r.u8()?;
        Ok(match tag {
            b'Z' => (r.u8()? != 0).to_string(),
            b'B' => (r.u8()? as i8).to_string(),
            b'C' => {
                let code = u16::from_be_bytes([r.u8()?, r.u8()?]);
                format!("'{}'", char::from_u32(code as u32).unwrap_or('?'))
            }
            b'S' => (u16::from_be_bytes([r.u8()?, r.u8()?]) as i16).to_string(),
            b'I' => r.i32()?.to_string(),
            b'J' => (r.u64()? as i64).to_string(),
            b'F' => f32::from_bits(r.i32()? as u32).to_string(),
            b'D' => f64::from_bits(r.u64()?).to_string(),
            b'V' => "void".to_string(),
            _ => {
                let object_id = r.id(self.id_sizes.object_id)?;
                if object_id == 0 {
                    "null".to_string()
                } else if tag == b's' {
                    match self.string_value(object_id) {
                        Ok(s) => format!("\"{}\"", truncate_for_display(&s, 120)),
                        Err(_) => format!("String@{object_id}"),
                    }
                } else if tag == b'[' {
                    match self.array_length(object_id) {
                        Ok(len) => format!("array[{len}]"),
                        Err(_) => format!("array@{object_id}"),
                    }
                } else {
                    format!("@{object_id}")
                }
            }
        })
    }

    fn string_value(&self, object_id: u64) -> anyhow::Result<String> {
        let mut w = Writer::new();
        w.id(object_id, self.id_sizes.object_id);
        let reply = self.send(cmdset::STRING_REFERENCE, string_cmd::VALUE, w.buf)?;
        Reader::new(&reply.data).utf8()
    }

    fn array_length(&self, object_id: u64) -> anyhow::Result<i32> {
        let mut w = Writer::new();
        w.id(object_id, self.id_sizes.object_id);
        let reply = self.send(cmdset::ARRAY_REFERENCE, array_cmd::LENGTH, w.buf)?;
        Reader::new(&reply.data).i32()
    }
}

/// The JDWP error code behind a failed command, if that's why it failed.
pub fn jdwp_error_code(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<JdwpError>().map(|e| e.0)
}

/// `Lcom/example/App;` -> `com.example.App`; `[I` -> `int[]`; `I` -> `int`.
pub fn signature_to_class_name(signature: &str) -> String {
    if let Some(inner) = signature.strip_prefix('[') {
        return format!("{}[]", signature_to_class_name(inner));
    }
    match signature {
        "Z" => "boolean".into(),
        "B" => "byte".into(),
        "C" => "char".into(),
        "S" => "short".into(),
        "I" => "int".into(),
        "J" => "long".into(),
        "F" => "float".into(),
        "D" => "double".into(),
        "V" => "void".into(),
        s => s
            .strip_prefix('L')
            .and_then(|s| s.strip_suffix(';'))
            .unwrap_or(s)
            .replace('/', "."),
    }
}

/// Short display type: `Ljava/lang/String;` -> `String`.
fn display_type(signature: &str) -> String {
    let full = signature_to_class_name(signature);
    match full.rfind('.') {
        Some(dot) => full[dot + 1..].to_string(),
        None => full,
    }
}

fn truncate_for_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}…")
}

fn spawn_reader_thread(
    mut stream: TcpStream,
    responses_tx: Sender<Packet>,
    events_tx: Sender<DebugEvent>,
    shared_id_sizes: SharedIdSizes,
) {
    std::thread::spawn(move || {
        loop {
            let packet = match read_packet(&mut stream) {
                Ok(p) => p,
                Err(_) => break, // connection closed / target VM exited
            };
            if packet.is_reply() {
                if responses_tx.send_blocking(packet).is_err() {
                    break;
                }
                continue;
            }

            // The only command packets a debugger ever receives are
            // unsolicited `Event.Composite` from the target VM. Despite
            // JDWP's request/reply framing, HotSpot's transport does not
            // expect (and appears to choke on) a reply to these — sending
            // one here reliably desynced the target's own socket parser
            // during testing ("transport error 202: recv error"), so this
            // deliberately does *not* send one.
            if packet.command_set == cmdset::EVENT && packet.command == event_cmd::COMPOSITE {
                let id_sizes = *shared_id_sizes.lock().unwrap();
                for event in parse_composite(&packet.data, id_sizes) {
                    if events_tx.send_blocking(event).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// Parses an `Event.Composite` packet's data into zero or more
/// [`DebugEvent`]s — a single Composite packet can (and for suspend-all
/// breakpoints, does) carry several events at once. `id_sizes` is `None`
/// only in the brief window before the initial `IDSizes` request completes,
/// during which the only event that can arrive is `VMStart` (the target VM
/// sends it automatically on attach, before this client has requested
/// anything else) — its one field's width is inferred from the packet's
/// remaining byte length instead of a known `IdSizes` in that case.
fn parse_composite(data: &[u8], id_sizes: Option<IdSizes>) -> Vec<DebugEvent> {
    let mut out = Vec::new();
    let mut r = Reader::new(data);
    let Ok(_suspend_policy) = r.u8() else {
        return out;
    };
    let Ok(count) = r.i32() else { return out };
    for _ in 0..count {
        let Ok(kind) = r.u8() else { break };
        let Ok(request_id) = r.i32() else { break };
        match kind {
            k if k == event_kind::VM_START => {
                let width = id_sizes.map(|s| s.object_id).unwrap_or(r.remaining() as u8);
                let Ok(_thread_id) = r.id(width) else {
                    break;
                };
                out.push(DebugEvent::VmStart);
            }
            k if k == event_kind::VM_DEATH => {
                out.push(DebugEvent::VmDeath);
            }
            k if k == event_kind::CLASS_PREPARE => {
                let Some(sizes) = id_sizes else { break };
                let Ok(_thread_id) = r.id(sizes.object_id) else {
                    break;
                };
                let Ok(_ref_type_tag) = r.u8() else { break };
                let Ok(reference_type_id) = r.id(sizes.reference_type_id) else {
                    break;
                };
                let Ok(_signature) = r.utf8() else { break };
                let Ok(_status) = r.i32() else { break };
                out.push(DebugEvent::ClassPrepare { reference_type_id });
            }
            k if k == event_kind::BREAKPOINT || k == event_kind::SINGLE_STEP => {
                let Some(sizes) = id_sizes else { break };
                let Ok(thread_id) = r.id(sizes.object_id) else {
                    break;
                };
                let Ok(_type_tag) = r.u8() else { break };
                let Ok(class_id) = r.id(sizes.reference_type_id) else {
                    break;
                };
                let Ok(method_id) = r.id(sizes.method_id) else {
                    break;
                };
                let Ok(index) = r.u64() else { break };
                let location = Location {
                    class_id,
                    method_id,
                    index,
                };
                out.push(if kind == event_kind::SINGLE_STEP {
                    DebugEvent::Step {
                        thread_id,
                        location,
                        request_id,
                    }
                } else {
                    DebugEvent::Breakpoint {
                        thread_id,
                        location,
                    }
                });
            }
            _ => break,
        }
    }
    out
}
