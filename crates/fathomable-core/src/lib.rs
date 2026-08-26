// @okf-doc: /decisions/0002-crate-layout.md
#![forbid(unsafe_code)]
//! Terminal-free core of Fathomable.
//!
//! Everything that must be testable without a terminal lives here: the
//! document model, Markdown layout, annotations, sessions, and configuration.
//! This crate never depends on `ratatui`, `crossterm`, or `rmcp`.

pub mod config;
mod document;
pub mod layout;
pub mod theme;

pub use document::{Document, LoadError};
mod xdg;

pub use xdg::{APP_DIR, XdgDirs};
