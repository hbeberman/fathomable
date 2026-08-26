// @okf-doc: /decisions/0012-workspace-mode.md
//! Session records and the provisional v0 socket protocol.
//!
//! A session is one running TUI bound to one workspace root (ADR 0003). It
//! writes a [`Record`] under `$XDG_STATE_HOME/fathomable/sessions/<id>/` and
//! listens on `$XDG_RUNTIME_DIR/fathomable/<id>.sock`. The socket speaks
//! line-delimited JSON; protocol version 0 (ADR 0012) knows only `ping` and
//! `session_info`, and every request carries `"v"` so later versions can
//! refuse old clients with a clear error. The binary owns the socket; this
//! module owns the wire types and [`answer`], which is pure.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::session::{Request, Response};
//!
//! let request: Request = r#"{"v":0,"op":"ping"}"#.parse()?;
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

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 0;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Liveness check.
    Ping,
    /// Ask for the session record.
    SessionInfo,
}

#[derive(Serialize, Deserialize)]
struct RequestWire {
    v: u32,
    op: String,
}

impl Request {
    /// The request as one JSON line without the newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let op = match self {
            Self::Ping => "ping",
            Self::SessionInfo => "session_info",
        };
        serde_json::to_string(&RequestWire {
            v: PROTOCOL_VERSION,
            op: op.to_owned(),
        })
        .unwrap_or_default()
    }
}

impl FromStr for Request {
    type Err = ProtocolError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let wire: RequestWire = serde_json::from_str(line)
            .map_err(|error| ProtocolError(format!("malformed request: {error}")))?;
        if wire.v != PROTOCOL_VERSION {
            return Err(ProtocolError(format!(
                "unsupported protocol version {} (this session speaks {PROTOCOL_VERSION})",
                wire.v
            )));
        }
        match wire.op.as_str() {
            "ping" => Ok(Self::Ping),
            "session_info" => Ok(Self::SessionInfo),
            other => Err(ProtocolError(format!("unknown operation `{other}`"))),
        }
    }
}

/// A response over the session socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// Answer to [`Request::Ping`].
    Pong,
    /// Answer to [`Request::SessionInfo`].
    Session(Record),
    /// The request was refused; the text says why.
    Error(String),
}

#[derive(Serialize, Deserialize)]
struct ResponseWire {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pong: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Response {
    /// The response as one JSON line without the newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let wire = match self {
            Self::Pong => ResponseWire {
                ok: true,
                pong: Some(true),
                session: None,
                error: None,
            },
            Self::Session(record) => ResponseWire {
                ok: true,
                pong: None,
                session: Some(record.clone()),
                error: None,
            },
            Self::Error(message) => ResponseWire {
                ok: false,
                pong: None,
                session: None,
                error: Some(message.clone()),
            },
        };
        serde_json::to_string(&wire).unwrap_or_default()
    }
}

impl FromStr for Response {
    type Err = ProtocolError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let wire: ResponseWire = serde_json::from_str(line)
            .map_err(|error| ProtocolError(format!("malformed response: {error}")))?;
        match wire {
            ResponseWire {
                ok: false, error, ..
            } => Ok(Self::Error(
                error.unwrap_or_else(|| "unspecified error".to_owned()),
            )),
            ResponseWire {
                session: Some(record),
                ..
            } => Ok(Self::Session(record)),
            ResponseWire {
                pong: Some(true), ..
            } => Ok(Self::Pong),
            _ => Err(ProtocolError("response carries no payload".to_owned())),
        }
    }
}

/// Answer one request line on behalf of `record`. Never fails: a bad line
/// becomes [`Response::Error`].
#[must_use]
pub fn answer(line: &str, record: &Record) -> Response {
    match line.parse::<Request>() {
        Ok(Request::Ping) => Response::Pong,
        Ok(Request::SessionInfo) => Response::Session(record.clone()),
        Err(error) => Response::Error(error.to_string()),
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

    use super::{Id, ProtocolError, Record, Request, Response, answer};

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
    fn requests_round_trip_and_reject_other_versions() -> Result<(), ProtocolError> {
        for request in [Request::Ping, Request::SessionInfo] {
            assert_eq!(request.to_line().parse::<Request>()?, request);
        }
        let error = r#"{"v":1,"op":"ping"}"#.parse::<Request>().err();
        assert!(error.is_some_and(|e| e.to_string().contains("version 1")));
        assert_eq!(r#"{"v":0,"op":"open"}"#.parse::<Request>().ok(), None);
        assert_eq!("not json".parse::<Request>().ok(), None);
        Ok(())
    }

    #[test]
    fn answer_covers_every_case() -> Result<(), ProtocolError> {
        let record = record();
        assert_eq!(answer(r#"{"v":0,"op":"ping"}"#, &record), Response::Pong);
        let info = answer(r#"{"v":0,"op":"session_info"}"#, &record);
        assert_eq!(info, Response::Session(record.clone()));
        assert_eq!(info.to_line().parse::<Response>()?, info);
        let error = answer("{}", &record);
        assert!(matches!(&error, Response::Error(_)));
        assert!(error.to_line().starts_with(r#"{"ok":false"#));
        assert_eq!(error.to_line().parse::<Response>()?, error);
        assert_eq!(
            record.socket(),
            Some(std::path::Path::new("/run/fathomable/1700000000-42.sock"))
        );
        Ok(())
    }
}
