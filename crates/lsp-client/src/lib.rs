//! Minimal LSP client for Eclipse JDT Language Server (jdtls): process
//! spawn, the `initialize` handshake, document sync notifications,
//! `publishDiagnostics`, completion, hover, and the edit/code-action
//! shapes used for quick fixes, imports and code generation.

mod client;
mod discover;
pub mod edits;
mod jsonrpc;

pub use client::{CompletionItem, Diagnostic, DiagnosticSeverity, LspClient, PublishDiagnostics, ServerStatus, parse_server_status, path_to_uri, uri_to_path};
pub use discover::{JdtlsInstall, discover_jdtls, discover_jdtls_with, is_server_dir};
pub use edits::{CodeAction, FileEdit, TextEdit, parse_code_actions, parse_text_edits, parse_workspace_edit};
