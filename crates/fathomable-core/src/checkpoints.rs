// @okf-doc: /decisions/0049-inline-threads-and-the-rail.md
//! Checkpoints: the reader's own marks of a file's content (ADR 0049),
//! one timeline per file, beside git and the last-seen snapshots.
//!
//! A [`Store`] lives in a `checkpoints/` directory: `<sha256>` holds each
//! distinct content once, and `index.jsonl` is an append-only log of
//! events `{ "event": "checkpoint", "id", "created", "origin", "files":
//! [{ "path", "blob" }] }`. Checkpointing one file and checkpointing the
//! workspace append to the same per-file timelines; a file whose content
//! equals its latest entry gets no new entry, so a timeline never holds
//! two identical neighbours. Nothing expires; removing the directory is
//! always safe.
//!
//! # Examples
//!
//! ```no_run
//! use std::path::Path;
//!
//! use fathomable_core::checkpoints::{Origin, Store};
//!
//! let mut store = Store::open(Path::new("/tmp/checkpoints")).unwrap();
//! let stored = store.record(Origin::File, [(Path::new("README.md"), "# Hello\n")]).unwrap();
//! assert_eq!(stored, 1);
//! let latest = store.timeline(Path::new("README.md")).last().unwrap();
//! assert_eq!(store.text(latest).unwrap(), "# Hello\n");
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The append-only event log inside the store directory.
pub const INDEX_FILE: &str = "index.jsonl";

/// Which key made a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `Space v c`: this file alone.
    File,
    /// `Space v C`: every changed file in the workspace at once.
    Workspace,
}

/// One entry on a file's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    created: u64,
    origin: Origin,
    blob: String,
}

impl Checkpoint {
    /// When the checkpoint was made, in seconds since the Unix epoch.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Which key made it.
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// Whether it was part of a workspace checkpoint (the `◆` mark).
    #[must_use]
    pub fn is_workspace(&self) -> bool {
        self.origin == Origin::Workspace
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileRef {
    path: PathBuf,
    blob: String,
}

/// One log line; `kind` is written as `event` so the record names itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Event {
    #[serde(rename = "event")]
    kind: String,
    id: String,
    created: u64,
    origin: Origin,
    files: Vec<FileRef>,
}

const EVENT_NAME: &str = "checkpoint";

/// The checkpoint directory for one workspace.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    timelines: BTreeMap<PathBuf, Vec<Checkpoint>>,
    events: usize,
    log: Option<File>,
}

impl Store {
    /// Open or create the store at `dir`, reading every event in the log.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or log cannot be created
    /// or read; a malformed log line is skipped, not fatal.
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let log_path = dir.join(INDEX_FILE);
        let text = match fs::read_to_string(&log_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut store = Self {
            dir: dir.to_path_buf(),
            timelines: BTreeMap::new(),
            events: 0,
            log: None,
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Event>(line) {
                Ok(event) if event.kind == EVENT_NAME => store.apply(&event),
                Ok(event) => {
                    tracing::warn!(kind = %event.kind, "skipping unknown checkpoint event");
                }
                Err(error) => tracing::warn!(%error, "skipping malformed checkpoint record"),
            }
        }
        store.log = Some(
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(&log_path)?,
        );
        Ok(store)
    }

    /// The store directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Number of checkpoint events in the log.
    #[must_use]
    pub fn events(&self) -> usize {
        self.events
    }

    /// Number of files with a timeline.
    #[must_use]
    pub fn len(&self) -> usize {
        self.timelines.len()
    }

    /// Whether no file has a timeline.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timelines.is_empty()
    }

    /// Bytes the content blobs take on disk.
    #[must_use]
    pub fn blob_bytes(&self) -> u64 {
        fs::read_dir(&self.dir).map_or(0, |dir| {
            dir.filter_map(Result::ok)
                .filter(|e| e.file_name() != INDEX_FILE)
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
    }

    /// The timeline of the root-relative `path`, oldest first; empty when
    /// the file was never checkpointed.
    #[must_use]
    pub fn timeline(&self, path: &Path) -> &[Checkpoint] {
        self.timelines.get(path).map_or(&[], Vec::as_slice)
    }

    /// Whether `text` is what the latest checkpoint of `path` holds, so a
    /// new checkpoint would add nothing.
    #[must_use]
    pub fn is_current(&self, path: &Path, text: &str) -> bool {
        self.timeline(path)
            .last()
            .is_some_and(|latest| latest.blob == hash(text))
    }

    /// Append one checkpoint event for the `files` whose text differs
    /// from their latest entry, or that have none, and return how many
    /// that was. When it is none, nothing is written.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when a blob or the log cannot be written.
    pub fn record<'a>(
        &mut self,
        origin: Origin,
        files: impl IntoIterator<Item = (&'a Path, &'a str)>,
    ) -> io::Result<usize> {
        let mut refs = Vec::new();
        for (path, text) in files {
            if self.is_current(path, text) {
                continue;
            }
            let blob = hash(text);
            let target = self.dir.join(&blob);
            if !target.is_file() {
                let tmp = self.dir.join(format!("{blob}.tmp"));
                fs::write(&tmp, text)?;
                fs::rename(&tmp, &target)?;
            }
            refs.push(FileRef {
                path: path.to_path_buf(),
                blob,
            });
        }
        if refs.is_empty() {
            return Ok(0);
        }
        let created = now();
        let event = Event {
            kind: EVENT_NAME.to_owned(),
            id: event_id(created, &refs),
            created,
            origin,
            files: refs,
        };
        if let Some(log) = self.log.as_mut() {
            let mut line = serde_json::to_string(&event)?;
            line.push('\n');
            log.write_all(line.as_bytes())?;
        }
        let stored = event.files.len();
        self.apply(&event);
        Ok(stored)
    }

    /// The content a checkpoint holds.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the blob is missing or unreadable.
    pub fn text(&self, checkpoint: &Checkpoint) -> io::Result<String> {
        fs::read_to_string(self.dir.join(&checkpoint.blob))
    }

    fn apply(&mut self, event: &Event) {
        self.events += 1;
        for file in &event.files {
            self.timelines
                .entry(file.path.clone())
                .or_default()
                .push(Checkpoint {
                    created: event.created,
                    origin: event.origin,
                    blob: file.blob.clone(),
                });
        }
    }
}

/// An event id: the short hash of when it was made and what it lists.
fn event_id(created: u64, files: &[FileRef]) -> String {
    let mut bytes = created.to_le_bytes().to_vec();
    for file in files {
        bytes.extend_from_slice(file.path.as_os_str().as_encoded_bytes());
        bytes.extend_from_slice(file.blob.as_bytes());
    }
    crate::annotations::short_hash(&bytes)
}

fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> io::Result<Self> {
            let dir = std::env::temp_dir().join(format!(
                "fathomable-checkpoints-{name}-{}",
                std::process::id()
            ));
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

    #[test]
    fn timelines_grow_only_when_content_moves() -> io::Result<()> {
        let tmp = TempDir::new("timeline")?;
        let mut store = Store::open(&tmp.0)?;
        assert!(store.is_empty());
        let a = Path::new("a.md");
        let b = Path::new("b.md");

        assert_eq!(store.record(Origin::File, [(a, "one\n")])?, 1);
        assert_eq!(
            store.record(Origin::File, [(a, "one\n")])?,
            0,
            "the same content adds no entry"
        );
        assert_eq!(store.events(), 1, "and writes no event");
        assert_eq!(
            store.record(Origin::Workspace, [(a, "two\n"), (b, "one\n")])?,
            2
        );
        assert_eq!(
            store.record(Origin::Workspace, [(a, "two\n"), (b, "two\n")])?,
            1,
            "only the moved file joins the event"
        );

        let timeline = store.timeline(a);
        assert_eq!(timeline.len(), 2);
        assert!(!timeline[0].is_workspace());
        assert!(timeline[1].is_workspace());
        assert_eq!(store.text(&timeline[0])?, "one\n");
        assert_eq!(store.text(&timeline[1])?, "two\n");
        assert!(store.is_current(a, "two\n"));
        assert!(!store.is_current(a, "one\n"));
        assert!(!store.is_current(Path::new("c.md"), ""));
        assert_eq!(store.timeline(Path::new("c.md")), &[]);
        Ok(())
    }

    #[test]
    fn reopening_replays_the_log_and_keeps_one_blob_per_content() -> io::Result<()> {
        let tmp = TempDir::new("reopen")?;
        let mut store = Store::open(&tmp.0)?;
        store.record(Origin::File, [(Path::new("a.md"), "same\n")])?;
        store.record(Origin::File, [(Path::new("b.md"), "same\n")])?;
        store.record(Origin::Workspace, [(Path::new("a.md"), "other\n")])?;
        drop(store);

        let mut log = OpenOptions::new()
            .append(true)
            .open(tmp.0.join(INDEX_FILE))?;
        writeln!(log, "not json")?;
        writeln!(
            log,
            r#"{{"event":"other","id":"x","created":1,"origin":"file","files":[]}}"#
        )?;
        drop(log);

        let store = Store::open(&tmp.0)?;
        assert_eq!(store.events(), 3, "malformed and foreign lines are skipped");
        assert_eq!(store.len(), 2);
        assert_eq!(store.timeline(Path::new("a.md")).len(), 2);
        assert_eq!(store.timeline(Path::new("b.md")).len(), 1);
        let blobs = fs::read_dir(&tmp.0)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != INDEX_FILE)
            .count();
        assert_eq!(blobs, 2, "identical content across files is stored once");
        assert!(store.blob_bytes() > 0);
        Ok(())
    }
}
