// @okf-doc: /decisions/0002-crate-layout.md
//! The document model: a file's path and its content as last read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::content::{Content, Format, Policy};

/// A file read from the workspace.
///
/// The workspace is read-only by construction: a `Document` remembers where it
/// came from and what it contained, and is refreshed with [`Document::reload`].
/// What it holds is a [`Content`]: text, or the size of a binary or
/// over-limit file that was not read (ADR 0026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    path: PathBuf,
    policy: Policy,
    content: Content,
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
    /// Read `path` under `policy`: as UTF-8 text, or as a binary or
    /// over-limit file whose bytes are left on disk.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when the file cannot be opened or read, or is
    /// text by `policy` but not valid UTF-8.
    pub fn load(path: impl Into<PathBuf>, policy: Policy) -> Result<Self, LoadError> {
        let path = path.into();
        let content = read(&path, policy)?;
        tracing::debug!(path = %path.display(), content = ?Summary(&content), "loaded document");
        Ok(Self {
            path,
            policy,
            content,
        })
    }

    /// Re-read the document from disk under the policy it was loaded with.
    ///
    /// Returns `Ok(true)` when the content changed. On error the previous
    /// content is kept, so a transient read failure never blanks the view.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when the file cannot be re-read.
    pub fn reload(&mut self) -> Result<bool, LoadError> {
        let content = read(&self.path, self.policy)?;
        let changed = content != self.content;
        tracing::debug!(path = %self.path.display(), changed, "reloaded document");
        self.content = content;
        Ok(changed)
    }

    /// Where the document was read from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the document holds as of the last successful load or reload.
    #[must_use]
    pub fn content(&self) -> &Content {
        &self.content
    }

    /// The document text, when it is text.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.content.text()
    }
}

/// `Content` for the log: the text's length, never the text.
struct Summary<'a>(&'a Content);

impl std::fmt::Debug for Summary<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Content::Text(text) => write!(f, "text({} bytes)", text.len()),
            other => write!(f, "{other:?}"),
        }
    }
}

fn read(path: &Path, policy: Policy) -> Result<Content, LoadError> {
    let fail = |source| LoadError {
        path: path.to_path_buf(),
        source,
    };
    let size = fs::metadata(path).map_err(fail)?.len();
    if policy.attr.decided() == Some(true) {
        let format = sniff(path).map_err(fail)?;
        return Ok(Content::Binary { size, format });
    }
    if size > policy.max_bytes && policy.attr.decided() == Some(false) {
        return Ok(Content::TooLarge {
            size,
            max_bytes: policy.max_bytes,
        });
    }
    if size > policy.max_bytes {
        // The prefix says whether this is a large text or a binary; either
        // way the rest stays on disk.
        let head = prefix(path).map_err(fail)?;
        return Ok(if crate::content::is_binary(&head) {
            Content::Binary {
                size,
                format: Format::sniff(&head),
            }
        } else {
            Content::TooLarge {
                size,
                max_bytes: policy.max_bytes,
            }
        });
    }
    let bytes = fs::read(path).map_err(fail)?;
    if policy.attr.classify(&bytes) == Some(true) {
        return Ok(Content::Binary {
            size,
            format: Format::sniff(&bytes),
        });
    }
    String::from_utf8(bytes)
        .map(Content::Text)
        .map_err(|error| fail(io::Error::new(io::ErrorKind::InvalidData, error)))
}

/// The magic number of the file at `path`, without reading the rest.
fn sniff(path: &Path) -> io::Result<Option<Format>> {
    Ok(Format::sniff(&prefix(path)?))
}

/// The first [`crate::content::SNIFF_BYTES`] of the file at `path`.
fn prefix(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(crate::content::SNIFF_BYTES);
    fs::File::open(path)?
        .take(u64::try_from(crate::content::SNIFF_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut head)?;
    Ok(head)
}
