// @okf-doc: /decisions/0002-crate-layout.md
#![forbid(unsafe_code)]
//! Terminal-free core of Fathomable.
//!
//! Everything that must be testable without a terminal lives here: the
//! document model, Markdown layout, annotations, sessions, and configuration.
//! This crate never depends on `ratatui`, `crossterm`, or `rmcp`.

pub mod agents;
pub mod annotations;
pub mod bond;
pub mod checkpoints;
pub mod clock;
pub mod config;
pub mod content;
pub mod context;
pub mod diff;
mod document;
pub mod editor;
pub mod follow;
pub mod highlight;
pub mod layout;
pub mod link;
pub mod picker;
pub mod reanchor;
pub mod seen;
pub mod session;
pub mod status;
pub mod theme;
pub mod tree;
pub mod vocabulary;
pub mod workspace;

pub use document::{Document, LoadError};
mod xdg;

pub use xdg::{APP_DIR, XdgDirs};
