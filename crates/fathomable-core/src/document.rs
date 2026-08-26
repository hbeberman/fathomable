// @okf-doc: /decisions/0002-crate-layout.md
//! The document model: a file's path and its text as last read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A file read from the workspace.
///
/// The workspace is read-only by construction: a `Document` remembers where it
/// came from and what it contained, and is refreshed with [`Document::reload`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    path: PathBuf,
    text: String,
}

/// Why a document could not be read.
#[derive(Debug, thiserror::Error)]
#[error("cannot read {path}")]
pub struct LoadError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl LoadError {
    /// The path that could not be read.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Document {
    /// Read `path` as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when the file cannot be opened, read, or is not
    /// valid UTF-8.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, LoadError> {
        let path = path.into();
        let text = read(&path)?;
        tracing::debug!(path = %path.display(), bytes = text.len(), "loaded document");
        Ok(Self { path, text })
    }

    /// Re-read the document from disk.
    ///
    /// Returns `Ok(true)` when the text changed. On error the previous text is
    /// kept, so a transient read failure never blanks the view.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when the file cannot be re-read.
    pub fn reload(&mut self) -> Result<bool, LoadError> {
        let text = read(&self.path)?;
        let changed = text != self.text;
        tracing::debug!(path = %self.path.display(), changed, "reloaded document");
        self.text = text;
        Ok(changed)
    }

    /// Where the document was read from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The document text as of the last successful load or reload.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

fn read(path: &Path) -> Result<String, LoadError> {
    fs::read_to_string(path).map_err(|source| LoadError {
        path: path.to_path_buf(),
        source,
    })
}
