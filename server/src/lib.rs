//! `cmantic-ls`: C-Mantic style C/C++ refactorings as a language server.
//!
//! The refactoring logic (`actions`, `placement`, `symbol`) works on plain text and is
//! independent of the LSP plumbing in `server`.

pub mod actions;
pub mod config;
pub mod document;
pub mod edit_buffer;
pub mod header_source;
pub mod placement;
pub mod server;
pub mod symbol;
pub mod text;
