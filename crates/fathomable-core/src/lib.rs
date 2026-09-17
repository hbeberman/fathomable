// @okf-doc: /decisions/0002-crate-layout.md
#![forbid(unsafe_code)]
//! Terminal-free core of Fathomable.
//!
//! Everything that must be testable without a terminal lives here: the
//! document model, Markdown layout, annotations, sessions, and configuration.
//! This crate never depends on `ratatui`, `crossterm`, or `rmcp`.

pub mod annotations;
pub mod clock;
pub mod config;
pub mod content;
pub mod context;
pub mod diff;
mod document;
pub mod editor;
pub mod follow;
pub mod highlight;
pub mod identity;
pub mod layout;
pub mod link;
pub mod picker;
pub mod reach;
pub mod reanchor;
pub mod review_points;
pub mod session;
pub mod status;
pub mod theme;
pub mod tree;
pub mod vocabulary;
pub mod workspace;
pub mod worktrees;

#[doc(inline)]
pub use document::{Document, LoadError};
mod xdg;

#[doc(inline)]
pub use xdg::{SOCKET_PATH_MAX, XdgDirs, socket_path_fits};
