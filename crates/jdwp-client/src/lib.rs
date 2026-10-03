//! A minimal Java Debug Wire Protocol (JDWP) client — attach to a JVM
//! started with `-agentlib:jdwp=...`, resume it, and set line breakpoints.
//! See `client.rs` for the scope this deliberately does and doesn't cover.

mod client;
mod consts;
mod cursor;
mod wire;

pub use client::{
    DebugEvent, Frame, IdSizes, JdwpClient, JdwpError, Location, StepDepth, Variable, jdwp_error_code,
    signature_to_class_name,
};

/// JDWP `EventKind` for breakpoints — needed to clear one.
pub const EVENT_KIND_BREAKPOINT: u8 = consts::event_kind::BREAKPOINT;
/// JDWP `EventKind` for single steps — needed to clear a finished step.
pub const EVENT_KIND_SINGLE_STEP: u8 = consts::event_kind::SINGLE_STEP;
