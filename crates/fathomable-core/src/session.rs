// @okf-doc: /decisions/0062-one-version-no-compatibility.md
//! Viewer records and workspace markers.
//!
//! A workspace's annotation state is the workspace's; a running TUI is a
//! *viewer* of it (ADR 0024, words per ADR 0047). Each viewer writes a
//! [`Record`] under `$XDG_STATE_HOME/fathomable/viewers/<id>/`; the workspace
//! itself is marked by a [`Marker`] beside its thread store for viewer and
//! worktree diagnostics.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::XdgDirs;
/// File name of the record inside a session directory.
pub(crate) const RECORD_FILE: &str = "session.json";

/// File name of the workspace marker inside a workspace state directory.
pub(crate) const WORKSPACE_FILE: &str = "workspace.json";

/// A session identifier: `<unix-seconds>-<pid>`, unique per host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    /// Mint an id for the current process.
    #[must_use]
    pub fn mint() -> Self {
        let seconds = crate::clock::now();
        Self(format!("{seconds}-{}", std::process::id()))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Id {
    type Err = IdParseError;

    /// Accept `digits-digits` only, so an id can never name a path outside
    /// the sessions directory.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = s.split_once('-').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_digit())
                && b.bytes().all(|c| c.is_ascii_digit())
        });
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(IdParseError(s.to_owned()))
        }
    }
}

/// An invalid viewer session identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdParseError(String);

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid session id `{}`", self.0)
    }
}

impl std::error::Error for IdParseError {}

/// What a running viewer writes about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    id: Id,
    pid: u32,
    /// The workspace key (ADR 0070): what the state directory is keyed by.
    key: PathBuf,
    /// The worktree the viewer shows; the key itself outside git.
    root: PathBuf,
    started: u64,
    /// The user-set viewer name (ADR 0024 `--name`, `:name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

impl Record {
    /// Describe the current process as a viewer of the workspace keyed
    /// by `key`, showing the worktree at `root`.
    #[must_use]
    pub fn new(id: Id, key: PathBuf, root: PathBuf) -> Self {
        Self {
            id,
            pid: std::process::id(),
            key,
            root,
            started: crate::clock::now(),
            name: None,
        }
    }

    /// The same viewer showing the worktree at `root` (ADR 0070).
    #[must_use]
    pub fn on_worktree(mut self, root: PathBuf) -> Self {
        self.root = root;
        self
    }

    /// The workspace key (ADR 0070).
    #[must_use]
    pub fn key(&self) -> &Path {
        &self.key
    }

    /// Give the viewer a name; empty clears it.
    #[must_use]
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name.filter(|name| !name.trim().is_empty());
        self
    }

    /// The viewer name, when the user set one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Whether `key` names this viewer: its name or its id.
    #[must_use]
    pub fn is_called(&self, key: &str) -> bool {
        self.id.as_str() == key || self.name.as_deref() == Some(key)
    }

    /// The session id.
    #[must_use]
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Process id of the TUI.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Workspace root the session views.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Start time in seconds since the Unix epoch.
    #[must_use]
    pub fn started(&self) -> u64 {
        self.started
    }

    /// Whether the recorded process still exists (Linux `/proc`).
    #[must_use]
    pub fn is_alive(&self) -> bool {
        Path::new("/proc").join(self.pid.to_string()).exists()
    }

    /// The directory this record lives in.
    #[must_use]
    pub fn dir(&self, dirs: &XdgDirs) -> PathBuf {
        dirs.viewers_dir().join(self.id.as_str())
    }

    /// Write the record, creating its directory.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or file cannot be written.
    pub fn write(&self, dirs: &XdgDirs) -> io::Result<()> {
        let dir = self.dir(dirs);
        dirs.prepare_state_dir(&dir)?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        crate::private_state::write(dir.join(RECORD_FILE), json)
    }

    /// Remove the record directory; missing is not an error.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory exists but cannot be removed.
    pub fn remove(&self, dirs: &XdgDirs) -> io::Result<()> {
        match fs::symlink_metadata(self.dir(dirs)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        dirs.prepare_state_dir(self.dir(dirs))?;
        match crate::private_state::open_read(self.dir(dirs).join(RECORD_FILE)) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match fs::remove_dir_all(self.dir(dirs)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Read one record file.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the file cannot be read or is not a record.
    pub fn read(path: &Path) -> io::Result<Self> {
        let bytes = crate::private_state::read(path)?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }

    /// Every record on disk, oldest first; unreadable ones are skipped with
    /// a log line. A missing viewers directory yields an empty list.
    #[must_use]
    pub fn list(dirs: &XdgDirs) -> Vec<Self> {
        let mut records = Self::list_in(dirs);
        records.sort_by_key(|record| record.started);
        records
    }

    fn list_in(dirs: &XdgDirs) -> Vec<Self> {
        let mut records = Vec::new();
        let dir = dirs.viewers_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return records;
        };
        if let Err(error) = dirs.prepare_state_dir(&dir) {
            tracing::warn!(%error, "cannot read viewer directory");
            return records;
        }
        for entry in entries.flatten() {
            if let Err(error) = dirs.prepare_state_dir(entry.path()) {
                tracing::warn!(%error, "cannot read viewer record directory");
                continue;
            }
            let path = entry.path().join(RECORD_FILE);
            match Self::read(&path) {
                Ok(record) => records.push(record),
                Err(error) => tracing::warn!(%error, path = %path.display(), "bad viewer record"),
            }
        }
        records
    }

    /// Live records, oldest first.
    #[must_use]
    pub fn live(dirs: &XdgDirs) -> Vec<Self> {
        Self::list(dirs)
            .into_iter()
            .filter(Self::is_alive)
            .collect()
    }

    /// Remove records whose process is gone from the viewers directory.
    /// Returns how many were removed.
    pub fn sweep_dead(dirs: &XdgDirs) -> usize {
        let mut removed = 0;
        for record in Self::list_in(dirs) {
            if record.is_alive() {
                continue;
            }
            match record.remove(dirs) {
                Ok(()) => {
                    removed += 1;
                    tracing::info!(id = %record.id, pid = record.pid, "removed dead viewer");
                }
                Err(error) => tracing::warn!(%error, id = %record.id, "cannot remove viewer"),
            }
        }
        removed
    }
}

/// The marker a viewer leaves beside a workspace's thread store (ADR 0024).
///
/// It names the key behind the state directory's hash and every
/// worktree root the workspace had when it was written (ADR 0070), so
/// the workspace is known, and a cwd in any of its worktrees finds it,
/// when no viewer runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    key: PathBuf,
    roots: Vec<PathBuf>,
    /// When a viewer last started here, in Unix seconds.
    last_seen: u64,
}

impl Marker {
    /// Mark the workspace keyed by `key`, with worktrees at `roots`, as
    /// seen now.
    #[must_use]
    pub fn new(key: PathBuf, roots: Vec<PathBuf>) -> Self {
        Self {
            key,
            roots,
            last_seen: crate::clock::now(),
        }
    }

    /// The workspace key (ADR 0070).
    #[must_use]
    pub fn key(&self) -> &Path {
        &self.key
    }

    /// The worktree roots the workspace had when the marker was written.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// When a viewer last started here, in Unix seconds.
    #[must_use]
    pub fn last_seen(&self) -> u64 {
        self.last_seen
    }

    /// Write the marker into the workspace state directory.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory or file cannot be written.
    pub fn write(&self, dirs: &XdgDirs) -> io::Result<()> {
        let path = dirs.workspace_file(&self.key);
        if let Some(dir) = path.parent() {
            dirs.prepare_state_dir(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        crate::private_state::write(path, json)
    }

    /// Every known workspace, most recently seen first; unreadable markers
    /// are skipped with a log line.
    #[must_use]
    pub fn list(dirs: &XdgDirs) -> Vec<Self> {
        let mut markers = Vec::new();
        let Ok(entries) = fs::read_dir(dirs.state_dir().join("workspaces")) else {
            return markers;
        };
        if let Err(error) = dirs.prepare_state_dir(dirs.state_dir().join("workspaces")) {
            tracing::warn!(%error, "cannot read workspace directory");
            return markers;
        }
        for entry in entries.flatten() {
            if let Err(error) = dirs.prepare_state_dir(entry.path()) {
                tracing::warn!(%error, "cannot read workspace marker directory");
                continue;
            }
            let path = entry.path().join(WORKSPACE_FILE);
            let text = match crate::private_state::read(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "bad workspace marker");
                    continue;
                }
            };
            match serde_json::from_slice::<Self>(&text) {
                Ok(marker) => markers.push(marker),
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "bad workspace marker");
                }
            }
        }
        markers.sort_by_key(|marker| std::cmp::Reverse(marker.last_seen));
        markers
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Id, IdParseError, Record};

    fn record() -> Record {
        Record::new(
            "1700000000-42".parse().unwrap_or_else(|_| Id::mint()),
            PathBuf::from("/work"),
            PathBuf::from("/work"),
        )
    }

    /// Dead records are swept from `viewers/`; live ones stay.
    #[test]
    fn dead_records_are_swept() -> std::io::Result<()> {
        use std::ffi::OsString;
        use std::fs;

        let state = std::env::temp_dir().join(format!(
            "fathomable-sweep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let _ = fs::remove_dir_all(&state);
        let dirs = crate::XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| OsString::from(&state))
        });
        // A pid no live process has: the record reads as dead.
        let dead =
            r#"{"id":"1700000000-4000000","pid":4000000,"key":"/w","root":"/w","started":1}"#;
        let record_dir = dirs.viewers_dir().join("1700000000-4000000");
        dirs.prepare_state_dir(&record_dir)?;
        crate::private_state::write(record_dir.join(super::RECORD_FILE), dead)?;
        record().write(&dirs)?;
        assert_eq!(Record::sweep_dead(&dirs), 1);
        assert!(!dirs.viewers_dir().join("1700000000-4000000").exists());
        assert_eq!(Record::list(&dirs).len(), 1, "the live record stays");
        fs::remove_dir_all(&state)
    }

    #[test]
    fn ids_are_validated() -> Result<(), IdParseError> {
        let id: Id = "1700000000-42".parse()?;
        assert_eq!(id.to_string(), "1700000000-42");
        assert_eq!(
            "../etc".parse::<Id>().map_err(|error| error.to_string()),
            Err("invalid session id `../etc`".to_owned())
        );
        assert_eq!("12-".parse::<Id>().ok(), None);
        let minted = Id::mint();
        assert_eq!(minted.as_str().parse::<Id>().ok(), Some(minted.clone()));
        Ok(())
    }

    #[test]
    fn records_carry_an_optional_name() -> Result<(), serde_json::Error> {
        let named = record().with_name(Some("left".to_owned()));
        assert_eq!(named.name(), Some("left"));
        assert!(named.is_called("left"));
        assert!(named.is_called("1700000000-42"));
        assert!(!named.is_called("right"));
        assert_eq!(record().with_name(Some("  ".to_owned())).name(), None);
        let json = serde_json::to_string(&named)?;
        assert!(json.contains(r#""name":"left""#), "{json}");
        assert!(!json.contains("socket"), "{json}");
        assert!(!serde_json::to_string(&record())?.contains("name"));
        assert_eq!(serde_json::from_str::<Record>(&json)?, named);
        Ok(())
    }
}
