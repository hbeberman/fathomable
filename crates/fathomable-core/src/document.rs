// @okf-doc: /decisions/0002-crate-layout.md
//! The document model: a file's path and its content as last read.

use std::fs;
use std::io::{self, Read};
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
    /// Remember a missing file without reading or creating it.
    ///
    /// The placeholder has empty text. [`Self::reload`] reads the file
    /// under `policy` if it reappears.
    #[must_use]
    pub fn missing(path: impl Into<PathBuf>, policy: Policy) -> Self {
        Self {
            path: path.into(),
            policy,
            content: Content::Text(String::new()),
        }
    }

    /// Retain snapshot `bytes` for a missing file under `policy`.
    ///
    /// The document still points at its worktree path, so [`Self::reload`]
    /// reads the live file if it reappears.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when text bytes are not valid UTF-8.
    pub fn from_snapshot(
        path: impl Into<PathBuf>,
        bytes: Vec<u8>,
        policy: Policy,
    ) -> Result<Self, LoadError> {
        let path = path.into();
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let content = classify_snapshot(&path, bytes, size, policy)?;
        tracing::debug!(
            path = %path.display(),
            content = ?Summary(&content),
            "loaded snapshot document"
        );
        Ok(Self {
            path,
            policy,
            content,
        })
    }

    /// Retain a bounded prefix of snapshot content whose complete byte size is
    /// `size`.
    ///
    /// A snapshot larger than `policy.max_bytes` is classified from `prefix`
    /// without retaining or requesting the rest. At or below the limit,
    /// `prefix` must contain the complete snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when an in-limit snapshot is incomplete or text
    /// bytes are not valid UTF-8.
    pub fn from_snapshot_prefix(
        path: impl Into<PathBuf>,
        prefix: Vec<u8>,
        size: u64,
        policy: Policy,
    ) -> Result<Self, LoadError> {
        let path = path.into();
        let content = classify_snapshot(&path, prefix, size, policy)?;
        tracing::debug!(
            path = %path.display(),
            content = ?Summary(&content),
            "loaded bounded snapshot document"
        );
        Ok(Self {
            path,
            policy,
            content,
        })
    }

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

    /// Replace retained snapshot bytes under the document's existing policy.
    ///
    /// Returns `Ok(true)` when the retained content changed.
    ///
    /// # Errors
    ///
    /// Returns [`LoadError`] when text bytes are not valid UTF-8.
    pub fn replace_snapshot(&mut self, bytes: Vec<u8>) -> Result<bool, LoadError> {
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let content = classify_snapshot(&self.path, bytes, size, self.policy)?;
        let changed = content != self.content;
        self.content = content;
        Ok(changed)
    }

    /// Follow the file to `path` after it was renamed on disk (ADR 0028):
    /// the content stays and the next [`Document::reload`] reads the new
    /// path.
    pub fn rename(&mut self, path: impl Into<PathBuf>) {
        self.path = path.into();
        tracing::debug!(path = %self.path.display(), "document renamed");
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
    let mut file = fs::File::open(path).map_err(fail)?;
    let size = file.metadata().map_err(fail)?.len();
    read_from(path, &mut file, size, policy)
}

fn read_from(
    path: &Path,
    mut reader: impl Read,
    size: u64,
    policy: Policy,
) -> Result<Content, LoadError> {
    let fail = |source| LoadError {
        path: path.to_path_buf(),
        source,
    };
    if policy.attr.decided() == Some(true) {
        let head = prefix(&mut reader).map_err(fail)?;
        return Ok(Content::Binary {
            size: size.max(u64::try_from(head.len()).unwrap_or(u64::MAX)),
            format: Format::sniff(&head),
        });
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
        let head = prefix(&mut reader).map_err(fail)?;
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
    let bytes = bounded_bytes(&mut reader, policy.max_bytes).map_err(fail)?;
    let retained = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let classified_size = if retained > policy.max_bytes {
        size.max(retained)
    } else {
        retained
    };
    classify_snapshot(path, bytes, classified_size, policy)
}

fn classify_snapshot(
    path: &Path,
    bytes: Vec<u8>,
    size: u64,
    policy: Policy,
) -> Result<Content, LoadError> {
    let fail = |source| LoadError {
        path: path.to_path_buf(),
        source,
    };
    let retained = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size <= policy.max_bytes && retained != size {
        return Err(fail(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "snapshot prefix does not contain the complete in-limit file",
        )));
    }
    if policy.attr.decided() == Some(true) {
        return Ok(Content::Binary {
            size,
            format: Format::sniff(&bytes),
        });
    }
    if size > policy.max_bytes {
        return Ok(
            if policy.attr.decided() == Some(false) || !crate::content::is_binary(&bytes) {
                Content::TooLarge {
                    size,
                    max_bytes: policy.max_bytes,
                }
            } else {
                Content::Binary {
                    size,
                    format: Format::sniff(&bytes),
                }
            },
        );
    }
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

/// Read enough to retain in-limit text or classify an over-limit file.
fn bounded_bytes(reader: impl Read, max_bytes: u64) -> io::Result<Vec<u8>> {
    let sniff_bytes = u64::try_from(crate::content::SNIFF_BYTES).unwrap_or(u64::MAX);
    let limit = max_bytes.saturating_add(1).max(sniff_bytes);
    let mut bytes = Vec::new();
    reader.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The first [`crate::content::SNIFF_BYTES`] from `reader`.
fn prefix(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(crate::content::SNIFF_BYTES);
    reader
        .take(u64::try_from(crate::content::SNIFF_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut head)?;
    Ok(head)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;

    use crate::content::{Attr, Content, Policy};

    use super::{LoadError, read_from};

    #[test]
    fn a_read_is_bounded_when_content_grows_past_observed_size() -> Result<(), LoadError> {
        let policy = Policy {
            attr: Attr::Text,
            max_bytes: 4,
        };
        let content = read_from(Path::new("growing.txt"), Cursor::new(b"12345"), 4, policy)?;
        assert_eq!(
            content,
            Content::TooLarge {
                size: 5,
                max_bytes: 4
            }
        );
        Ok(())
    }
}
