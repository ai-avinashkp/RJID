//! JDWP command-set/command numbers and event/suspend-policy constants used
//! by this client. Not exhaustive — only what's needed for attach, resume,
//! class-prepare-triggered breakpoints, and reporting a breakpoint hit's
//! location. See the JDWP spec for the full command set.
//!
//! A few constants here (`SUSPEND`, `SOURCE_FILE`, `FRAMES`, suspend-policy
//! `NONE`/`ALL`) aren't called from this crate's first slice yet — pause,
//! source-file cross-checking, and stack traces are the next layer on top
//! of attach/resume/breakpoint, kept here now since they're part of the
//! same reference table rather than scattered in as each lands.
#![allow(dead_code)]

pub mod cmdset {
    pub const VIRTUAL_MACHINE: u8 = 1;
    pub const REFERENCE_TYPE: u8 = 2;
    pub const METHOD: u8 = 6;
    pub const STRING_REFERENCE: u8 = 10;
    pub const THREAD_REFERENCE: u8 = 11;
    pub const ARRAY_REFERENCE: u8 = 13;
    pub const EVENT_REQUEST: u8 = 15;
    pub const STACK_FRAME: u8 = 16;
    pub const EVENT: u8 = 64;
}

pub mod vm_cmd {
    pub const CLASSES_BY_SIGNATURE: u8 = 2;
    pub const ID_SIZES: u8 = 7;
    pub const SUSPEND: u8 = 8;
    pub const RESUME: u8 = 9;
    pub const EXIT: u8 = 10;
}

pub mod reftype_cmd {
    pub const SIGNATURE: u8 = 1;
    pub const METHODS: u8 = 5;
    pub const SOURCE_FILE: u8 = 7;
}

pub mod method_cmd {
    pub const LINE_TABLE: u8 = 1;
    pub const VARIABLE_TABLE: u8 = 2;
}

pub mod string_cmd {
    pub const VALUE: u8 = 1;
}

pub mod array_cmd {
    pub const LENGTH: u8 = 1;
}

pub mod frame_cmd {
    pub const GET_VALUES: u8 = 1;
}

/// JDWP error codes this client reacts to specifically.
pub mod error {
    /// No debug info (e.g. a class compiled without `-g` has no local
    /// variable table).
    pub const ABSENT_INFORMATION: u16 = 101;
}

pub mod thread_cmd {
    pub const NAME: u8 = 1;
    pub const FRAMES: u8 = 6;
}

pub mod event_request_cmd {
    pub const SET: u8 = 1;
    pub const CLEAR: u8 = 2;
}

pub mod event_cmd {
    pub const COMPOSITE: u8 = 100;
}

/// `EventKind` values from the JDWP spec — only the ones this client sets
/// requests for or expects to see in a `Composite` event.
pub mod event_kind {
    pub const SINGLE_STEP: u8 = 1;
    pub const BREAKPOINT: u8 = 2;
    pub const CLASS_PREPARE: u8 = 8;
    pub const VM_START: u8 = 90;
    pub const VM_DEATH: u8 = 99;
}

pub mod suspend_policy {
    pub const NONE: u8 = 0;
    pub const EVENT_THREAD: u8 = 1;
    pub const ALL: u8 = 2;
}

/// `EventRequest.Set` modifier kinds used by this client.
pub mod mod_kind {
    /// Report the event only once, then expire the request.
    pub const COUNT: u8 = 1;
    /// Single-step a thread by size/depth.
    pub const STEP: u8 = 10;
    /// Restricts an event request to classes whose name matches a pattern
    /// (`*` at either end only, per JDWP).
    pub const CLASS_MATCH: u8 = 5;
    /// Skip events in classes matching a pattern.
    pub const CLASS_EXCLUDE: u8 = 6;
    /// Restricts a `Breakpoint` request to one exact bytecode location.
    pub const LOCATION_ONLY: u8 = 7;
}
