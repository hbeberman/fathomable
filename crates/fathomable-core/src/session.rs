// @okf-doc: /decisions/0012-workspace-mode.md
//! Session records and the v1 socket protocol.
//!
//! A session is one running TUI bound to one workspace root (ADR 0003). It
//! writes a [`Record`] under `$XDG_STATE_HOME/fathomable/sessions/<id>/` and
//! listens on `$XDG_RUNTIME_DIR/fathomable/<id>.sock`. The socket speaks
//! line-delimited JSON: one [`Request`] per line, answered by one
//! [`Response`] per line. Every request carries `"v"`; version 1 (ADR 0014)
//! adds `open`, `follow`, `annotations_list`, and `thread_reply` to the v0
//! `ping` and `session_info` (ADR 0012), which are still accepted with
//! `"v":0`. The binary owns the socket and the state behind every operation;
//! this module owns the wire types.
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
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::XdgDirs;
use crate::annotations::{Author, Thread, ThreadId};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The oldest protocol version still accepted, for `ping` and `session_info`.
const OLDEST_VERSION: u32 = 0;

/// File name of the record inside a session directory.
pub const RECORD_FILE: &str = "session.json";

/// A session identifier: `<unix-seconds>-<pid>`, unique per host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    /// Mint an id for the current process.
    #[must_use]
    pub fn mint() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
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

/// What a running session writes about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    id: Id,
    pid: u32,
    root: PathBuf,
    socket: PathBuf,
    started: u64,
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
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
        }
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
        dirs.sessions_dir().join(self.id.as_str())
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
    /// a log line. A missing sessions directory yields an empty list.
    #[must_use]
    pub fn list(dirs: &XdgDirs) -> Vec<Self> {
        let mut records = Vec::new();
        let Ok(entries) = fs::read_dir(dirs.sessions_dir()) else {
            return records;
        };
        for entry in entries.flatten() {
            let path = entry.path().join(RECORD_FILE);
            match Self::read(&path) {
                Ok(record) => records.push(record),
                Err(error) => tracing::warn!(%error, path = %path.display(), "bad session record"),
            }
        }
        records.sort_by_key(|record| record.started);
        records
    }

    /// Remove records whose process is gone. Returns how many were removed.
    pub fn sweep_dead(dirs: &XdgDirs) -> usize {
        let mut removed = 0;
        for record in Self::list(dirs) {
            if record.is_alive() {
                continue;
            }
            match record.remove(dirs) {
                Ok(()) => {
                    removed += 1;
                    if let Some(socket) = record.socket() {
                        let _ = fs::remove_file(socket);
                    }
                    tracing::info!(id = %record.id, pid = record.pid, "removed dead session");
                }
                Err(error) => tracing::warn!(%error, id = %record.id, "cannot remove session"),
            }
        }
        removed
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
    AnnotationsList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    /// Append a reply to a thread, optionally resolving it.
    ThreadReply {
        thread: ThreadId,
        author: Author,
        body: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resolve: bool,
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
    /// Answer to [`Request::AnnotationsList`].
    Threads(Vec<Thread>),
    /// The request was refused; the text says why.
    Error(String),
}

/// Follow-mode state reported with `session_info` (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowState {
    /// The effective `follow.source` name.
    pub follow_source: String,
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
    use crate::annotations::Author;

    fn record() -> Record {
        Record::new(
            "1700000000-42".parse().unwrap_or_else(|_| Id::mint()),
            PathBuf::from("/work"),
            Some(PathBuf::from("/run/fathomable/1700000000-42.sock")),
        )
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
            Request::AnnotationsList {
                since: Some(7),
                path: None,
            },
            Request::ThreadReply {
                thread: serde_json::from_str(r#""1-2-3""#)
                    .map_err(|e| ProtocolError(e.to_string()))?,
                author: Author::Agent {
                    name: "reviewer".to_owned(),
                    client: Some("claude-code".to_owned()),
                },
                body: "done".to_owned(),
                resolve: true,
            },
        ];
        for request in requests {
            let line = request.to_line();
            assert!(line.starts_with(r#"{"v":1,"op":""#), "{line}");
            assert_eq!(line.parse::<Request>()?, request);
        }
        assert_eq!(Request::Ping.to_line(), r#"{"v":1,"op":"ping"}"#);
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
        let too_old = r#"{"v":0,"op":"follow","paths":[]}"#.parse::<Request>().err();
        assert!(too_old.is_some_and(|e| e.to_string().contains("needs protocol version 1")));
        let too_new = r#"{"v":2,"op":"ping"}"#.parse::<Request>().err();
        assert!(too_new.is_some_and(|e| e.to_string().contains("version 2")));
        assert_eq!(r#"{"v":1,"op":"dance"}"#.parse::<Request>().ok(), None);
        assert_eq!("not json".parse::<Request>().ok(), None);
    }

    #[test]
    fn responses_round_trip() -> Result<(), ProtocolError> {
        let responses = [
            Response::Pong,
            Response::Session(record(), None),
            Response::Session(
                record(),
                Some(super::FollowState {
                    follow_source: "workspace".to_owned(),
                    auto_jump: true,
                }),
            ),
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
