// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Annotation threads, content anchors, and the append-only JSONL store.
//!
//! A [`Thread`] is one user comment on a [`LineRange`] of a workspace file
//! plus its replies (ADR 0005). Threads are keyed to content, not position:
//! an [`Anchor`] records a short hash of every annotated line and of the
//! lines immediately above and below, and [`Thread::locate`] finds the range
//! again after the file changes. When the lines are gone the thread is
//! [`Placement::Detached`] at its last known range rather than lost.
//!
//! Every change is one JSON line appended to `threads.jsonl` under the
//! workspace's state directory (ADR 0013); [`Store::open`] folds the file
//! back into threads. Timestamps are supplied by the caller so the module
//! stays pure and testable.
//!
//! # Examples
//!
//! ```no_run
//! use std::path::Path;
//! use fathomable_core::annotations::{Draft, LineRange, Store};
//!
//! let mut store = Store::open("/tmp/threads.jsonl")?;
//! let text = "# Title\n\nalpha\nbeta\n";
//! let draft = Draft::new(Path::new("README.md"), LineRange::new(3, 4), "rename these");
//! let id = store.annotate(draft, text, 1_700_000_000)?;
//! assert_eq!(store.thread(&id).map(|t| t.snippet()), Some("alpha\nbeta"));
//! # Ok::<(), fathomable_core::annotations::StoreError>(())
//! ```

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The record format version written in every event line.
pub const FORMAT_VERSION: u32 = 2;

/// File name of the thread store inside a workspace state directory.
pub const THREADS_FILE: &str = "threads.jsonl";

/// Hex characters kept from a SHA-256 digest; 64 bits is plenty to tell
/// lines of one file apart and keeps the JSONL readable.
const HASH_CHARS: usize = 16;

/// Short content hash of one line, ignoring trailing whitespace.
#[must_use]
pub fn line_hash(line: &str) -> String {
    short_hash(line.trim_end().as_bytes())
}

/// Sixteen hex characters of the SHA-256 of `bytes`.
#[must_use]
pub fn short_hash(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(HASH_CHARS);
    for byte in digest.iter().take(HASH_CHARS / 2) {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// An inclusive range of 1-based source lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineRange {
    start: usize,
    end: usize,
}

impl LineRange {
    /// A range from `a` to `b` inclusive, in either order; 0 is clamped to 1.
    #[must_use]
    pub fn new(a: usize, b: usize) -> Self {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        Self {
            start: start.max(1),
            end: end.max(1),
        }
    }

    /// First line, 1-based.
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
    }

    /// Last line, 1-based and inclusive.
    #[must_use]
    pub fn end(&self) -> usize {
        self.end
    }

    /// Number of lines covered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start + 1
    }

    /// Always `false`: a range covers at least one line.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether `line` lies inside the range.
    #[must_use]
    pub fn contains(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

/// Content hashes that re-locate an annotated range after edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Anchor {
    /// Hash `range` of `text` plus one line of context on each side.
    ///
    /// Returns `None` when the range runs past the end of the text.
    #[must_use]
    pub fn capture(text: &str, range: LineRange) -> Option<Self> {
        let lines: Vec<&str> = text.lines().collect();
        if range.end > lines.len() {
            return None;
        }
        let hashes = lines[range.start - 1..range.end]
            .iter()
            .map(|line| line_hash(line))
            .collect();
        Some(Self {
            lines: hashes,
            before: (range.start > 1).then(|| line_hash(lines[range.start - 2])),
            after: lines.get(range.end).map(|line| line_hash(line)),
        })
    }

    /// Find the range in `text` whose lines hash like this anchor.
    ///
    /// When several windows match, the one whose surrounding lines also
    /// match wins; ties go to the window closest to `hint`. `None` means the
    /// exact lines no longer exist.
    #[must_use]
    pub fn locate(&self, text: &str, hint: LineRange) -> Option<LineRange> {
        let n = self.lines.len();
        if n == 0 {
            return None;
        }
        let hashes: Vec<String> = text.lines().map(line_hash).collect();
        if hashes.len() < n {
            return None;
        }
        let mut best: Option<(usize, usize, usize)> = None;
        for (start, window) in hashes.windows(n).enumerate() {
            if window != self.lines.as_slice() {
                continue;
            }
            let before_ok = match &self.before {
                Some(hash) => start > 0 && hashes[start - 1] == *hash,
                None => start == 0,
            };
            let after_ok = match &self.after {
                Some(hash) => hashes.get(start + n) == Some(hash),
                None => start + n == hashes.len(),
            };
            let context = usize::from(before_ok) + usize::from(after_ok);
            let distance = (start + 1).abs_diff(hint.start);
            let candidate = (context, distance, start);
            let better = best.is_none_or(|(c, d, _)| context > c || (context == c && distance < d));
            if better {
                best = Some(candidate);
            }
        }
        best.map(|(_, _, start)| LineRange::new(start + 1, start + n))
    }
}

/// Where a thread sits in the current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The annotated lines were found here.
    Anchored(LineRange),
    /// The lines were found here, but they are the replacement of the
    /// lines the comment was written on (ADR 0019) and nobody has
    /// answered since.
    Edited(LineRange),
    /// The lines are gone; this is the last known range.
    Detached(LineRange),
}

impl Placement {
    /// The range to draw at, anchored or not.
    #[must_use]
    pub fn range(&self) -> LineRange {
        match self {
            Self::Anchored(range) | Self::Edited(range) | Self::Detached(range) => *range,
        }
    }

    /// Whether the lines under the thread were rewritten since the last
    /// reply or resolution.
    #[must_use]
    pub fn is_edited(&self) -> bool {
        matches!(self, Self::Edited(_))
    }

    /// Whether the annotated lines no longer exist.
    #[must_use]
    pub fn is_detached(&self) -> bool {
        matches!(self, Self::Detached(_))
    }
}

/// Identifier of a thread, unique within one store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(String);

impl ThreadId {
    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who wrote a reply or resolved a thread.
///
/// An agent carries the name it goes by plus, when the reply arrived over
/// MCP, the client implementation the host reported (ADR 0014). On the wire
/// an author is a plain string unless it has a client, in which case it is
/// `{"name":..,"client":..}`; older files therefore still load.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "AuthorWire", into = "AuthorWire")]
pub enum Author {
    /// The person at the keyboard.
    User,
    /// An agent.
    Agent {
        /// The persona it declared, or the client name when it declared none.
        name: String,
        /// The MCP client that carried the reply, when known.
        client: Option<String>,
    },
}

impl Author {
    /// An agent known only by `name`.
    #[must_use]
    pub fn agent(name: impl Into<String>) -> Self {
        Self::Agent {
            name: name.into(),
            client: None,
        }
    }

    /// Whether this is the user rather than an agent.
    #[must_use]
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User)
    }

    /// The short label: `user` or the agent's name, without the client.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Agent { name, .. } => name,
        }
    }
}

impl fmt::Display for Author {
    /// `user`, the agent name, or `name (client)` when the two differ.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Agent {
                name,
                client: Some(client),
            } if client != name => write!(f, "{name} ({client})"),
            Self::Agent { name, .. } => f.write_str(name),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum AuthorWire {
    Name(String),
    Full {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
    },
}

impl From<AuthorWire> for Author {
    fn from(wire: AuthorWire) -> Self {
        match wire {
            AuthorWire::Name(name) if name == "user" => Self::User,
            AuthorWire::Name(name) => Self::agent(name),
            AuthorWire::Full { name, client } => Self::Agent { name, client },
        }
    }
}

impl From<Author> for AuthorWire {
    fn from(author: Author) -> Self {
        match author {
            Author::User => Self::Name("user".to_owned()),
            Author::Agent { name, client: None } => Self::Name(name),
            Author::Agent { name, client } => Self::Full { name, client },
        }
    }
}

/// One reply in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    author: Author,
    created: u64,
    body: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    proposed_resolved: bool,
}

impl Reply {
    /// A reply by `author` at `created` (Unix seconds).
    #[must_use]
    pub fn new(author: Author, created: u64, body: impl Into<String>) -> Self {
        Self {
            author,
            created,
            body: body.into(),
            proposed_resolved: false,
        }
    }

    /// Mark the reply as proposing that the thread be resolved.
    #[must_use]
    pub fn proposing_resolution(mut self) -> Self {
        self.proposed_resolved = true;
        self
    }

    /// Who wrote it.
    #[must_use]
    pub fn author(&self) -> &Author {
        &self.author
    }

    /// When it was written, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// The reply text.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Whether the author proposed resolving the thread.
    #[must_use]
    pub fn proposes_resolution(&self) -> bool {
        self.proposed_resolved
    }
}

/// Whether a thread is open or how it was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Awaiting action.
    Open,
    /// Resolved by the user.
    Resolved,
    /// Force-resolved by an agent (ADR 0005 `auto_resolved`).
    AutoResolved,
}

/// An annotation with its replies and status.
///
/// Serializes as a plain object so it can travel over the session socket
/// (ADR 0014); the JSONL file stores events, not threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    id: ThreadId,
    path: PathBuf,
    range: LineRange,
    snippet: String,
    anchor: Anchor,
    created: u64,
    updated: u64,
    comment: String,
    replies: Vec<Reply>,
    status: Status,
    /// When the thread was last re-anchored to rewritten lines, until the
    /// user replies, resolves, or reopens (ADR 0019).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edited: Option<u64>,
    /// The `HEAD` commit the annotation was written against, when the
    /// workspace had one (ADR 0024); `None` reads as unscoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

impl Thread {
    /// The thread id.
    #[must_use]
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    /// Workspace-relative path of the annotated file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The range as it was when the annotation was made.
    #[must_use]
    pub fn range(&self) -> LineRange {
        self.range
    }

    /// The annotated source lines, without the trailing newline.
    #[must_use]
    pub fn snippet(&self) -> &str {
        &self.snippet
    }

    /// The content anchor.
    #[must_use]
    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    /// When the annotation was made, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// When the thread last changed (reply, resolve, reopen), in Unix
    /// seconds; equals [`created`](Self::created) until then.
    #[must_use]
    pub fn updated(&self) -> u64 {
        self.updated
    }

    /// The user's comment.
    #[must_use]
    pub fn comment(&self) -> &str {
        &self.comment
    }

    /// Replies in the order they were made.
    #[must_use]
    pub fn replies(&self) -> &[Reply] {
        &self.replies
    }

    /// Open, resolved, or auto-resolved.
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    /// When the lines under the thread were last rewritten, if the user
    /// has not answered since.
    #[must_use]
    pub fn edited(&self) -> Option<u64> {
        self.edited
    }

    /// The `HEAD` commit the annotation was written against, if any.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Whether the thread is open and its newest message is not the user's.
    ///
    /// Such a thread is *waiting* on the user (ADR 0030): an agent replied
    /// last, and only the user's reply, resolve, or reopen ends the wait.
    /// A resolved thread never waits.
    #[must_use]
    pub fn awaits_user(&self) -> bool {
        self.status == Status::Open
            && self
                .replies
                .last()
                .is_some_and(|reply| !reply.author().is_user())
    }

    /// Where the thread sits in `text` now.
    #[must_use]
    pub fn locate(&self, text: &str) -> Placement {
        match (self.anchor.locate(text, self.range), self.edited) {
            (Some(range), Some(_)) => Placement::Edited(range),
            (Some(range), None) => Placement::Anchored(range),
            (None, _) => Placement::Detached(self.range),
        }
    }
}

/// What the user supplies to start a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    path: PathBuf,
    range: LineRange,
    comment: String,
    commit: Option<String>,
}

impl Draft {
    /// A comment on `range` of the workspace-relative `path`.
    #[must_use]
    pub fn new(path: &Path, range: LineRange, comment: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            range,
            comment: comment.into(),
            commit: None,
        }
    }

    /// Record the `HEAD` commit the comment is written against
    /// (ADR 0024), so the thread shows only where that commit is
    /// reachable.
    #[must_use]
    pub fn at_commit(mut self, commit: Option<String>) -> Self {
        self.commit = commit;
        self
    }
}

/// Which threads the current `HEAD` shows (ADR 0024).
///
/// A thread written against a commit is visible only while that commit is
/// `HEAD` or one of its ancestors; a thread without a commit, or any thread
/// when the workspace has no `HEAD`, is always visible.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    reachable: Option<HashSet<String>>,
}

impl Scope {
    /// A scope that shows every thread: no git, or no `HEAD` yet.
    #[must_use]
    pub fn unscoped() -> Self {
        Self { reachable: None }
    }

    /// A scope over the commits reachable from `HEAD`. The set need only
    /// hold the commits that threads mention; see
    /// [`Workspace::reachable`](crate::workspace::Workspace::reachable).
    #[must_use]
    pub fn reachable(commits: HashSet<String>) -> Self {
        Self {
            reachable: Some(commits),
        }
    }

    /// Whether `thread` is on the current work.
    #[must_use]
    pub fn includes(&self, thread: &Thread) -> bool {
        match (&self.reachable, thread.commit()) {
            (Some(reachable), Some(commit)) => reachable.contains(commit),
            _ => true,
        }
    }
}

/// One line of the JSONL file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Annotate {
        v: u32,
        id: ThreadId,
        path: PathBuf,
        range: LineRange,
        snippet: String,
        anchor: Anchor,
        created: u64,
        comment: String,
        /// Absent in version 1 records, which read as unscoped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    Reply {
        v: u32,
        thread: ThreadId,
        #[serde(flatten)]
        reply: Reply,
    },
    Resolve {
        v: u32,
        thread: ThreadId,
        by: Author,
        created: u64,
    },
    Reopen {
        v: u32,
        thread: ThreadId,
        created: u64,
    },
    /// The thread's lines were rewritten and it now sits on the
    /// replacement (ADR 0019).
    Relocate {
        v: u32,
        thread: ThreadId,
        range: LineRange,
        anchor: Anchor,
        created: u64,
    },
    /// The file was renamed and the thread now lives at `path`, range
    /// and anchor unchanged (ADR 0028).
    Move {
        v: u32,
        thread: ThreadId,
        path: PathBuf,
        created: u64,
    },
}

/// The threads of one workspace, backed by an append-only JSONL file.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    threads: Vec<Thread>,
}

impl Store {
    /// Load the store at `path`, or start empty when the file is missing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the file cannot be read, a line is not a
    /// known event, or an event refers to a thread the file never created.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let mut store = Self {
            path,
            threads: Vec::new(),
        };
        let text = match fs::read_to_string(&store.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(store),
            Err(error) => return Err(StoreError::io(&store.path, error)),
        };
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(line)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
            store
                .apply(event)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
        }
        tracing::debug!(path = %store.path.display(), threads = store.threads.len(), "loaded threads");
        Ok(store)
    }

    /// Where the JSONL file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every thread, oldest first.
    #[must_use]
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    /// The distinct commits threads were written against.
    pub fn commits(&self) -> impl Iterator<Item = &str> + '_ {
        let mut found: Vec<&str> = Vec::new();
        self.threads
            .iter()
            .filter_map(Thread::commit)
            .filter(move |commit| {
                let new = !found.contains(commit);
                if new {
                    found.push(commit);
                }
                new
            })
    }

    /// Threads on the workspace-relative `path`, oldest first.
    pub fn for_path<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Thread> + 'a {
        self.threads
            .iter()
            .filter(move |thread| thread.path == path)
    }

    /// The distinct paths that have an open thread, in first-seen order.
    pub fn open_paths(&self) -> impl Iterator<Item = &Path> + '_ {
        let mut found: Vec<&Path> = Vec::new();
        self.threads
            .iter()
            .filter(|thread| thread.status == Status::Open)
            .map(|thread| thread.path.as_path())
            .filter(move |path| {
                let new = !found.contains(path);
                if new {
                    found.push(path);
                }
                new
            })
    }

    /// The thread with `id`, if any.
    #[must_use]
    pub fn thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.threads.iter().find(|thread| thread.id == *id)
    }

    /// Start a thread from `draft` over the file's current `text` at `now`
    /// (Unix seconds); the snippet and anchor are captured from `text`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the range runs past the end of `text` or
    /// the file cannot be appended to.
    pub fn annotate(&mut self, draft: Draft, text: &str, now: u64) -> Result<ThreadId, StoreError> {
        let anchor = Anchor::capture(text, draft.range).ok_or(StoreError {
            kind: ErrorKind::BadRange(draft.range),
        })?;
        let snippet = text
            .lines()
            .skip(draft.range.start - 1)
            .take(draft.range.len())
            .collect::<Vec<_>>()
            .join("\n");
        let id = ThreadId(format!(
            "{now}-{}-{}",
            std::process::id(),
            self.threads.len() + 1
        ));
        self.commit(Event::Annotate {
            v: FORMAT_VERSION,
            id: id.clone(),
            path: draft.path,
            range: draft.range,
            snippet,
            anchor,
            created: now,
            comment: draft.comment,
            commit: draft.commit,
        })?;
        Ok(id)
    }

    /// Append `reply` to the thread `id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn reply(&mut self, id: &ThreadId, reply: Reply) -> Result<(), StoreError> {
        self.commit(Event::Reply {
            v: FORMAT_VERSION,
            thread: id.clone(),
            reply,
        })
    }

    /// Resolve the thread `id`; an agent author marks it auto-resolved.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn resolve(&mut self, id: &ThreadId, by: Author, now: u64) -> Result<(), StoreError> {
        self.commit(Event::Resolve {
            v: FORMAT_VERSION,
            thread: id.clone(),
            by,
            created: now,
        })
    }

    /// Reopen the thread `id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn reopen(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
        self.commit(Event::Reopen {
            v: FORMAT_VERSION,
            thread: id.clone(),
            created: now,
        })
    }

    /// Move the thread `id` onto `range` of `text`, the lines that replaced
    /// the ones it was written on (ADR 0019). The thread reads as edited
    /// until the user replies, resolves, or reopens it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown, the range runs
    /// past the end of `text`, or the file cannot be appended to.
    pub fn relocate(
        &mut self,
        id: &ThreadId,
        range: LineRange,
        text: &str,
        now: u64,
    ) -> Result<(), StoreError> {
        let anchor = Anchor::capture(text, range).ok_or(StoreError {
            kind: ErrorKind::BadRange(range),
        })?;
        self.commit(Event::Relocate {
            v: FORMAT_VERSION,
            thread: id.clone(),
            range,
            anchor,
            created: now,
        })
    }

    /// Move the thread `id` to `path`, the file's name after a rename
    /// (ADR 0028). Its range and anchor are untouched, so it locates in
    /// the renamed file exactly as it did before.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn move_path(&mut self, id: &ThreadId, path: &Path, now: u64) -> Result<(), StoreError> {
        self.commit(Event::Move {
            v: FORMAT_VERSION,
            thread: id.clone(),
            path: path.to_path_buf(),
            created: now,
        })
    }

    /// Apply an event in memory, then append it; the file is only written
    /// when the event is valid.
    fn commit(&mut self, event: Event) -> Result<(), StoreError> {
        let line = serde_json::to_string(&event).map_err(|error| StoreError {
            kind: ErrorKind::Parse(0, error.to_string()),
        })?;
        self.apply(event)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| StoreError::io(&self.path, error))?;
        writeln!(file, "{line}").map_err(|error| StoreError::io(&self.path, error))
    }

    fn apply(&mut self, event: Event) -> Result<(), StoreError> {
        match event {
            Event::Annotate {
                id,
                path,
                range,
                snippet,
                anchor,
                created,
                comment,
                commit,
                ..
            } => {
                self.threads.push(Thread {
                    id,
                    path,
                    range,
                    snippet,
                    anchor,
                    created,
                    updated: created,
                    comment,
                    replies: Vec::new(),
                    status: Status::Open,
                    edited: None,
                    commit,
                });
            }
            Event::Reply { thread, reply, .. } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(reply.created);
                if reply.author.is_user() {
                    thread.edited = None;
                }
                thread.replies.push(reply);
            }
            Event::Resolve {
                thread,
                by,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                if by.is_user() {
                    thread.edited = None;
                }
                thread.status = if by.is_user() {
                    Status::Resolved
                } else {
                    Status::AutoResolved
                };
            }
            Event::Reopen {
                thread, created, ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.status = Status::Open;
                thread.edited = None;
            }
            Event::Relocate {
                thread,
                range,
                anchor,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.range = range;
                thread.anchor = anchor;
                thread.edited = Some(created);
            }
            Event::Move {
                thread,
                path,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.path = path;
            }
        }
        Ok(())
    }

    fn thread_mut(&mut self, id: &ThreadId) -> Result<&mut Thread, StoreError> {
        self.threads
            .iter_mut()
            .find(|thread| thread.id == *id)
            .ok_or_else(|| StoreError {
                kind: ErrorKind::UnknownThread(id.clone()),
            })
    }
}

#[derive(Debug)]
enum ErrorKind {
    Io(PathBuf, io::Error),
    Parse(usize, String),
    UnknownThread(ThreadId),
    BadRange(LineRange),
}

/// Why the store could not be read or written.
#[derive(Debug)]
pub struct StoreError {
    kind: ErrorKind,
}

impl StoreError {
    fn io(path: &Path, error: io::Error) -> Self {
        Self {
            kind: ErrorKind::Io(path.to_path_buf(), error),
        }
    }

    fn parse(line: usize, message: String) -> Self {
        Self {
            kind: ErrorKind::Parse(line, message),
        }
    }

    /// Whether the cause was an I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(..))
    }

    /// Whether the cause was a malformed line; carries its 1-based number.
    #[must_use]
    pub fn parse_line(&self) -> Option<usize> {
        match self.kind {
            ErrorKind::Parse(line, _) => Some(line),
            _ => None,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Io(path, error) => write!(f, "{}: {error}", path.display()),
            ErrorKind::Parse(line, message) => write!(f, "threads.jsonl line {line}: {message}"),
            ErrorKind::UnknownThread(id) => write!(f, "unknown thread {id}"),
            ErrorKind::BadRange(range) => write!(f, "lines {range} are past the end of the file"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(_, error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        Anchor, Author, Draft, LineRange, Placement, Reply, Scope, Status, Store, StoreError,
        Thread, ThreadId, line_hash,
    };

    const TEXT: &str = "# Title\n\nalpha\nbeta\ngamma\n\ndelta\n";

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fathomable-annotations-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir.join("nested").join("threads.jsonl"))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent().and_then(Path::parent) {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn line_hash_ignores_trailing_whitespace_only() {
        assert_eq!(line_hash("alpha"), line_hash("alpha  \t"));
        assert_ne!(line_hash("alpha"), line_hash(" alpha"));
        assert_eq!(line_hash("x").len(), 16);
    }

    #[test]
    fn line_range_orders_and_displays() {
        let range = LineRange::new(5, 3);
        assert_eq!((range.start(), range.end(), range.len()), (3, 5, 3));
        assert!(range.contains(4) && !range.contains(6));
        assert_eq!(range.to_string(), "3-5");
        assert_eq!(LineRange::new(0, 0).to_string(), "1");
    }

    #[test]
    fn thread_waits_after_agent_reply_until_user_answers() -> Result<(), StoreError> {
        let file = TempFile::new("waiting");
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(3, 3), "why?"),
            TEXT,
            10,
        )?;
        let waiting = |store: &Store| store.thread(&id).is_some_and(Thread::awaits_user);
        assert!(!waiting(&store), "a fresh comment is the user's own");
        store.reply(&id, Reply::new(Author::agent("claude"), 11, "because"))?;
        assert!(waiting(&store));
        store.reply(&id, Reply::new(Author::User, 12, "ok"))?;
        assert!(!waiting(&store));
        store.reply(&id, Reply::new(Author::agent("claude"), 13, "done"))?;
        store.resolve(&id, Author::User, 14)?;
        assert!(!waiting(&store), "a resolved thread never waits");
        Ok(())
    }

    #[test]
    fn anchor_follows_moved_lines_and_detaches_when_gone() -> Result<(), String> {
        let range = LineRange::new(3, 4);
        let anchor = Anchor::capture(TEXT, range).ok_or("capture")?;
        assert_eq!(anchor.locate(TEXT, range), Some(range));
        let moved = "# Title\n\nintro\nmore intro\n\nalpha\nbeta\ngamma\n";
        assert_eq!(anchor.locate(moved, range), Some(LineRange::new(6, 7)));
        let edited = "# Title\n\nalpha\nBETA\ngamma\n";
        assert_eq!(anchor.locate(edited, range), None);
        assert_eq!(Anchor::capture(TEXT, LineRange::new(7, 9)), None);
        Ok(())
    }

    #[test]
    fn relocate_moves_a_thread_and_the_user_acknowledges_the_edit() -> Result<(), StoreError> {
        let file = TempFile::new("relocate");
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(Path::new("README.md"), LineRange::new(3, 4), "rename"),
            TEXT,
            100,
        )?;
        let edited = "# Title\n\nalpha\nBETA\ngamma\n";
        store.relocate(&id, LineRange::new(3, 4), edited, 110)?;
        let again = Store::open(&file.0)?;
        assert_eq!(again.threads(), store.threads());
        let thread = again
            .thread(&id)
            .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
        assert_eq!(thread.edited(), Some(110));
        assert_eq!(thread.updated(), 110);
        assert_eq!(
            thread.snippet(),
            "alpha\nbeta",
            "the snippet stays as commented on"
        );
        assert!(thread.locate(edited).is_edited());
        assert!(thread.locate(TEXT).is_detached());
        // An agent's reply leaves the edit flag; the user's clears it.
        store.reply(&id, Reply::new(Author::agent("claude"), 111, "fixed"))?;
        assert!(store.thread(&id).is_some_and(|t| t.edited().is_some()));
        store.reply(&id, Reply::new(Author::User, 112, "ok"))?;
        assert!(store.thread(&id).is_some_and(|t| t.edited().is_none()));
        assert!(
            store
                .thread(&id)
                .is_some_and(|t| !t.locate(edited).is_edited())
        );
        // A bad range is an error and writes nothing.
        assert!(
            store
                .relocate(&id, LineRange::new(8, 9), edited, 113)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn move_path_carries_a_thread_to_the_renamed_file() -> Result<(), StoreError> {
        let file = TempFile::new("move");
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(Path::new("old.md"), LineRange::new(3, 4), "rename"),
            TEXT,
            100,
        )?;
        store.move_path(&id, Path::new("docs/new.md"), 120)?;
        let again = Store::open(&file.0)?;
        assert_eq!(again.threads(), store.threads());
        let thread = again
            .thread(&id)
            .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
        assert_eq!(thread.path(), Path::new("docs/new.md"));
        assert_eq!(thread.updated(), 120, "since polling sees the move");
        assert_eq!(thread.range(), LineRange::new(3, 4));
        assert_eq!(
            thread.locate(TEXT).range(),
            LineRange::new(3, 4),
            "the anchor still finds its lines"
        );
        assert!(again.for_path(Path::new("old.md")).next().is_none());
        let log = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
        assert!(log.contains(r#""event":"move""#));
        let unknown = ThreadId("nope".to_owned());
        assert!(store.move_path(&unknown, Path::new("x"), 1).is_err());
        Ok(())
    }

    #[test]
    fn anchor_prefers_matching_context_then_nearest() -> Result<(), String> {
        // Two identical "item" lines; context picks the one after "two".
        let text = "one\nitem\ntwo\nitem\nthree\n";
        let anchor = Anchor::capture(text, LineRange::new(4, 4)).ok_or("capture")?;
        let shifted = "zero\none\nitem\ntwo\nitem\nthree\n";
        assert_eq!(
            anchor.locate(shifted, LineRange::new(4, 4)),
            Some(LineRange::new(5, 5))
        );
        // No context matches anywhere: nearest to the hint wins.
        let stripped = "item\nx\nitem\ny\nitem\n";
        assert_eq!(
            anchor.locate(stripped, LineRange::new(4, 4)),
            Some(LineRange::new(3, 3))
        );
        Ok(())
    }

    #[test]
    fn authors_serialize_as_strings_unless_they_carry_a_client() -> Result<(), serde_json::Error> {
        let plain = Author::agent("claude");
        assert_eq!(serde_json::to_string(&plain)?, r#""claude""#);
        assert_eq!(serde_json::from_str::<Author>(r#""user""#)?, Author::User);
        let full = Author::Agent {
            name: "reviewer".to_owned(),
            client: Some("claude-code".to_owned()),
        };
        let json = serde_json::to_string(&full)?;
        assert_eq!(json, r#"{"name":"reviewer","client":"claude-code"}"#);
        assert_eq!(serde_json::from_str::<Author>(&json)?, full);
        assert_eq!(full.to_string(), "reviewer (claude-code)");
        let same = Author::Agent {
            name: "claude-code".to_owned(),
            client: Some("claude-code".to_owned()),
        };
        assert_eq!(same.to_string(), "claude-code");
        Ok(())
    }

    #[test]
    fn store_round_trips_threads_replies_and_status() -> Result<(), StoreError> {
        let file = TempFile::new("roundtrip");
        let mut store = Store::open(&file.0)?;
        let draft = Draft::new(Path::new("README.md"), LineRange::new(3, 4), "rename");
        let id = store.annotate(draft, TEXT, 100)?;
        store.reply(
            &id,
            Reply::new(Author::agent("claude"), 101, "done").proposing_resolution(),
        )?;
        let other = store.annotate(
            Draft::new(Path::new("docs/guide.md"), LineRange::new(1, 1), "hmm"),
            TEXT,
            102,
        )?;
        store.resolve(&other, Author::agent("bot"), 103)?;
        store.resolve(&id, Author::User, 104)?;
        store.reopen(&id, 105)?;

        let again = Store::open(&file.0)?;
        assert_eq!(again.threads(), store.threads());
        let thread = again
            .thread(&id)
            .ok_or_else(|| StoreError::parse(0, "lost".into()))?;
        assert_eq!(thread.snippet(), "alpha\nbeta");
        assert_eq!(thread.comment(), "rename");
        assert_eq!(thread.status(), Status::Open);
        assert_eq!(thread.replies().len(), 1);
        assert!(thread.replies()[0].proposes_resolution());
        assert_eq!(thread.replies()[0].author().to_string(), "claude");
        assert_eq!(thread.updated(), 105);
        assert_eq!(
            again.thread(&other).map(super::Thread::status),
            Some(Status::AutoResolved)
        );
        assert_eq!(again.thread(&other).map(super::Thread::updated), Some(103));
        assert_eq!(again.for_path(Path::new("README.md")).count(), 1);

        let raw = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
        assert_eq!(raw.lines().count(), 6);
        assert!(raw.lines().all(|line| line.contains("\"v\":2")));
        assert!(raw.contains("\"author\":\"claude\""));
        Ok(())
    }

    /// A thread carries the commit it was written against; the scope hides
    /// it where that commit is not reachable, and a version 1 record with no
    /// commit is shown everywhere (ADR 0024).
    #[test]
    fn threads_are_scoped_by_the_commit_they_were_written_against() -> Result<(), StoreError> {
        let file = TempFile::new("scope");
        if let Some(parent) = file.0.parent() {
            fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
        }
        fs::write(
            &file.0,
            concat!(
                r#"{"event":"annotate","v":1,"id":"1-1-1","path":"a.md","range":[1,1],"#,
                r##""snippet":"# Title","anchor":{"lines":["x"]},"created":1,"comment":"old"}"##,
                "\n"
            ),
        )
        .map_err(|e| StoreError::io(&file.0, e))?;
        let mut store = Store::open(&file.0)?;
        let legacy = store.threads()[0].id().clone();
        assert_eq!(store.thread(&legacy).and_then(Thread::commit), None);
        let scoped = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(2, 2), "new")
                .at_commit(Some("abc123".to_owned())),
            TEXT,
            2,
        )?;
        assert_eq!(store.commits().collect::<Vec<_>>(), ["abc123"]);
        let again = Store::open(&file.0)?;
        assert_eq!(
            again.thread(&scoped).and_then(Thread::commit),
            Some("abc123")
        );

        let everywhere = Scope::unscoped();
        let on_branch = Scope::reachable(HashSet::from(["abc123".to_owned()]));
        let elsewhere = Scope::reachable(HashSet::new());
        let visible = |scope: &Scope| -> Vec<&ThreadId> {
            again
                .threads()
                .iter()
                .filter(|t| scope.includes(t))
                .map(Thread::id)
                .collect()
        };
        assert_eq!(visible(&everywhere), [&legacy, &scoped]);
        assert_eq!(visible(&on_branch), [&legacy, &scoped]);
        assert_eq!(visible(&elsewhere), [&legacy]);
        Ok(())
    }

    #[test]
    fn store_rejects_bad_ranges_unknown_threads_and_bad_lines() -> Result<(), StoreError> {
        let file = TempFile::new("errors");
        let mut store = Store::open(&file.0)?;
        let draft = Draft::new(Path::new("a.md"), LineRange::new(9, 9), "x");
        let range_error = store.annotate(draft, TEXT, 1).err();
        assert!(range_error.is_some_and(|e| e.to_string().contains('9')));
        let ghost = super::ThreadId("nope".to_owned());
        let ghost_error = store.reopen(&ghost, 1).err();
        assert!(ghost_error.is_some_and(|e| e.to_string().contains("nope")));
        assert!(!file.0.exists(), "invalid events are never written");

        if let Some(parent) = file.0.parent() {
            fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
        }
        fs::write(&file.0, "{\"event\":\"dance\"}\n").map_err(|e| StoreError::io(&file.0, e))?;
        let Err(error) = Store::open(&file.0) else {
            return Err(StoreError::parse(0, "accepted garbage".into()));
        };
        assert_eq!(error.parse_line(), Some(1));
        assert!(!error.is_io());
        Ok(())
    }

    #[test]
    fn thread_locate_reports_placement() -> Result<(), StoreError> {
        let file = TempFile::new("placement");
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(5, 5), "gamma?"),
            TEXT,
            1,
        )?;
        let thread = store.thread(&id).cloned();
        let thread = thread.ok_or_else(|| StoreError::parse(0, "lost".into()))?;
        assert_eq!(
            thread.locate(TEXT),
            Placement::Anchored(LineRange::new(5, 5))
        );
        let placement = thread.locate("nothing here\n");
        assert!(placement.is_detached());
        assert_eq!(placement.range(), LineRange::new(5, 5));
        Ok(())
    }
}
