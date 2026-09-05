// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Annotation threads, content anchors, and the append-only JSONL store.
//!
//! A [`Thread`] is one user comment on a [`LineRange`] of a workspace file
//! plus its replies (ADR 0005). Threads are keyed to content, not position:
//! an [`Anchor`] records a short hash of every annotated line and of the
//! lines immediately above and below, and [`Thread::locate`] finds the range
//! again after the file changes. When the lines are gone the thread is
//! [`Placement::Detached`] at its last known range rather than lost. Each
//! thread also carries a [`Context`] window of the text it was last placed
//! in, so an edit made while nothing runs can be followed (ADR 0038).
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
        /// The harness session it subscribed as (ADR 0040), when it did.
        id: Option<String>,
        /// The agent type it subscribed with (ADR 0040), when it did.
        kind: Option<String>,
    },
}

impl Default for Author {
    /// The user: what a record written before ADR 0061 means by saying
    /// nothing.
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
            kind: None,
        }
    }

    /// The same author signing as subscriber `id` of type `kind`.
    ///
    /// The user stays the user.
    #[must_use]
    pub fn subscribed(self, id: impl Into<String>, kind: impl Into<String>) -> Self {
        match self {
            Self::User => Self::User,
            Self::Agent { name, client, .. } => Self::Agent {
                name,
                client,
                id: Some(id.into()),
                kind: Some(kind.into()),
            },
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

    /// The subscriber id the message was signed with, if any.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::User => None,
            Self::Agent { id, .. } => id.as_deref(),
        }
    }

    /// The agent type the message was signed with, if any.
    #[must_use]
    pub fn kind(&self) -> Option<&str> {
        match self {
            Self::User => None,
            Self::Agent { kind, .. } => kind.as_deref(),
        }
    }
}

impl fmt::Display for Author {
    /// `user`, the agent name, `name (type)` for a subscribed agent, or
    /// `name (client)` when the two differ.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Agent {
                name,
                kind: Some(kind),
                ..
            } => write!(f, "{name} ({kind})"),
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
enum AuthorWire {
    Name(String),
    Full {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
}

impl From<AuthorWire> for Author {
    fn from(wire: AuthorWire) -> Self {
        match wire {
            AuthorWire::Name(name) if name == "user" => Self::User,
            AuthorWire::Name(name) => Self::agent(name),
            AuthorWire::Full {
                name,
                client,
                id,
                kind,
            } => Self::Agent {
                name,
                client,
                id,
                kind,
            },
        }
    }
}

impl From<Author> for AuthorWire {
    fn from(author: Author) -> Self {
        match author {
            Author::User => Self::Name("user".to_owned()),
            Author::Agent {
                name,
                client: None,
                id: None,
                kind: None,
            } => Self::Name(name),
            Author::Agent {
                name,
                client,
                id,
                kind,
            } => Self::Full {
                name,
                client,
                id,
                kind,
            },
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
    range: LineRange,
    snippet: String,
    anchor: Anchor,
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

    /// The window of text the thread was last placed in (ADR 0038);
    /// `None` for a thread recorded before windows were kept.
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

    /// The last act on the thread (ADR 0058): its author and time.
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

    /// Whether the thread is open and the user has the last word, so it
    /// is *pending* for every subscribed agent (ADR 0058): any agent's
    /// reply, or a resolution, ends it.
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
    #[must_use]
    pub fn locate(&self, text: &str) -> Placement {
        match (self.anchor.locate(text, self.range), self.edited) {
            (Some(range), Some(_)) => Placement::Edited(range),
            (Some(range), None) => Placement::Anchored(range),
            (None, _) => Placement::Detached(self.range),
        }
    }
}

/// What starts a thread: who is commenting, where, and what they say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    author: Author,
    path: PathBuf,
    range: LineRange,
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
pub struct Reach {
    reachable: Option<HashSet<String>>,
}

impl Reach {
    /// A reach that shows every thread: no git, or no `HEAD` yet.
    #[must_use]
    pub fn everything() -> Self {
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
        /// Who wrote the comment; absent, and the user's, on records
        /// written before ADR 0061.
        #[serde(default, skip_serializing_if = "Author::is_user")]
        author: Author,
        comment: String,
        /// Absent in version 1 records, which read as unscoped.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        /// The window the lines were placed in (ADR 0038); absent on
        /// records written before it.
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<Context>,
    },
    /// The window a thread's lines sit in, recorded for a thread that
    /// had none (ADR 0038). Neither moves the thread nor updates it.
    Context {
        v: u32,
        thread: ThreadId,
        context: Context,
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
            | Self::Rescope { thread, .. }
            | Self::Context { thread, .. } => Some(thread),
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
    /// Returns [`StoreError`] when the file cannot be read, a line is not a
    /// known event, or an event refers to a thread the file never created.
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
        let context = Context::capture(text, draft.range);
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

    /// Resolve the thread `id`, as the user.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn resolve(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
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
            context: Context::capture(text, range),
        })
    }

    /// Record the window of `text` around `range` as where the thread `id`
    /// sits (ADR 0038), for a thread stored before windows were kept. The
    /// thread's range, anchor, and `updated` are untouched.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown, the range runs
    /// past the end of `text`, or the file cannot be appended to.
    pub fn record_context(
        &mut self,
        id: &ThreadId,
        range: LineRange,
        text: &str,
        now: u64,
    ) -> Result<(), StoreError> {
        let context = Context::capture(text, range).ok_or(StoreError {
            kind: ErrorKind::BadRange(range),
        })?;
        self.commit(Event::Context {
            v: FORMAT_VERSION,
            thread: id.clone(),
            context,
            created: now,
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
    /// when the event is valid.
    fn commit(&mut self, event: Event) -> Result<(), StoreError> {
        let mut line = serde_json::to_string(&event).map_err(|error| StoreError {
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
        // One `write` for line and newline together: with `O_APPEND` each
        // call lands whole, so two writers (a second viewer, a headless
        // `--mcp` reply) cannot interleave `{a}{b}\n\n` (ADR 0032).
        line.push('\n');
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
                thread.range = range;
                thread.anchor = anchor;
                thread.edited = Some(created);
                thread.context = context;
            }
            Event::Context {
                thread, context, ..
            } => {
                self.thread_mut(&thread)?.context = Some(context);
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
    UnknownMessage(ThreadId, MessageTarget),
    MessageNotEditable(ThreadId, MessageTarget),
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

    use fathomable_testing::TempDir;

    use super::{
        Anchor, Author, Draft, Event, FORMAT_VERSION, LineRange, MessageTarget, Placement, Reach,
        Reply, Status, Store, StoreError, Thread, ThreadId, line_hash,
    };

    const TEXT: &str = "# Title\n\nalpha\nbeta\ngamma\n\ndelta\n";

    /// A store path two directories deep inside a fresh temp dir, so a
    /// test sees the store create its parents.
    struct TempFile(
        PathBuf,
        #[expect(dead_code, reason = "held for its Drop")] TempDir,
    );

    impl TempFile {
        fn new(name: &str) -> Result<Self, StoreError> {
            let dir = TempDir::new(&format!("annotations-{name}"))
                .map_err(|e| StoreError::io(Path::new(name), e))?;
            Ok(Self(dir.0.join("nested").join("threads.jsonl"), dir))
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
        let file = TempFile::new("waiting")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(3, 3),
                "why?",
            ),
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
        store.resolve(&id, 14)?;
        assert!(!waiting(&store), "a resolved thread never waits");
        Ok(())
    }

    #[test]
    fn user_messages_can_be_edited_and_other_messages_cannot() -> Result<(), StoreError> {
        let file = TempFile::new("edit")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(3, 3),
                "why?",
            ),
            TEXT,
            10,
        )?;
        store.reply(&id, Reply::new(Author::agent("claude"), 11, "because"))?;
        store.reply(&id, Reply::new(Author::User, 12, "okay"))?;

        store.edit(&id, MessageTarget::Comment, "why exactly?", 13)?;
        store.edit(&id, MessageTarget::Reply(1), "understood", 14)?;
        let before_invalid =
            fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?;
        assert!(
            store
                .edit(&id, MessageTarget::Reply(0), "not mine", 15)
                .is_err()
        );
        assert!(
            store
                .edit(&id, MessageTarget::Reply(2), "missing", 15)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(&file.0).map_err(|error| StoreError::io(&file.0, error))?,
            before_invalid,
            "invalid edits are not appended"
        );

        let reloaded = Store::open(&file.0)?;
        let thread = reloaded.thread(&id).ok_or_else(|| StoreError {
            kind: super::ErrorKind::UnknownThread(id.clone()),
        })?;
        assert_eq!(thread.comment(), "why exactly?");
        assert_eq!(thread.replies()[0].body(), "because");
        assert_eq!(thread.replies()[1].body(), "understood");
        assert_eq!(thread.updated(), 14);
        Ok(())
    }

    #[test]
    fn a_deleted_thread_is_gone_and_later_events_on_it_are_ignored() -> Result<(), StoreError> {
        let file = TempFile::new("delete")?;
        let mut store = Store::open(&file.0)?;
        let keep = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "keep",
            ),
            TEXT,
            10,
        )?;
        let gone = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(3, 3),
                "gone",
            ),
            TEXT,
            11,
        )?;
        store.delete(&gone, 12)?;
        assert!(store.thread(&gone).is_none());
        assert!(store.delete(&gone, 13).is_err(), "already gone");
        assert_eq!(store.threads().len(), 1);
        // A headless reply that raced the deletion lands after the
        // tombstone; the file still loads and the thread stays gone.
        let raced = serde_json::to_string(&Event::Reply {
            v: FORMAT_VERSION,
            thread: gone.clone(),
            reply: Reply::new(Author::agent("claude"), 14, "late"),
        })
        .map_err(|error| StoreError::parse(0, error.to_string()))?;
        let mut text = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
        text.push_str(&raced);
        text.push('\n');
        fs::write(&file.0, text).map_err(|e| StoreError::io(&file.0, e))?;
        let reloaded = Store::open(&file.0)?;
        assert!(reloaded.thread(&gone).is_none());
        assert!(reloaded.thread(&keep).is_some());
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
        let file = TempFile::new("relocate")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("README.md"),
                LineRange::new(3, 4),
                "rename",
            ),
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
        let file = TempFile::new("move")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("old.md"),
                LineRange::new(3, 4),
                "rename",
            ),
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
            id: None,
            kind: None,
        };
        let json = serde_json::to_string(&full)?;
        assert_eq!(json, r#"{"name":"reviewer","client":"claude-code"}"#);
        assert_eq!(serde_json::from_str::<Author>(&json)?, full);
        assert_eq!(full.to_string(), "reviewer (claude-code)");
        let same = Author::Agent {
            name: "claude-code".to_owned(),
            client: Some("claude-code".to_owned()),
            id: None,
            kind: None,
        };
        assert_eq!(same.to_string(), "claude-code");
        let signed = full.subscribed("s-1", "coder");
        let json = serde_json::to_string(&signed)?;
        assert_eq!(
            json,
            r#"{"name":"reviewer","client":"claude-code","id":"s-1","kind":"coder"}"#
        );
        assert_eq!(serde_json::from_str::<Author>(&json)?, signed);
        assert_eq!(signed.to_string(), "reviewer (coder)");
        assert_eq!(signed.id(), Some("s-1"));
        assert_eq!(Author::User.subscribed("s-1", "coder"), Author::User);
        Ok(())
    }

    /// A thread is pending for every agent while the user has the last
    /// word, and waiting on the user while an agent has it (ADR 0058):
    /// any agent's reply ends the one, and the user's reply, edit, or
    /// reopen ends the other.
    #[test]
    fn the_last_act_decides_whose_turn_it_is() -> Result<(), StoreError> {
        let file = TempFile::new("pending")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(3, 3),
                "why?",
            ),
            TEXT,
            10,
        )?;
        let thread = |store: &Store| {
            store
                .thread(&id)
                .cloned()
                .ok_or(StoreError::parse(0, "gone".into()))
        };
        let t = thread(&store)?;
        assert!(t.awaits_agent() && !t.awaits_user());
        assert_eq!(t.last_act(), (&Author::User, 10));
        assert_eq!(t.newest(), (&Author::User, 10));

        // Any agent's reply is the answer; a second session does not owe one.
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(me.clone(), 11, "because"))?;
        let t = thread(&store)?;
        assert!(!t.awaits_agent() && t.awaits_user());
        assert_eq!(t.last_act(), (&me, 11));

        // An edit of the comment, older than the reply, is the user's word.
        store.edit(&id, MessageTarget::Comment, "why, exactly?", 12)?;
        let t = thread(&store)?;
        assert!(t.awaits_agent());
        assert_eq!(t.last_act(), (&Author::User, 12));
        assert_eq!(t.comment_edited(), Some(12));
        assert_eq!(t.newest(), (&me, 11), "an edit is not a new message");

        store.reply(&id, Reply::new(me.clone(), 13, "ah"))?;
        store.reply(&id, Reply::new(Author::User, 14, "still"))?;
        store.edit(&id, MessageTarget::Reply(2), "still, yes", 15)?;
        let t = thread(&store)?;
        assert_eq!(t.replies()[2].edited(), Some(15));
        assert_eq!(t.last_act(), (&Author::User, 15));

        store.reply(
            &id,
            Reply::new(me.clone(), 16, "done").proposing_resolution(),
        )?;
        assert!(thread(&store)?.awaits_user());

        // Resolving ends both; reopening is the user's act, so the agent's turn.
        store.resolve(&id, 17)?;
        let t = thread(&store)?;
        assert!(!t.awaits_agent() && !t.awaits_user());
        store.reopen(&id, 18)?;
        let t = thread(&store)?;
        assert!(t.awaits_agent() && !t.awaits_user());
        assert_eq!(t.reopened(), Some(18));
        assert_eq!(t.last_act(), (&Author::User, 18));

        // At a tie a message beats a reopen.
        store.reply(&id, Reply::new(me.clone(), 18, "reopened, noted"))?;
        assert_eq!(thread(&store)?.last_act(), (&me, 18));
        assert!(thread(&store)?.awaits_user());

        let reloaded = Store::open(&file.0)?;
        assert_eq!(
            reloaded.thread(&id),
            store.thread(&id),
            "acts survive a reload"
        );
        Ok(())
    }

    /// An agent's comment is the agent's act, so its thread waits on the
    /// user from birth; the record says who only for an agent, so a
    /// user's record, and every record written before ADR 0061, loads
    /// as the user's.
    #[test]
    fn an_agents_comment_is_its_own_act() -> Result<(), StoreError> {
        let file = TempFile::new("agent-comment")?;
        let mut store = Store::open(&file.0)?;
        let bot = Author::agent("bot").subscribed("s-1", "coder");
        let theirs = store.annotate(
            Draft::new(bot.clone(), Path::new("a.md"), LineRange::new(3, 3), "look"),
            TEXT,
            10,
        )?;
        let mine = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(4, 4),
                "why?",
            ),
            TEXT,
            11,
        )?;
        let reloaded = Store::open(&file.0)?;
        for store in [&store, &reloaded] {
            let t = store
                .thread(&theirs)
                .ok_or(StoreError::parse(0, "gone".into()))?;
            assert_eq!(t.author(), &bot);
            assert_eq!(t.last_act(), (&bot, 10));
            assert!(t.awaits_user() && !t.awaits_agent());
            let t = store
                .thread(&mine)
                .ok_or(StoreError::parse(0, "gone".into()))?;
            assert_eq!(t.author(), &Author::User);
            assert!(t.awaits_agent() && !t.awaits_user());
        }
        let lines: Vec<String> = std::fs::read_to_string(&file.0)
            .map_err(|e| StoreError::parse(0, e.to_string()))?
            .lines()
            .map(str::to_owned)
            .collect();
        assert!(lines[0].contains("\"author\""), "{}", lines[0]);
        assert!(!lines[1].contains("\"author\""), "{}", lines[1]);
        Ok(())
    }

    #[test]
    fn two_handles_appending_to_one_file_keep_every_line_whole() -> Result<(), StoreError> {
        let file = TempFile::new("two-writers")?;
        let mut first = Store::open(&file.0)?;
        let id = first.annotate(
            Draft::new(
                Author::User,
                Path::new("README.md"),
                LineRange::new(3, 3),
                "one",
            ),
            TEXT,
            100,
        )?;
        // A second viewer, or a headless `--mcp` reply, opens its own handle.
        let mut second = Store::open(&file.0)?;
        for turn in 0..20 {
            first.reply(&id, Reply::new(Author::agent("a"), 200 + turn, "from a"))?;
            second.reply(&id, Reply::new(Author::agent("b"), 300 + turn, "from b"))?;
        }
        let text = fs::read_to_string(&file.0).map_err(|e| StoreError::io(&file.0, e))?;
        assert!(
            text.lines()
                .all(|line| line.starts_with('{') && line.ends_with('}'))
        );
        let merged = Store::open(&file.0)?;
        assert_eq!(merged.threads()[0].replies().len(), 40);
        Ok(())
    }

    #[test]
    fn store_round_trips_threads_replies_and_status() -> Result<(), StoreError> {
        let file = TempFile::new("roundtrip")?;
        let mut store = Store::open(&file.0)?;
        let draft = Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 4),
            "rename",
        );
        let id = store.annotate(draft, TEXT, 100)?;
        store.reply(
            &id,
            Reply::new(Author::agent("claude"), 101, "done").proposing_resolution(),
        )?;
        let other = store.annotate(
            Draft::new(
                Author::User,
                Path::new("docs/guide.md"),
                LineRange::new(1, 1),
                "hmm",
            ),
            TEXT,
            102,
        )?;
        store.resolve(&other, 103)?;
        store.resolve(&id, 104)?;
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
        assert!(thread.proposes_resolution());
        assert_eq!(thread.replies()[0].author().to_string(), "claude");
        assert_eq!(thread.updated(), 105);
        assert_eq!(
            again.thread(&other).map(super::Thread::status),
            Some(Status::Resolved)
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
        let file = TempFile::new("scope")?;
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
            Draft::new(Author::User, Path::new("a.md"), LineRange::new(2, 2), "new")
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

        let everywhere = Reach::everything();
        let on_branch = Reach::reachable(HashSet::from(["abc123".to_owned()]));
        let elsewhere = Reach::reachable(HashSet::new());
        let visible = |scope: &Reach| -> Vec<&ThreadId> {
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

    /// A rescope moves the thread to another commit and bumps `updated`,
    /// and survives a reload (ADR 0035).
    #[test]
    fn a_rescope_moves_the_thread_to_the_new_commit() -> Result<(), StoreError> {
        let file = TempFile::new("rescope")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), "hm")
                .at_commit(Some("old".to_owned())),
            TEXT,
            10,
        )?;
        store.rescope(&id, "new", 20)?;
        let again = Store::open(&file.0)?;
        let thread = again.thread(&id).ok_or(StoreError {
            kind: super::ErrorKind::UnknownThread(id.clone()),
        })?;
        assert_eq!(thread.commit(), Some("new"));
        assert_eq!(thread.updated(), 20);
        assert_eq!(thread.status(), Status::Open);
        assert!(Reach::reachable(HashSet::from(["new".to_owned()])).includes(thread));
        Ok(())
    }

    #[test]
    fn store_rejects_bad_ranges_unknown_threads_and_bad_lines() -> Result<(), StoreError> {
        let file = TempFile::new("errors")?;
        let mut store = Store::open(&file.0)?;
        let draft = Draft::new(Author::User, Path::new("a.md"), LineRange::new(9, 9), "x");
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
        let file = TempFile::new("placement")?;
        let mut store = Store::open(&file.0)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(5, 5),
                "gamma?",
            ),
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
