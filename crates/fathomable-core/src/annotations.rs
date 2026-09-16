// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Annotation threads, content anchors, and the append-only JSONL store.
//!
//! A [`Thread`] is one user comment on a [`LineRange`] of a workspace file
//! plus its replies (ADR 0005). Threads are keyed to content, not position:
//! an [`Anchor`] records a short hash of every annotated line and of the
//! lines immediately above and below, and [`Thread::locate`] finds the range
//! again after the file changes. When the lines are gone the thread is
//! [`Placement::Detached`] at its last known range rather than lost. Each
//! line thread also carries a [`Context`] window of the text it was last
//! placed in, so an edit made while nothing runs can be followed (ADR 0038).
//! A file-wide thread has no range, anchor, or context.
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
//! use fathomable_core::annotations::{Author, Draft, LineRange, Store};
//!
//! let mut store = Store::open("/tmp/threads.jsonl")?;
//! let text = "# Title\n\nalpha\nbeta\n";
//! let draft = Draft::new(Author::User, Path::new("README.md"), LineRange::new(3, 4), "rename these");
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

use crate::context::Context;
use sha2::{Digest, Sha256};

/// The format version written in every event line; [`Store::open`]
/// refuses a file of another (ADR 0062).
pub(crate) const FORMAT_VERSION: u32 = 2;

/// File name of the thread store inside a workspace state directory.
pub(crate) const THREADS_FILE: &str = "threads.jsonl";

/// Hex characters kept from a SHA-256 digest; 64 bits is plenty to tell
/// lines of one file apart and keeps the JSONL readable.
const HASH_CHARS: usize = 16;

/// Short content hash of one line, ignoring trailing whitespace.
#[must_use]
pub(crate) fn line_hash(line: &str) -> String {
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

/// The line hashes of one text, computed once for every anchor located in it.
///
/// Locating hashes every line of the text; a file with many threads is
/// hashed once through this and each thread compared against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHashes(Vec<String>);

impl LineHashes {
    /// Hash every line of `text`.
    #[must_use]
    pub fn of(text: &str) -> Self {
        Self(text.lines().map(line_hash).collect())
    }

    /// How many lines were hashed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the text had no lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
    /// exact lines no longer exist. To locate many anchors in one text,
    /// hash it once with [`LineHashes::of`] and use [`Anchor::locate_in`].
    #[must_use]
    pub fn locate(&self, text: &str, hint: LineRange) -> Option<LineRange> {
        self.locate_in(&LineHashes::of(text), hint)
    }

    /// [`Anchor::locate`] in a text hashed beforehand.
    #[must_use]
    pub fn locate_in(&self, hashes: &LineHashes, hint: LineRange) -> Option<LineRange> {
        let n = self.lines.len();
        if n == 0 {
            return None;
        }
        let hashes = &hashes.0;
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
    /// The thread is on the file as a whole, not on lines of it
    /// (ADR 0063).
    File,
}

impl Placement {
    /// The range to draw at, anchored or not; `None` for a thread on the
    /// file as a whole.
    #[must_use]
    pub fn range(&self) -> Option<LineRange> {
        match self {
            Self::Anchored(range) | Self::Edited(range) | Self::Detached(range) => Some(*range),
            Self::File => None,
        }
    }

    /// Whether the thread is on the file as a whole (ADR 0063).
    #[must_use]
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
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
/// MCP, the client implementation the host reported and its qualified
/// identity (ADR 0014). On the wire the user is the compact string `"user"`;
/// agents always use an object with a `name` and optional `client` and `id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "AuthorRepr", into = "AuthorRepr")]
pub enum Author {
    /// The person at the keyboard.
    User,
    /// An agent.
    Agent {
        /// The stable harness label, or a local fixture's chosen name.
        name: String,
        /// The MCP client that carried the reply, when known.
        client: Option<String>,
        /// The harness-qualified chat identity, when known.
        id: Option<String>,
    },
}

impl Default for Author {
    /// The user, when a record says nothing.
    fn default() -> Self {
        Self::User
    }
}

impl Author {
    /// An agent known only by `name`.
    #[must_use]
    pub fn agent(name: impl Into<String>) -> Self {
        Self::Agent {
            name: name.into(),
            client: None,
            id: None,
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

    /// The chat identity the message was signed with, if known.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::User => None,
            Self::Agent { id, .. } => id.as_deref(),
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
                ..
            } if client != name => write!(f, "{name} ({client})"),
            Self::Agent { name, .. } => f.write_str(name),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum AuthorRepr {
    User(String),
    Agent(AgentRepr),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRepr {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

impl TryFrom<AuthorRepr> for Author {
    type Error = String;

    fn try_from(repr: AuthorRepr) -> Result<Self, Self::Error> {
        match repr {
            AuthorRepr::User(name) if name == "user" => Ok(Self::User),
            AuthorRepr::User(name) => Err(format!(
                "unknown author {name:?}; expected the compact user value \"user\" or an agent object"
            )),
            AuthorRepr::Agent(AgentRepr { name, client, id }) => {
                Ok(Self::Agent { name, client, id })
            }
        }
    }
}

impl From<Author> for AuthorRepr {
    fn from(author: Author) -> Self {
        match author {
            Author::User => Self::User("user".to_owned()),
            Author::Agent { name, client, id } => Self::Agent(AgentRepr { name, client, id }),
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
    /// When the user last edited it (ADR 0058); only a user's reply can be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edited: Option<u64>,
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
            edited: None,
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

    /// When the user last edited it, in Unix seconds (ADR 0058).
    #[must_use]
    pub fn edited(&self) -> Option<u64> {
        self.edited
    }
}

/// A message within a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "index", rename_all = "snake_case")]
pub enum MessageTarget {
    /// The comment that opened the thread.
    Comment,
    /// A reply by its zero-based position.
    Reply(usize),
}

/// Whether a thread is open or resolved.
///
/// Only the user resolves (ADR 0053); an agent's reply can propose it,
/// see [`Thread::proposes_resolution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Awaiting action.
    Open,
    /// Resolved by the user.
    Resolved,
}

/// An annotation with its replies and status.
///
/// Serializes as a plain object so it can travel over the session socket
/// (ADR 0014); the JSONL file stores events, not threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    id: ThreadId,
    path: PathBuf,
    /// The lines the comment is on; `None` for a comment on the file as
    /// a whole (ADR 0063), which then has no anchor and an empty snippet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<Anchor>,
    created: u64,
    updated: u64,
    /// Who wrote the comment (ADR 0061); the user unless the record says
    /// otherwise, so it is written only for an agent.
    #[serde(default, skip_serializing_if = "Author::is_user")]
    author: Author,
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
    /// When the user last edited the comment (ADR 0058).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment_edited: Option<u64>,
    /// When the user last reopened the thread (ADR 0058).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reopened: Option<u64>,
    /// The text the thread was last placed in (ADR 0038). Not sent over
    /// the session socket, where the snippet already travels.
    #[serde(skip)]
    context: Option<Context>,
}

impl Thread {
    /// The thread id.
    #[must_use]
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    /// Who wrote the comment that opened the thread (ADR 0061).
    #[must_use]
    pub fn author(&self) -> &Author {
        &self.author
    }

    /// Workspace-relative path of the annotated file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The range as it was when the annotation was made; `None` for a
    /// thread on the file as a whole (ADR 0063).
    #[must_use]
    pub fn range(&self) -> Option<LineRange> {
        self.range
    }

    /// Whether the thread is on the file as a whole rather than on lines
    /// of it (ADR 0063).
    #[must_use]
    pub fn is_on_file(&self) -> bool {
        self.range.is_none()
    }

    /// Where the thread is, the way notices name it: `path:lines`, or
    /// the path alone for a thread on the file as a whole (ADR 0063).
    #[must_use]
    pub fn place(&self) -> String {
        match self.range {
            Some(range) => format!("{}:{range}", self.path.display()),
            None => self.path.display().to_string(),
        }
    }

    /// The annotated source lines, without the trailing newline; empty
    /// for a thread on the file as a whole.
    #[must_use]
    pub fn snippet(&self) -> &str {
        &self.snippet
    }

    /// The content anchor; `None` for a thread on the file as a whole.
    #[must_use]
    pub fn anchor(&self) -> Option<&Anchor> {
        self.anchor.as_ref()
    }

    /// The window of text the line thread was last placed in (ADR 0038);
    /// `None` for a file-wide thread.
    #[must_use]
    pub fn context(&self) -> Option<&Context> {
        self.context.as_ref()
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

    /// Open or resolved.
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    /// Whether the thread is open and its newest reply proposes resolving
    /// it (ADR 0053), so the user's `o` is all it needs.
    #[must_use]
    pub fn proposes_resolution(&self) -> bool {
        self.status == Status::Open && self.replies.last().is_some_and(Reply::proposes_resolution)
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

    /// When the user last edited the comment, in Unix seconds (ADR 0058).
    #[must_use]
    pub fn comment_edited(&self) -> Option<u64> {
        self.comment_edited
    }

    /// When the user last reopened the thread, in Unix seconds (ADR 0058).
    #[must_use]
    pub fn reopened(&self) -> Option<u64> {
        self.reopened
    }

    /// The last act on the thread: its author and time.
    ///
    /// The acts are the comment, written or edited; each reply, written
    /// or edited; and the most recent reopen. The comment, an edit, and a
    /// reopen are the user's; a reply is its author's. At a tie a later
    /// message wins, and a message beats a reopen.
    #[must_use]
    pub fn last_act(&self) -> (&Author, u64) {
        let mut act = (
            &self.author,
            self.comment_edited
                .map_or(self.created, |at| at.max(self.created)),
        );
        for reply in &self.replies {
            let at = reply
                .edited
                .map_or(reply.created, |at| at.max(reply.created));
            if at >= act.1 {
                act = (&reply.author, at);
            }
        }
        if let Some(reopened) = self.reopened
            && reopened > act.1
        {
            act = (&Author::User, reopened);
        }
        act
    }

    /// Whether the thread is open and an agent has the last word, so it
    /// is *waiting* on the user (ADR 0030, amended by ADR 0058): only the
    /// user's reply, edit, reopen, or resolve ends the wait.
    #[must_use]
    pub fn awaits_user(&self) -> bool {
        self.status == Status::Open && !self.last_act().0.is_user()
    }

    /// Whether the thread is open and the user has the last word, so an
    /// agent's reply would be the next act.
    #[must_use]
    pub fn awaits_agent(&self) -> bool {
        self.status == Status::Open && self.last_act().0.is_user()
    }

    /// The newest message: the last reply, or the comment when there
    /// are none, as its author and `created` time.
    #[must_use]
    pub fn newest(&self) -> (&Author, u64) {
        self.replies
            .last()
            .map_or((&Author::User, self.created), |reply| {
                (reply.author(), reply.created())
            })
    }

    /// Where the thread sits in `text` now.
    ///
    /// To place many threads in one text, hash it once with
    /// [`LineHashes::of`] and use [`Thread::locate_in`].
    #[must_use]
    pub fn locate(&self, text: &str) -> Placement {
        self.locate_in(&LineHashes::of(text))
    }

    /// [`Thread::locate`] in a text hashed beforehand.
    #[must_use]
    pub fn locate_in(&self, hashes: &LineHashes) -> Placement {
        let (Some(anchor), Some(range)) = (&self.anchor, self.range) else {
            return Placement::File;
        };
        match (anchor.locate_in(hashes, range), self.edited) {
            (Some(range), Some(_)) => Placement::Edited(range),
            (Some(range), None) => Placement::Anchored(range),
            (None, _) => Placement::Detached(range),
        }
    }
}

/// What starts a thread: who is commenting, where, and what they say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    author: Author,
    path: PathBuf,
    range: Option<LineRange>,
    comment: String,
    commit: Option<String>,
}

impl Draft {
    /// `author`'s comment on `range` of the workspace-relative `path`.
    ///
    /// The viewer passes the user; an agent's `thread_start` passes the
    /// agent (ADR 0061).
    #[must_use]
    pub fn new(author: Author, path: &Path, range: LineRange, comment: impl Into<String>) -> Self {
        Self {
            author,
            path: path.to_path_buf(),
            range: Some(range),
            comment: comment.into(),
            commit: None,
        }
    }

    /// `author`'s comment on the workspace-relative `path` as a whole
    /// (ADR 0063): the thread has no lines, no anchor, and no snippet.
    #[must_use]
    pub fn on_file(author: Author, path: &Path, comment: impl Into<String>) -> Self {
        Self {
            author,
            path: path.to_path_buf(),
            range: None,
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

/// The `v` of one event line, read before the event itself.
#[derive(Deserialize)]
struct Stamp {
    v: u32,
}

/// One line of the JSONL file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Annotate {
        v: u32,
        id: ThreadId,
        path: PathBuf,
        /// Absent for a comment on the file as a whole (ADR 0063).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<LineRange>,
        snippet: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<Anchor>,
        created: u64,
        /// Who wrote the comment; written only for an agent (ADR 0061).
        #[serde(default, skip_serializing_if = "Author::is_user")]
        author: Author,
        comment: String,
        /// `None` for a workspace outside git, which reads as unscoped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        /// The window the lines were placed in (ADR 0038); absent only
        /// for a comment on the file as a whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Context>,
    },
    Reply {
        v: u32,
        thread: ThreadId,
        #[serde(flatten)]
        reply: Reply,
    },
    /// The user replaced one of their messages (ADR 0013).
    Edit {
        v: u32,
        thread: ThreadId,
        target: MessageTarget,
        body: String,
        created: u64,
    },
    /// The user resolved the thread; only the user can (ADR 0053).
    Resolve {
        v: u32,
        thread: ThreadId,
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
        context: Context,
    },
    /// The file was renamed and the thread now lives at `path`, range
    /// and anchor unchanged (ADR 0028).
    Move {
        v: u32,
        thread: ThreadId,
        path: PathBuf,
        created: u64,
    },
    /// The user deleted the thread (ADR 0034). A tombstone: the thread
    /// is dropped on load and later events on it are ignored.
    Delete {
        v: u32,
        thread: ThreadId,
        created: u64,
    },
    /// A history rewrite dropped the thread's commit while its lines
    /// stayed; it now belongs to `commit` (ADR 0035).
    Rescope {
        v: u32,
        thread: ThreadId,
        commit: String,
        created: u64,
    },
}

impl Event {
    /// The thread an event acts on; none for the one that creates it.
    fn thread_id(&self) -> Option<&ThreadId> {
        match self {
            Self::Annotate { .. } => None,
            Self::Reply { thread, .. }
            | Self::Edit { thread, .. }
            | Self::Resolve { thread, .. }
            | Self::Reopen { thread, .. }
            | Self::Relocate { thread, .. }
            | Self::Move { thread, .. }
            | Self::Delete { thread, .. }
            | Self::Rescope { thread, .. } => Some(thread),
        }
    }
}

/// The threads of one workspace, backed by an append-only JSONL file.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    threads: Vec<Thread>,
    /// Threads a tombstone removed, so an event that raced the deletion
    /// (a headless reply) is skipped rather than rejected as unknown.
    deleted: HashSet<ThreadId>,
}

impl Store {
    /// Load the store at `path`, or start empty when the file is missing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the file cannot be read, a line is of
    /// another format version or not a known event, or an event refers to
    /// a thread the file never created.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let mut store = Self {
            path,
            threads: Vec::new(),
            deleted: HashSet::new(),
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
            let Stamp { v } = serde_json::from_str(line)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
            if v != FORMAT_VERSION {
                return Err(StoreError::version(&store.path, index + 1, v));
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
        let (anchor, context, snippet) = match draft.range {
            Some(range) => {
                let anchor = Anchor::capture(text, range).ok_or(StoreError {
                    kind: ErrorKind::BadRange(range),
                })?;
                let context = Context::capture(text, range).ok_or(StoreError {
                    kind: ErrorKind::BadRange(range),
                })?;
                let snippet = text
                    .lines()
                    .skip(range.start - 1)
                    .take(range.len())
                    .collect::<Vec<_>>()
                    .join("\n");
                (Some(anchor), Some(context), snippet)
            }
            None => (None, None, String::new()),
        };
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
            author: draft.author,
            comment: draft.comment,
            commit: draft.commit,
            context,
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

    /// Replace a user-authored message in thread `id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread or message is unknown, the
    /// selected reply was not written by the user, or the file cannot be
    /// appended to.
    pub fn edit(
        &mut self,
        id: &ThreadId,
        target: MessageTarget,
        body: impl Into<String>,
        now: u64,
    ) -> Result<(), StoreError> {
        self.commit(Event::Edit {
            v: FORMAT_VERSION,
            thread: id.clone(),
            target,
            body: body.into(),
            created: now,
        })
    }

    /// Resolve the thread `id`, as the user, fixing it to `head`, the
    /// workspace's `HEAD` commit (ADR 0072): when the thread is at another
    /// commit, or at none, a rescope to `head` is appended first, so the
    /// thread shows only while that commit is `HEAD`. `None` outside git.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn resolve(
        &mut self,
        id: &ThreadId,
        head: Option<&str>,
        now: u64,
    ) -> Result<(), StoreError> {
        let at = self.thread(id).and_then(Thread::commit);
        if let Some(head) = head
            && at != Some(head)
        {
            self.rescope(id, head, now)?;
        }
        self.commit(Event::Resolve {
            v: FORMAT_VERSION,
            thread: id.clone(),
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
    /// Returns [`StoreError`] when the thread is unknown or on the file as
    /// a whole, the range runs past the end of `text`, or the file cannot
    /// be appended to.
    pub fn relocate(
        &mut self,
        id: &ThreadId,
        range: LineRange,
        text: &str,
        now: u64,
    ) -> Result<(), StoreError> {
        self.on_lines(id)?;
        let anchor = Anchor::capture(text, range).ok_or(StoreError {
            kind: ErrorKind::BadRange(range),
        })?;
        let context = Context::capture(text, range).ok_or(StoreError {
            kind: ErrorKind::BadRange(range),
        })?;
        self.commit(Event::Relocate {
            v: FORMAT_VERSION,
            thread: id.clone(),
            range,
            anchor,
            created: now,
            context,
        })
    }

    /// Delete the thread `id` (ADR 0034): a tombstone is appended and the
    /// thread is dropped from every reader.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn delete(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
        self.thread_mut(id)?;
        self.commit(Event::Delete {
            v: FORMAT_VERSION,
            thread: id.clone(),
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

    /// Record that the thread `id` now belongs to `commit` (ADR 0035):
    /// a rewrite dropped the commit it was written against while its
    /// lines stayed in the working tree.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn rescope(&mut self, id: &ThreadId, commit: &str, now: u64) -> Result<(), StoreError> {
        self.commit(Event::Rescope {
            v: FORMAT_VERSION,
            thread: id.clone(),
            commit: commit.to_owned(),
            created: now,
        })
    }

    /// Apply an event in memory, then append it; the file is only written
    /// when the event is valid, and the memory only kept when the file
    /// took it, so what the viewer shows is what the next reload reads.
    fn commit(&mut self, event: Event) -> Result<(), StoreError> {
        let mut line = serde_json::to_string(&event).map_err(|error| StoreError {
            kind: ErrorKind::Parse(0, error.to_string()),
        })?;
        let before = (self.threads.clone(), self.deleted.clone());
        self.apply(event)?;
        line.push('\n');
        if let Err(error) = self.append(&line) {
            (self.threads, self.deleted) = before;
            return Err(error);
        }
        Ok(())
    }

    /// Append `line`, newline included, to the file.
    fn append(&self, line: &str) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| StoreError::io(&self.path, error))?;
        // One `write` for line and newline together: with `O_APPEND` each
        // call lands whole, so two writers (a second viewer, a headless
        // `--mcp` reply) cannot interleave `{a}{b}\n\n` (ADR 0032).
        file.write_all(line.as_bytes())
            .map_err(|error| StoreError::io(&self.path, error))
    }

    #[expect(clippy::too_many_lines, reason = "one arm per event kind")]
    fn apply(&mut self, event: Event) -> Result<(), StoreError> {
        if let Some(id) = event.thread_id()
            && self.deleted.contains(id)
        {
            return Ok(());
        }
        match event {
            Event::Annotate {
                id,
                path,
                range,
                snippet,
                anchor,
                created,
                author,
                comment,
                commit,
                context,
                ..
            } => {
                if range.is_some() != anchor.is_some() || range.is_some() != context.is_some() {
                    return Err(StoreError {
                        kind: ErrorKind::AnnotationShape,
                    });
                }
                self.threads.push(Thread {
                    id,
                    path,
                    range,
                    snippet,
                    anchor,
                    created,
                    updated: created,
                    author,
                    comment,
                    replies: Vec::new(),
                    status: Status::Open,
                    edited: None,
                    commit,
                    comment_edited: None,
                    reopened: None,
                    context,
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
            Event::Edit {
                thread,
                target,
                body,
                created,
                ..
            } => {
                let id = thread.clone();
                let thread = self.thread_mut(&thread)?;
                match target {
                    MessageTarget::Comment => {
                        thread.comment = body;
                        thread.comment_edited = Some(created);
                    }
                    MessageTarget::Reply(index) => {
                        let reply = thread.replies.get_mut(index).ok_or_else(|| StoreError {
                            kind: ErrorKind::UnknownMessage(id.clone(), target),
                        })?;
                        if !reply.author.is_user() {
                            return Err(StoreError {
                                kind: ErrorKind::MessageNotEditable(id, target),
                            });
                        }
                        reply.body = body;
                        reply.edited = Some(created);
                    }
                }
                thread.updated = thread.updated.max(created);
            }
            Event::Resolve {
                thread, created, ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.edited = None;
                thread.status = Status::Resolved;
            }
            Event::Reopen {
                thread, created, ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.status = Status::Open;
                thread.edited = None;
                thread.reopened = Some(created);
            }
            Event::Relocate {
                thread,
                range,
                anchor,
                created,
                context,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.range = Some(range);
                thread.anchor = Some(anchor);
                thread.edited = Some(created);
                thread.context = Some(context);
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
            Event::Delete { thread, .. } => {
                self.thread_mut(&thread)?;
                self.threads.retain(|other| other.id != thread);
                self.deleted.insert(thread);
            }
            Event::Rescope {
                thread,
                commit,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.updated = thread.updated.max(created);
                thread.commit = Some(commit);
            }
        }
        Ok(())
    }

    /// Refuse a thread on the file as a whole (ADR 0063), which has no
    /// lines to move or window to record.
    fn on_lines(&self, id: &ThreadId) -> Result<(), StoreError> {
        let thread = self.thread(id).ok_or_else(|| StoreError {
            kind: ErrorKind::UnknownThread(id.clone()),
        })?;
        if thread.is_on_file() {
            return Err(StoreError {
                kind: ErrorKind::OnFile(id.clone()),
            });
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
    Version(PathBuf, usize, u32),
    UnknownThread(ThreadId),
    UnknownMessage(ThreadId, MessageTarget),
    MessageNotEditable(ThreadId, MessageTarget),
    BadRange(LineRange),
    OnFile(ThreadId),
    AnnotationShape,
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

    fn version(path: &Path, line: usize, found: u32) -> Self {
        Self {
            kind: ErrorKind::Version(path.to_path_buf(), line, found),
        }
    }

    /// Whether the cause was an I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(..))
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Io(path, error) => write!(f, "{}: {error}", path.display()),
            ErrorKind::Parse(line, message) => write!(f, "{THREADS_FILE} line {line}: {message}"),
            ErrorKind::Version(path, line, found) => write!(
                f,
                "{THREADS_FILE} line {line}: format version {found}, this build writes \
                 {FORMAT_VERSION}; delete {} to start over",
                path.display()
            ),
            ErrorKind::UnknownThread(id) => write!(f, "unknown thread {id}"),
            ErrorKind::UnknownMessage(id, target) => {
                write!(f, "unknown message {target:?} in thread {id}")
            }
            ErrorKind::MessageNotEditable(id, target) => {
                write!(
                    f,
                    "message {target:?} in thread {id} was not written by the user"
                )
            }
            ErrorKind::BadRange(range) => write!(f, "lines {range} are past the end of the file"),
            ErrorKind::OnFile(id) => {
                write!(f, "thread {id} is on the file as a whole and has no lines")
            }
            ErrorKind::AnnotationShape => f.write_str(
                "line annotations must carry a range, anchor, and context; \
                     file-wide annotations must carry none",
            ),
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
mod tests;
