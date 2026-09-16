// @okf-doc: /decisions/0015-follow-mode.md
//! Last-seen snapshots: the text of each file as the reader last saw it
//! (ADR 0015), the base for "what changed since I looked".
//!
//! A [`Store`] lives in a `seen/` directory: `blobs/<sha256>` holds each
//! distinct content once, and `seen.jsonl` logs `{path, sha256, at}`
//! records, the last record per path winning. Opening the store drops
//! records older than [`MAX_AGE`], rewrites the log to its last records,
//! and deletes blobs nothing references. Files over [`MAX_BYTES`] are not
//! snapshotted. Removing the directory is always safe.
//!
//! # Examples
//!
//! ```no_run
//! use std::path::Path;
//!
//! use fathomable_core::seen::Store;
//!
//! let mut store = Store::open(Path::new("/tmp/seen")).unwrap();
//! store.record(Path::new("README.md"), "# Hello\n").unwrap();
//! assert_eq!(store.text(Path::new("README.md")).unwrap().as_deref(), Some("# Hello\n"));
//! ```

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::clock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Files larger than this are never snapshotted.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

/// Records older than this are dropped when the store opens.
pub const MAX_AGE: Duration = Duration::from_hours(30 * 24);

const LOG_FILE: &str = "seen.jsonl";
const BLOBS_DIR: &str = "blobs";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    sha256: String,
    at: u64,
}

/// The snapshot directory for one workspace.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    entries: BTreeMap<PathBuf, Entry>,
    log: File,
}

/// Read-only access to the latest recorded snapshots.
///
/// Unlike [`Store::open`], loading snapshots this way never creates,
/// compacts, prunes, or otherwise changes the snapshot directory.
#[derive(Debug)]
pub struct Snapshots {
    dir: PathBuf,
    entries: BTreeMap<PathBuf, Entry>,
}

impl Snapshots {
    /// Load the latest record for every path without changing the store.
    ///
    /// A missing log is an empty snapshot set. Malformed records are skipped.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when an existing log cannot be read.
    pub fn read(dir: &Path) -> io::Result<Self> {
        let log_path = dir.join(LOG_FILE);
        let text = match fs::read_to_string(log_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut entries = BTreeMap::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => {
                    entries.insert(entry.path.clone(), entry);
                }
                Err(error) => tracing::warn!(%error, "skipping malformed seen record"),
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            entries,
        })
    }

    /// The snapshot of `path`, when both its record and blob exist.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the recorded blob exists but cannot be read.
    pub fn text(&self, path: &Path) -> io::Result<Option<String>> {
        let Some(entry) = self.entries.get(path) else {
            return Ok(None);
        };
        match fs::read_to_string(self.dir.join(BLOBS_DIR).join(&entry.sha256)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Store {
    /// Open or create the store at `dir`, compacting the log and pruning
    /// unreferenced or expired blobs.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or log cannot be created
    /// or read; a malformed log line is skipped, not fatal.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_pinned(dir, std::iter::empty())
    }

    /// Open like [`open`](Self::open), but keep the snapshots of `pinned`
    /// paths however old they are (ADR 0020: files with open threads).
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or log cannot be created
    /// or read; a malformed log line is skipped, not fatal.
    pub fn open_pinned<'a>(
        dir: &Path,
        pinned: impl IntoIterator<Item = &'a Path>,
    ) -> io::Result<Self> {
        let pinned: HashSet<&Path> = pinned.into_iter().collect();
        fs::create_dir_all(dir.join(BLOBS_DIR))?;
        let log_path = dir.join(LOG_FILE);
        let text = match fs::read_to_string(&log_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let cutoff = clock::now().saturating_sub(MAX_AGE.as_secs());
        let mut entries = BTreeMap::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) if entry.at >= cutoff || pinned.contains(entry.path.as_path()) => {
                    entries.insert(entry.path.clone(), entry);
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "skipping malformed seen record"),
            }
        }
        let mut compacted = String::new();
        for entry in entries.values() {
            let _ = writeln!(compacted, "{}", serde_json::to_string(entry)?);
        }
        fs::write(&log_path, compacted)?;
        prune_blobs(dir, &entries);
        Ok(Self {
            dir: dir.to_path_buf(),
            entries,
            log: OpenOptions::new().append(true).open(&log_path)?,
        })
    }

    /// The store directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Number of files with a snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no file has a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes used by the blobs on disk.
    #[must_use]
    pub fn blob_bytes(&self) -> u64 {
        fs::read_dir(self.dir.join(BLOBS_DIR)).map_or(0, |dir| {
            dir.filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
    }

    /// Snapshot `text` as what the reader last saw of the root-relative
    /// `path`. Returns `false`, recording nothing, when the text exceeds
    /// [`MAX_BYTES`] or already is the snapshot.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the blob or the log cannot be written.
    pub fn record(&mut self, path: &Path, text: &str) -> io::Result<bool> {
        if text.len() > MAX_BYTES {
            return Ok(false);
        }
        let sha256 = hash(text);
        if self.entries.get(path).is_some_and(|e| e.sha256 == sha256) {
            return Ok(false);
        }
        let blob = self.blob_path(&sha256);
        if !blob.is_file() {
            let tmp = blob.with_extension("tmp");
            fs::write(&tmp, text)?;
            fs::rename(&tmp, &blob)?;
        }
        let entry = Entry {
            path: path.to_path_buf(),
            sha256,
            at: clock::now(),
        };
        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');
        self.log.write_all(line.as_bytes())?;
        self.entries.insert(path.to_path_buf(), entry);
        Ok(true)
    }

    /// The snapshot of the root-relative `path`, if one exists.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the blob exists in the log but cannot be
    /// read.
    pub fn text(&self, path: &Path) -> io::Result<Option<String>> {
        let Some(entry) = self.entries.get(path) else {
            return Ok(None);
        };
        match fs::read_to_string(self.blob_path(&entry.sha256)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Whether `path` has a snapshot.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        self.dir.join(BLOBS_DIR).join(sha256)
    }
}

fn prune_blobs(dir: &Path, entries: &BTreeMap<PathBuf, Entry>) {
    let live: HashSet<&str> = entries.values().map(|e| e.sha256.as_str()).collect();
    let Ok(dir) = fs::read_dir(dir.join(BLOBS_DIR)) else {
        return;
    };
    for entry in dir.filter_map(Result::ok) {
        let name = entry.file_name();
        let keep = name.to_str().is_some_and(|n| live.contains(n));
        if !keep && let Err(error) = fs::remove_file(entry.path()) {
            tracing::warn!(%error, path = %entry.path().display(), "cannot prune blob");
        }
    }
}

fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use fathomable_testing::TempDir;

    use super::*;

    #[test]
    fn records_and_reads_back_across_reopen() -> io::Result<()> {
        let tmp = TempDir::new("seen-roundtrip")?;
        let mut store = Store::open(&tmp.0)?;
        assert!(store.is_empty());
        assert!(store.record(Path::new("a.md"), "one\n")?);
        assert!(!store.record(Path::new("a.md"), "one\n")?);
        assert!(store.record(Path::new("b.md"), "one\n")?);
        assert!(store.record(Path::new("a.md"), "two\n")?);
        assert_eq!(store.len(), 2);
        assert_eq!(store.text(Path::new("a.md"))?.as_deref(), Some("two\n"));
        assert_eq!(store.text(Path::new("b.md"))?.as_deref(), Some("one\n"));
        assert_eq!(store.text(Path::new("c.md"))?, None);
        drop(store);

        let store = Store::open(&tmp.0)?;
        assert_eq!(store.len(), 2);
        assert!(store.contains(Path::new("a.md")));
        assert_eq!(store.text(Path::new("a.md"))?.as_deref(), Some("two\n"));
        let log = fs::read_to_string(tmp.0.join(LOG_FILE))?;
        assert_eq!(log.lines().count(), 2, "log compacted to last records");
        assert!(store.blob_bytes() > 0);
        Ok(())
    }

    #[test]
    fn snapshot_reader_does_not_compact_or_create_state() -> io::Result<()> {
        let tmp = TempDir::new("seen-read-only")?;
        let dir = tmp.0.join("seen");
        let mut store = Store::open(&dir)?;
        store.record(Path::new("a.md"), "one\n")?;
        drop(store);
        let log = dir.join(LOG_FILE);
        let before = fs::read(&log)?;

        let snapshots = Snapshots::read(&dir)?;
        assert_eq!(snapshots.text(Path::new("a.md"))?.as_deref(), Some("one\n"));
        assert_eq!(fs::read(log)?, before);

        let missing = tmp.0.join("missing");
        assert!(Snapshots::read(&missing)?.entries.is_empty());
        assert!(!missing.exists());
        Ok(())
    }

    #[test]
    fn open_prunes_unreferenced_and_expired() -> io::Result<()> {
        let tmp = TempDir::new("seen-prune")?;
        let mut store = Store::open(&tmp.0)?;
        store.record(Path::new("a.md"), "one\n")?;
        store.record(Path::new("a.md"), "two\n")?;
        drop(store);
        let blobs = || fs::read_dir(tmp.0.join(BLOBS_DIR)).map_or(0, Iterator::count);
        assert_eq!(blobs(), 2);
        let store = Store::open(&tmp.0)?;
        assert_eq!(blobs(), 1, "the superseded blob is pruned");
        drop(store);

        let stale = Entry {
            path: PathBuf::from("old.md"),
            sha256: hash("gone\n"),
            at: 1,
        };
        let mut log = OpenOptions::new().append(true).open(tmp.0.join(LOG_FILE))?;
        writeln!(log, "{}", serde_json::to_string(&stale)?)?;
        writeln!(log, "not json")?;
        drop(log);
        let store = Store::open_pinned(&tmp.0, [Path::new("old.md")])?;
        assert!(
            store.contains(Path::new("old.md")),
            "pinned records outlive MAX_AGE"
        );
        drop(store);
        let store = Store::open(&tmp.0)?;
        assert!(!store.contains(Path::new("old.md")));
        assert!(store.contains(Path::new("a.md")));
        Ok(())
    }

    #[test]
    fn large_text_is_not_recorded() -> io::Result<()> {
        let tmp = TempDir::new("seen-large")?;
        let mut store = Store::open(&tmp.0)?;
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(!store.record(Path::new("big"), &big)?);
        assert!(!store.contains(Path::new("big")));
        Ok(())
    }

    #[test]
    fn failed_log_write_does_not_advance_snapshot() -> io::Result<()> {
        let tmp = TempDir::new("seen-log-error")?;
        let mut store = Store::open(&tmp.0)?;
        let path = Path::new("a.md");
        store.record(path, "one\n")?;
        store.log = File::open(tmp.0.join(LOG_FILE))?;

        if let Ok(recorded) = store.record(path, "two\n") {
            return Err(io::Error::other(format!(
                "record succeeded with a read-only log: recorded={recorded}"
            )));
        }
        assert_eq!(store.text(path)?.as_deref(), Some("one\n"));
        drop(store);

        let mut store = Store::open(&tmp.0)?;
        assert_eq!(store.text(path)?.as_deref(), Some("one\n"));
        assert!(store.record(path, "two\n")?);
        drop(store);
        assert_eq!(Store::open(&tmp.0)?.text(path)?.as_deref(), Some("two\n"));
        Ok(())
    }
}
