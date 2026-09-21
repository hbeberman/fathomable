// @okf-doc: /decisions/0048-modules-by-concept.md
//! Test scaffolding shared by the Fathomable crates (ADR 0048).
//!
//! [`TempDir`] is a throwaway directory under the system temp dir, removed
//! when dropped; each test module writes its own fixture files into it.
//! The `git` functions build repositories the way the tests need them,
//! using only repository-local configuration, without personal/system
//! configuration or environment overrides.
//! The [`vocabulary`] module checks agent-facing prose against the
//! production vocabulary without adding those helpers to the runtime API.
//!
//! # Examples
//!
//! ```
//! use fathomable_testing::TempDir;
//!
//! let dir = TempDir::new("example")?;
//! std::fs::write(dir.0.join("README.md"), "# Readme\n")?;
//! assert!(dir.0.join("README.md").is_file());
//! # Ok::<(), std::io::Error>(())
//! ```

#![forbid(unsafe_code)]

use std::fs;
use std::io;
use std::path::PathBuf;

pub mod git;
pub mod vocabulary;

/// A directory under the system temp dir, created empty and removed on
/// drop. The path is the public field: tests join onto it directly.
#[derive(Debug)]
pub struct TempDir(pub PathBuf);

impl TempDir {
    /// Create `fathomable-<name>-<pid>` under the temp dir, empty; a
    /// leftover from an earlier run is removed first.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory cannot be created.
    pub fn new(name: &str) -> io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("fathomable-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

/// A tracked file of this repository, named by its path from the root,
/// for the gates that read the docs. The root comes from the manifest
/// directory Cargo (and nextest) hand the test process, so a test binary
/// reused from another checkout still reads the file where it runs;
/// the compile-time directory is the fallback outside Cargo.
#[must_use]
pub fn repo_file(relative: &str) -> PathBuf {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    manifest.join("../..").join(relative)
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
