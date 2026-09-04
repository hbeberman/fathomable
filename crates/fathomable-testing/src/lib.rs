// @okf-doc: /decisions/0048-modules-by-concept.md
//! Test scaffolding shared by the Fathomable crates (ADR 0048).
//!
//! [`TempDir`] is a throwaway directory under the system temp dir, removed
//! when dropped; each test module writes its own fixture files into it.
//! The `git` functions build repositories the way the tests need them,
//! with `GIT_*` environment overrides ignored as the workspace itself
//! ignores them.
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

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
