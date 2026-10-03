//! A jdtls connection is shared between `RootView` (which owns its
//! lifecycle: spawning, retroactive `didOpen` for already-open tabs,
//! `didClose` on tab close) and every open `CodeEditorView` (which calls
//! `didChange` directly on edits, without round-tripping through RootView).
//! `Rc<RefCell<>>` is fine here — GPUI entity updates all run on the single
//! UI thread; the one truly cross-thread hop (spawning jdtls, which blocks
//! for the `initialize` handshake) happens entirely inside
//! `rji_lsp_client::LspClient::spawn` on a background task before the
//! result ever reaches this shared cell.

use std::cell::RefCell;
use std::rc::Rc;

use rji_lsp_client::LspClient;

pub type SharedLspClient = Rc<RefCell<Option<LspClient>>>;
