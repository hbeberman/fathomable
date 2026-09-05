// @okf-doc: /decisions/0024-workspace-sessions.md
//! Viewer records, workspace markers, and the v2 socket protocol.
//!
//! A workspace's annotation state is the workspace's; a running TUI is a
//! *viewer* of it (ADR 0024, words per ADR 0047). Each viewer writes a
//! [`Record`] under `$XDG_STATE_HOME/fathomable/viewers/<id>/` and listens on
//! `$XDG_RUNTIME_DIR/fathomable/<workspace-hash>/<pid>.sock`; the workspace
//! itself is marked by a [`Marker`] beside its thread store so an agent can
//! find it when no viewer runs. The socket speaks line-delimited JSON: one
//! [`Request`] per line, answered by one [`Response`] per line. Every
//! request carries `"v"`; version 2 carries the viewer name in records,
//! version 1 (ADR 0014) added `open`, `follow`, `threads_list` (named
//! `annotations_list` until ADR 0051), and `thread_reply` to the v0 `ping`
//! and `session_info` (ADR 0012), which are still accepted with an older
//! `"v"`. The binary owns the socket and
//! the state behind every operation; this module owns the wire types.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::session::{Request, Response};
//!
//! let request: Request = r#"{"v":1,"op":"ping"}"#.parse()?;
//! assert_eq!(request, Request::Ping);
//! assert_eq!(Response::Pong.to_line(), r#"{"ok":true,"pong":true}"#);
//! # Ok::<(), fathomable_core::session::ProtocolError>(())
//! ```

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::XdgDirs;
use crate::annotations::{Author, LineRange, Thread, ThreadId};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// The oldest protocol version still accepted, for `ping` and `session_info`.
const OLDEST_VERSION: u32 = 0;

/// File name of the record inside a session directory.
pub const RECORD_FILE: &str = "session.json";

/// File name of the workspace marker inside a workspace state directory.
pub const WORKSPACE_FILE: &str = "workspace.json";

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
    type Err = ProtocolError;

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
            Err(ProtocolError(format!("invalid session id `{s}`")))
        }
    }
}

/// What a running viewer writes about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    id: Id,
    pid: u32,
    root: PathBuf,
    socket: PathBuf,
    started: u64,
    /// The user-set viewer name (ADR 0024 `--name`, `:name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

impl Record {
    /// Describe the current process as a session on `root`; `socket` is
    /// where it will listen (or `None` when `XDG_RUNTIME_DIR` is unset).
    #[must_use]
    pub fn new(id: Id, root: PathBuf, socket: Option<PathBuf>) -> Self {
        Self {
            id,
            pid: std::process::id(),
            root,
            socket: socket.unwrap_or_default(),
            started: crate::clock::now(),
            name: None,
        }
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

    /// Socket path, or `None` when the session has no socket.
    #[must_use]
    pub fn socket(&self) -> Option<&Path> {
        if self.socket.as_os_str().is_empty() {
            None
        } else {
            Some(&self.socket)
        }
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
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(dir.join(RECORD_FILE), json)
    }

    /// Remove the record directory; missing is not an error.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the directory exists but cannot be removed.
    pub fn remove(&self, dirs: &XdgDirs) -> io::Result<()> {
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
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(io::Error::other)
    }

    /// Every record on disk, oldest first; unreadable ones are skipped with
    /// a log line. A missing viewers directory yields an empty list.
    #[must_use]
    pub fn list(dirs: &XdgDirs) -> Vec<Self> {
        let mut records = Self::list_in(&dirs.viewers_dir());
        records.sort_by_key(|record| record.started);
        records
    }

    fn list_in(dir: &Path) -> Vec<Self> {
        let mut records = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return records;
        };
        for entry in entries.flatten() {
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
        let dir = dirs.viewers_dir();
        let mut removed = 0;
        for record in Self::list_in(&dir) {
            if record.is_alive() {
                continue;
            }
            match fs::remove_dir_all(dir.join(record.id.as_str())) {
                Ok(()) => {
                    removed += 1;
                    if let Some(socket) = record.socket() {
                        let _ = fs::remove_file(socket);
                    }
                    tracing::info!(id = %record.id, pid = record.pid, "removed dead viewer");
                }
                Err(error) => tracing::warn!(%error, id = %record.id, "cannot remove viewer"),
            }
        }
        removed
    }
}

/// The marker a viewer leaves beside a workspace's thread store, so the
/// root behind the state directory's hash is known when no viewer runs
/// (ADR 0024).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    root: PathBuf,
    /// When a viewer last started here, in Unix seconds.
    last_seen: u64,
}

impl Marker {
    /// Mark `root` as a workspace seen now.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            last_seen: crate::clock::now(),
        }
    }

    /// The workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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
        let path = dirs.workspace_file(&self.root);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json)
    }

    /// Every known workspace, most recently seen first; unreadable markers
    /// are skipped with a log line.
    #[must_use]
    pub fn list(dirs: &XdgDirs) -> Vec<Self> {
        let mut markers = Vec::new();
        let Ok(entries) = fs::read_dir(dirs.state_dir().join("workspaces")) else {
            return markers;
        };
        for entry in entries.flatten() {
            let path = entry.path().join(WORKSPACE_FILE);
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "bad workspace marker");
                    continue;
                }
            };
            match serde_json::from_str::<Self>(&text) {
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

/// A request over the session socket.
///
/// Paths are workspace-relative. `since` is in Unix seconds and matches
/// threads whose [`Thread::updated`] is at or after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Ask for the session record.
    SessionInfo,
    /// Show a file, optionally scrolled to a source line range.
    Open {
        /// Workspace-relative path.
        path: PathBuf,
        /// First source line to show, 1-based.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
        /// Last line of the range, when a range should be selected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_line: Option<usize>,
    },
    /// Record the files an agent is working on; replaces the previous list.
    Follow {
        /// Workspace-relative paths; empty clears the list.
        paths: Vec<PathBuf>,
    },
    /// Threads, optionally changed since a time or limited to one file.
    ThreadsList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    /// Append a reply to a thread, optionally resolving it. `lines`, when
    /// given, says where the thread's lines are now (ADR 0033): the
    /// thread is re-anchored there before the reply is added.
    ThreadReply {
        thread: ThreadId,
        author: Author,
        body: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resolve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<LineRange>,
    },
}

#[derive(Serialize, Deserialize)]
struct RequestWire {
    v: u32,
    #[serde(flatten)]
    request: Request,
}

#[derive(Deserialize)]
struct VersionOnly {
    v: u32,
}

impl Request {
    /// The request as one JSON line without the newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        serde_json::to_string(&RequestWire {
            v: PROTOCOL_VERSION,
            request: self.clone(),
        })
        .unwrap_or_default()
    }

    /// Whether a client speaking `v` may send this request.
    fn allowed_in(&self, v: u32) -> bool {
        v == PROTOCOL_VERSION || matches!(self, Self::Ping | Self::SessionInfo)
    }
}

impl FromStr for Request {
    type Err = ProtocolError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let VersionOnly { v } = serde_json::from_str(line)
            .map_err(|error| ProtocolError(format!("malformed request: {error}")))?;
        if !(OLDEST_VERSION..=PROTOCOL_VERSION).contains(&v) {
            return Err(ProtocolError(format!(
                "unsupported protocol version {v} (this session speaks {PROTOCOL_VERSION})"
            )));
        }
        let wire: RequestWire = serde_json::from_str(line)
            .map_err(|error| ProtocolError(format!("malformed request: {error}")))?;
        if wire.request.allowed_in(v) {
            Ok(wire.request)
        } else {
            Err(ProtocolError(format!(
                "operation needs protocol version {PROTOCOL_VERSION} (request says {v})"
            )))
        }
    }
}

/// A response over the session socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong,
    /// Answer to [`Request::SessionInfo`]: the record plus the follow-mode
    /// state when the TUI answered (ADR 0015), `None` when the socket
    /// answered alone.
    Session(Record, Option<FollowState>),
    /// Answer to [`Request::Open`], [`Request::Follow`], and
    /// [`Request::ThreadReply`]: the operation took effect.
    Done,
    /// Answer to [`Request::ThreadsList`].
    Threads(Vec<Thread>),
    /// The request was refused; the text says why.
    Error(String),
}

/// Follow-mode state reported with `session_info` (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowState {
    /// Whether auto-jump is on.
    pub auto_jump: bool,
}

#[derive(Serialize, Deserialize)]
struct ResponseWire {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pong: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Record>,
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    follow: Option<FollowState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    threads: Option<Vec<Thread>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Response {
    /// The response as one JSON line without the newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut wire = ResponseWire {
            ok: true,
            pong: None,
            session: None,
            follow: None,
            threads: None,
            error: None,
        };
        match self {
            Self::Pong => wire.pong = Some(true),
            Self::Session(record, follow) => {
                wire.session = Some(record.clone());
                wire.follow.clone_from(follow);
            }
            Self::Done => {}
            Self::Threads(threads) => wire.threads = Some(threads.clone()),
            Self::Error(message) => {
                wire.ok = false;
                wire.error = Some(message.clone());
            }
        }
        serde_json::to_string(&wire).unwrap_or_default()
    }
}

impl FromStr for Response {
    type Err = ProtocolError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let wire: ResponseWire = serde_json::from_str(line)
            .map_err(|error| ProtocolError(format!("malformed response: {error}")))?;
        Ok(match wire {
            ResponseWire {
                ok: false, error, ..
            } => Self::Error(error.unwrap_or_else(|| "unspecified error".to_owned())),
            ResponseWire {
                session: Some(record),
                follow,
                ..
            } => Self::Session(record, follow),
            ResponseWire {
                threads: Some(threads),
                ..
            } => Self::Threads(threads),
            ResponseWire {
                pong: Some(true), ..
            } => Self::Pong,
            _ => Self::Done,
        })
    }
}

/// A malformed line, id, or an unsupported protocol version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(String);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Id, ProtocolError, Record, Request, Response};
    use crate::annotations::{Author, LineRange};

    fn record() -> Record {
        Record::new(
            "1700000000-42".parse().unwrap_or_else(|_| Id::mint()),
            PathBuf::from("/work"),
            Some(PathBuf::from("/run/fathomable/1700000000-42.sock")),
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
            r#"{"id":"1700000000-4000000","pid":4000000,"root":"/w","socket":"","started":1}"#;
        let record_dir = dirs.viewers_dir().join("1700000000-4000000");
        fs::create_dir_all(&record_dir)?;
        fs::write(record_dir.join(super::RECORD_FILE), dead)?;
        record().write(&dirs)?;
        assert_eq!(Record::sweep_dead(&dirs), 1);
        assert!(!dirs.viewers_dir().join("1700000000-4000000").exists());
        assert_eq!(Record::list(&dirs).len(), 1, "the live record stays");
        fs::remove_dir_all(&state)
    }

    #[test]
    fn ids_are_validated() -> Result<(), ProtocolError> {
        let id: Id = "1700000000-42".parse()?;
        assert_eq!(id.to_string(), "1700000000-42");
        assert_eq!("../etc".parse::<Id>().ok(), None);
        assert_eq!("12-".parse::<Id>().ok(), None);
        let minted = Id::mint();
        assert_eq!(minted.as_str().parse::<Id>().ok(), Some(minted.clone()));
        Ok(())
    }

    #[test]
    fn requests_round_trip() -> Result<(), ProtocolError> {
        let requests = [
            Request::Ping,
            Request::SessionInfo,
            Request::Open {
                path: PathBuf::from("README.md"),
                line: Some(3),
                end_line: None,
            },
            Request::Follow {
                paths: vec![PathBuf::from("a.md"), PathBuf::from("b/c.md")],
            },
            Request::ThreadsList {
                since: Some(7),
                path: None,
            },
            Request::ThreadReply {
                thread: serde_json::from_str(r#""1-2-3""#)
                    .map_err(|e| ProtocolError(e.to_string()))?,
                author: Author::Agent {
                    name: "reviewer".to_owned(),
                    client: Some("claude-code".to_owned()),
                    id: None,
                    kind: None,
                },
                body: "done".to_owned(),
                resolve: true,
                lines: Some(LineRange::new(4, 6)),
            },
        ];
        for request in requests {
            let line = request.to_line();
            assert!(line.starts_with(r#"{"v":2,"op":""#), "{line}");
            assert_eq!(line.parse::<Request>()?, request);
        }
        assert_eq!(Request::Ping.to_line(), r#"{"v":2,"op":"ping"}"#);
        Ok(())
    }

    #[test]
    fn version_gating_keeps_v0_liveness_only() {
        assert_eq!(
            r#"{"v":0,"op":"ping"}"#.parse::<Request>().ok(),
            Some(Request::Ping)
        );
        assert_eq!(
            r#"{"v":0,"op":"session_info"}"#.parse::<Request>().ok(),
            Some(Request::SessionInfo)
        );
        let too_old = r#"{"v":1,"op":"follow","paths":[]}"#.parse::<Request>().err();
        assert!(too_old.is_some_and(|e| e.to_string().contains("needs protocol version 2")));
        let too_new = r#"{"v":3,"op":"ping"}"#.parse::<Request>().err();
        assert!(too_new.is_some_and(|e| e.to_string().contains("version 3")));
        assert_eq!(r#"{"v":2,"op":"dance"}"#.parse::<Request>().ok(), None);
        assert_eq!("not json".parse::<Request>().ok(), None);
    }

    #[test]
    fn records_carry_an_optional_name() {
        let named = record().with_name(Some("left".to_owned()));
        assert_eq!(named.name(), Some("left"));
        assert!(named.is_called("left"));
        assert!(named.is_called("1700000000-42"));
        assert!(!named.is_called("right"));
        assert_eq!(record().with_name(Some("  ".to_owned())).name(), None);
        let line = Response::Session(named.clone(), None).to_line();
        assert!(line.contains(r#""name":"left""#), "{line}");
        assert!(!Response::Session(record(), None).to_line().contains("name"));
        assert_eq!(
            line.parse::<Response>().ok(),
            Some(Response::Session(named, None))
        );
    }

    #[test]
    fn responses_round_trip() -> Result<(), ProtocolError> {
        let responses = [
            Response::Pong,
            Response::Session(record(), None),
            Response::Session(record(), Some(super::FollowState { auto_jump: true })),
            Response::Done,
            Response::Threads(Vec::new()),
            Response::Error("nope".to_owned()),
        ];
        for response in responses {
            assert_eq!(response.to_line().parse::<Response>()?, response);
        }
        assert!(
            Response::Error("x".to_owned())
                .to_line()
                .starts_with(r#"{"ok":false"#)
        );
        assert_eq!(Response::Done.to_line(), r#"{"ok":true}"#);
        assert_eq!(
            record().socket(),
            Some(std::path::Path::new("/run/fathomable/1700000000-42.sock"))
        );
        Ok(())
    }
}
