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

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::context::Context;
use sha2::{Digest, Sha256};

/// The format version written in every event line; [`Store::open`]
/// refuses a file of another (ADR 0062).
pub(crate) const FORMAT_VERSION: u32 = 4;

/// Maximum size of a persisted idempotency key, in bytes.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

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
    resolution_proposed: bool,
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
            resolution_proposed: false,
            edited: None,
        }
    }

    /// Mark the reply as proposing that the thread be resolved.
    #[must_use]
    pub fn proposing_resolution(mut self) -> Self {
        self.resolution_proposed = true;
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
        self.resolution_proposed
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

/// A message projected uniformly from a thread's comment or replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    target: MessageTarget,
    author: &'a Author,
    body: &'a str,
    created: u64,
    modified: u64,
    resolution_proposed: bool,
}

impl<'a> Message<'a> {
    /// Stable identity within the thread.
    #[must_use]
    pub fn target(&self) -> MessageTarget {
        self.target
    }

    /// Who wrote the message.
    #[must_use]
    pub fn author(&self) -> &'a Author {
        self.author
    }

    /// Message text.
    #[must_use]
    pub fn body(&self) -> &'a str {
        self.body
    }

    /// When the message was appended, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Creation or latest edit time, whichever is later.
    #[must_use]
    pub fn modified(&self) -> u64 {
        self.modified
    }

    /// Whether this message historically proposed resolution.
    #[must_use]
    pub fn resolution_proposed(&self) -> bool {
        self.resolution_proposed
    }
}

/// Messages in append order, including the opening comment.
#[derive(Debug, Clone)]
pub struct Messages<'a> {
    thread: &'a Thread,
    range: std::ops::Range<usize>,
}

impl<'a> Messages<'a> {
    fn project(&self, index: usize) -> Message<'a> {
        if index == 0 {
            Message {
                target: MessageTarget::Comment,
                author: &self.thread.author,
                body: &self.thread.comment,
                created: self.thread.created,
                modified: self
                    .thread
                    .comment_edited
                    .map_or(self.thread.created, |edited| {
                        edited.max(self.thread.created)
                    }),
                resolution_proposed: false,
            }
        } else {
            let reply_index = index - 1;
            let reply = &self.thread.replies[reply_index];
            Message {
                target: MessageTarget::Reply(reply_index),
                author: &reply.author,
                body: &reply.body,
                created: reply.created,
                modified: reply
                    .edited
                    .map_or(reply.created, |edited| edited.max(reply.created)),
                resolution_proposed: reply.resolution_proposed,
            }
        }
    }
}

impl<'a> Iterator for Messages<'a> {
    type Item = Message<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|index| self.project(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl DoubleEndedIterator for Messages<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.range.next_back().map(|index| self.project(index))
    }
}

impl ExactSizeIterator for Messages<'_> {}

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

/// Presentation lifecycle of an annotation thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// Unresolved without a current resolution proposal.
    Active,
    /// Unresolved with a current agent resolution proposal.
    ResolutionProposed,
    /// Resolved by the user or an authorized agent reply.
    Resolved,
}

/// One-shot permission for the next agent reply to resolve a thread.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoResolve {
    /// No agent reply may resolve the thread.
    #[default]
    Disabled,
    /// The next agent reply may resolve and consumes this permission.
    Enabled,
}

impl AutoResolve {
    /// Whether the one-shot permission is enabled.
    #[must_use]
    pub fn is_enabled(self) -> bool {
        self == Self::Enabled
    }

    fn toggled(self) -> Self {
        match self {
            Self::Disabled => Self::Enabled,
            Self::Enabled => Self::Disabled,
        }
    }
}

/// Extra state applied atomically with a user submission.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSubmit {
    /// Submit without changing existing one-shot permission.
    #[default]
    Normal,
    /// Submit and enable one-shot auto-resolve.
    EnableAutoResolve,
}

/// Result of a user write that refuses to reopen a resolved thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserWriteOutcome {
    /// The message or edit was persisted.
    Applied,
    /// The thread was resolved while the draft was open; nothing was written.
    ReopenRequired,
}

/// Whether an agent says its reply completes the work.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionIntent {
    /// The reply does not request resolution.
    #[default]
    NotRequested,
    /// The reply requests resolution.
    Resolve,
}

/// Durable result of an agent reply's resolution intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionOutcome {
    /// Resolution was not requested.
    NotRequested,
    /// Resolution was requested without one-shot permission.
    ResolutionProposed,
    /// Resolution was requested and authorized.
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
    #[serde(rename = "updated")]
    modified: u64,
    /// Who wrote the comment (ADR 0061); the user unless the record says
    /// otherwise, so it is written only for an agent.
    #[serde(default, skip_serializing_if = "Author::is_user")]
    author: Author,
    comment: String,
    replies: Vec<Reply>,
    lifecycle: Lifecycle,
    auto_resolve: AutoResolve,
    /// When the thread was last re-anchored to rewritten lines (ADR 0019).
    #[serde(default, rename = "edited", skip_serializing_if = "Option::is_none")]
    reanchored_at: Option<u64>,
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

    /// When the thread last changed, in Unix seconds.
    ///
    /// This includes messages, edits, lifecycle and auto-resolve changes,
    /// relocation, moves, and commit rescoping.
    #[must_use]
    pub fn modified(&self) -> u64 {
        self.modified
    }

    /// Compatibility name for [`Thread::modified`].
    #[must_use]
    pub fn updated(&self) -> u64 {
        self.modified()
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

    /// Opening comment and replies in append order.
    #[must_use]
    pub fn messages(&self) -> Messages<'_> {
        Messages {
            thread: self,
            range: 0..self.replies.len() + 1,
        }
    }

    /// Open or resolved.
    #[must_use]
    pub fn status(&self) -> Status {
        match self.lifecycle {
            Lifecycle::Active | Lifecycle::ResolutionProposed => Status::Open,
            Lifecycle::Resolved => Status::Resolved,
        }
    }

    /// Current presentation lifecycle.
    #[must_use]
    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// Current one-shot auto-resolve permission.
    #[must_use]
    pub fn auto_resolve(&self) -> AutoResolve {
        self.auto_resolve
    }

    /// Whether the thread currently has a resolution proposal.
    #[must_use]
    pub fn proposes_resolution(&self) -> bool {
        self.lifecycle == Lifecycle::ResolutionProposed
    }

    /// When the thread was last explicitly re-anchored.
    #[must_use]
    pub fn reanchored_at(&self) -> Option<u64> {
        self.reanchored_at
    }

    /// Compatibility name for [`Thread::reanchored_at`].
    #[must_use]
    pub fn edited(&self) -> Option<u64> {
        self.reanchored_at()
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
        self.status() == Status::Open && !self.last_act().0.is_user()
    }

    /// Whether the thread is open and the user has the last word, so an
    /// agent's reply would be the next act.
    #[must_use]
    pub fn awaits_agent(&self) -> bool {
        self.status() == Status::Open && self.last_act().0.is_user()
    }

    /// The newest message: the last reply, or the comment when there
    /// are none, as its author and `created` time.
    #[must_use]
    pub fn newest(&self) -> (&Author, u64) {
        self.messages()
            .next_back()
            .map_or((&self.author, self.created), |message| {
                (message.author(), message.created())
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
        match (anchor.locate_in(hashes, range), self.reanchored_at) {
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

/// The result of a write that optionally used an idempotency key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome<T> {
    value: T,
    replayed: bool,
}

impl<T> WriteOutcome<T> {
    fn applied(value: T) -> Self {
        Self {
            value,
            replayed: false,
        }
    }

    fn from_replay(value: T) -> Self {
        Self {
            value,
            replayed: true,
        }
    }

    /// The value produced by the write or recovered from the original write.
    #[must_use]
    pub fn value(&self) -> &T {
        &self.value
    }

    /// Consume the outcome and return its value.
    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }

    /// Whether the operation was already completed by an earlier request.
    #[must_use]
    pub fn replayed(&self) -> bool {
        self.replayed
    }
}

/// Parameters for one atomic agent reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReplyCommand {
    reply: Reply,
    resolution: ResolutionIntent,
    relocation: Option<LineRange>,
    head: Option<String>,
    idempotency: Option<Idempotency>,
}

impl AgentReplyCommand {
    /// Create an ordinary agent reply.
    #[must_use]
    pub fn new(author: Author, created: u64, body: impl Into<String>) -> Self {
        Self {
            reply: Reply::new(author, created, body),
            resolution: ResolutionIntent::NotRequested,
            relocation: None,
            head: None,
            idempotency: None,
        }
    }

    /// Request resolution as part of the reply.
    #[must_use]
    pub fn resolve(mut self) -> Self {
        self.resolution = ResolutionIntent::Resolve;
        self
    }

    /// Re-anchor the thread to `range` as part of the reply.
    #[must_use]
    pub fn relocate(mut self, range: LineRange) -> Self {
        self.relocation = Some(range);
        self
    }

    /// Supply the current checkout's `HEAD` for an authorized resolution.
    #[must_use]
    pub fn at_head(mut self, head: Option<String>) -> Self {
        self.head = head;
        self
    }

    /// Protect the reply with a caller-scoped idempotency key.
    #[must_use]
    pub fn idempotent(mut self, caller: impl Into<String>, key: impl Into<String>) -> Self {
        self.idempotency = Some(Idempotency {
            caller: caller.into(),
            key: key.into(),
        });
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Idempotency {
    caller: String,
    key: String,
}

/// Stable position after an append-log event.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActivityCursor(u64);

impl ActivityCursor {
    /// One-based append ordinal, or zero before the first event.
    #[must_use]
    pub fn ordinal(self) -> u64 {
        self.0
    }
}

/// An agent-authored opening comment or reply observed in the append log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivity {
    cursor: ActivityCursor,
    thread: ThreadId,
    target: MessageTarget,
    author: Author,
    path: PathBuf,
    range: Option<LineRange>,
    body: String,
    created: u64,
    resolution: ResolutionOutcome,
}

impl AgentActivity {
    /// Cursor immediately after this event.
    #[must_use]
    pub fn cursor(&self) -> ActivityCursor {
        self.cursor
    }

    /// Thread the message belongs to.
    #[must_use]
    pub fn thread(&self) -> &ThreadId {
        &self.thread
    }

    /// Stable message identity within the thread.
    #[must_use]
    pub fn target(&self) -> MessageTarget {
        self.target
    }

    /// Agent that authored the message.
    #[must_use]
    pub fn author(&self) -> &Author {
        &self.author
    }

    /// Path the thread occupied when the message was appended.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Range the thread occupied when the message was appended.
    #[must_use]
    pub fn range(&self) -> Option<LineRange> {
        self.range
    }

    /// Message text.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Append time in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Resolution result recorded by the containing operation.
    #[must_use]
    pub fn resolution(&self) -> ResolutionOutcome {
        self.resolution
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdempotentOperation {
    Start,
    Reply,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct IdempotencyReceipt {
    key: String,
    caller: String,
    operation: IdempotentOperation,
    intent: String,
    target: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<ResolutionOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReceiptKey {
    key: String,
    caller: String,
    operation: IdempotentOperation,
}

#[derive(Serialize)]
struct StartIntent<'a> {
    operation: IdempotentOperation,
    path: PathBuf,
    range: Option<LineRange>,
    body: &'a str,
}

#[derive(Serialize)]
struct ReplyIntent<'a> {
    operation: IdempotentOperation,
    thread: &'a ThreadId,
    body: &'a str,
    resolve: bool,
    lines: Option<LineRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReplyRelocation {
    range: LineRange,
    anchor: Anchor,
    created: u64,
    context: Context,
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
        auto_resolve: AutoResolve,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receipt: Option<IdempotencyReceipt>,
    },
    Reply {
        v: u32,
        thread: ThreadId,
        #[serde(flatten)]
        reply: Reply,
        submission: UserSubmit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relocation: Option<ReplyRelocation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receipt: Option<IdempotencyReceipt>,
    },
    /// One agent reply and all of its lifecycle effects.
    AgentReply {
        v: u32,
        thread: ThreadId,
        #[serde(flatten)]
        reply: Reply,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relocation: Option<ReplyRelocation>,
        resolution: ResolutionOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        receipt: Option<IdempotencyReceipt>,
    },
    /// The user replaced one of their messages (ADR 0013).
    Edit {
        v: u32,
        thread: ThreadId,
        target: MessageTarget,
        body: String,
        created: u64,
        submission: UserSubmit,
    },
    /// The user resolved the thread and optionally pinned it to `HEAD`.
    Resolve {
        v: u32,
        thread: ThreadId,
        created: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
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
    /// The user changed one-shot auto-resolve permission.
    SetAutoResolve {
        v: u32,
        thread: ThreadId,
        value: AutoResolve,
        created: u64,
    },
}

impl Event {
    /// The thread an event acts on; none for the one that creates it.
    fn thread_id(&self) -> Option<&ThreadId> {
        match self {
            Self::Annotate { .. } => None,
            Self::Reply { thread, .. }
            | Self::AgentReply { thread, .. }
            | Self::Edit { thread, .. }
            | Self::Resolve { thread, .. }
            | Self::Reopen { thread, .. }
            | Self::Relocate { thread, .. }
            | Self::Move { thread, .. }
            | Self::Delete { thread, .. }
            | Self::Rescope { thread, .. }
            | Self::SetAutoResolve { thread, .. } => Some(thread),
        }
    }

    fn receipt(&self) -> Option<&IdempotencyReceipt> {
        match self {
            Self::Annotate { receipt, .. }
            | Self::Reply { receipt, .. }
            | Self::AgentReply { receipt, .. } => receipt.as_ref(),
            Self::Edit { .. }
            | Self::Resolve { .. }
            | Self::Reopen { .. }
            | Self::Relocate { .. }
            | Self::Move { .. }
            | Self::Delete { .. }
            | Self::Rescope { .. }
            | Self::SetAutoResolve { .. } => None,
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
    receipts: HashMap<ReceiptKey, IdempotencyReceipt>,
    cursor: ActivityCursor,
    agent_activity: Vec<AgentActivity>,
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
            receipts: HashMap::new(),
            cursor: ActivityCursor::default(),
            agent_activity: Vec::new(),
        };
        let mut file = match OpenOptions::new().read(true).open(&store.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(store),
            Err(error) => return Err(StoreError::io(&store.path, error)),
        };
        file.lock_shared()
            .map_err(|error| StoreError::io(&store.path, error))?;
        let result = store.load_locked(&mut file);
        let _ = file.unlock();
        result?;
        tracing::debug!(path = %store.path.display(), threads = store.threads.len(), "loaded threads");
        Ok(store)
    }

    fn load_locked(&mut self, file: &mut File) -> Result<(), StoreError> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| StoreError::io(&self.path, error))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|error| StoreError::io(&self.path, error))?;
        let mut loaded = Self {
            path: self.path.clone(),
            threads: Vec::new(),
            deleted: HashSet::new(),
            receipts: HashMap::new(),
            cursor: ActivityCursor::default(),
            agent_activity: Vec::new(),
        };
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let Stamp { v } = serde_json::from_str(line)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
            if v != FORMAT_VERSION {
                return Err(StoreError::version(&self.path, index + 1, v));
            }
            let event: Event = serde_json::from_str(line)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
            let cursor = ActivityCursor(loaded.cursor.0 + 1);
            loaded
                .apply(event, cursor)
                .map_err(|error| StoreError::parse(index + 1, error.to_string()))?;
        }
        self.threads = loaded.threads;
        self.deleted = loaded.deleted;
        self.receipts = loaded.receipts;
        self.cursor = loaded.cursor;
        self.agent_activity = loaded.agent_activity;
        Ok(())
    }

    fn lock_for_write(&mut self) -> Result<File, StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| StoreError::io(parent, error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| StoreError::io(&self.path, error))?;
        file.lock()
            .map_err(|error| StoreError::io(&self.path, error))?;
        if let Err(error) = self.load_locked(&mut file) {
            let _ = file.unlock();
            return Err(error);
        }
        Ok(file)
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

    /// Cursor at the current end of the append log.
    #[must_use]
    pub fn activity_cursor(&self) -> ActivityCursor {
        self.cursor
    }

    /// Agent-authored message events appended after `cursor`.
    pub fn agent_activity_since(
        &self,
        cursor: ActivityCursor,
    ) -> impl Iterator<Item = &AgentActivity> {
        self.agent_activity
            .iter()
            .filter(move |activity| activity.cursor > cursor)
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
            .filter(|thread| thread.status() == Status::Open)
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
        self.annotate_with_submission(draft, text, now, UserSubmit::Normal)
    }

    /// Start a user thread and atomically apply submission state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `draft` is agent-authored, its range runs
    /// past the text, or the store cannot be appended to.
    pub fn annotate_user(
        &mut self,
        draft: Draft,
        text: &str,
        now: u64,
        submission: UserSubmit,
    ) -> Result<ThreadId, StoreError> {
        if !draft.author.is_user() {
            return Err(StoreError::message(
                "a user annotation requires a user-authored draft",
            ));
        }
        self.annotate_with_submission(draft, text, now, submission)
    }

    fn annotate_with_submission(
        &mut self,
        draft: Draft,
        text: &str,
        now: u64,
        submission: UserSubmit,
    ) -> Result<ThreadId, StoreError> {
        let (anchor, context, snippet) = capture_annotation(&draft, text)?;
        let mut file = self.lock_for_write()?;
        let id = ThreadId(format!(
            "{now}-{}-{}",
            std::process::id(),
            self.threads.len() + self.deleted.len() + 1
        ));
        let event = Event::Annotate {
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
            auto_resolve: auto_resolve_for_new_thread(submission),
            receipt: None,
        };
        let result = self.append_locked(&mut file, event);
        let _ = file.unlock();
        result?;
        Ok(id)
    }

    /// Start a thread with a durable per-caller idempotency key.
    ///
    /// The loader is called only when no matching receipt exists. This keeps
    /// retries safe after the source file has changed or disappeared.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key or caller identity is invalid, a
    /// matching key carries different arguments, the source cannot be loaded,
    /// or the event cannot be persisted.
    pub fn annotate_idempotent<F>(
        &mut self,
        draft: Draft,
        now: u64,
        key: &str,
        load_text: F,
    ) -> Result<WriteOutcome<ThreadId>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        let caller = caller_for(&draft.author, key)?.to_owned();
        self.annotate_idempotent_for_caller(draft, now, &caller, key, load_text)
    }

    /// Start a thread with a durable key scoped to an explicit caller.
    ///
    /// The caller is the authenticated, harness-qualified identity supplied
    /// by the request boundary. It is persisted in the receipt scope, but is
    /// not required to be duplicated in the annotation author.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key or caller identity is invalid, a
    /// matching key carries different arguments, the source cannot be loaded,
    /// or the event cannot be persisted.
    pub fn annotate_idempotent_for_caller<F>(
        &mut self,
        mut draft: Draft,
        now: u64,
        caller: &str,
        key: &str,
        load_text: F,
    ) -> Result<WriteOutcome<ThreadId>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        validate_caller(caller, key)?;
        draft.path = normalize_repo_path(&draft.path)?;
        let intent = start_intent(&draft)?;
        let receipt_key = ReceiptKey {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Start,
        };
        let mut file = self.lock_for_write()?;
        if let Some(receipt) = self.receipts.get(&receipt_key) {
            ensure_intent(receipt, &intent)?;
            if self.thread(&receipt.target).is_none() {
                return Err(StoreError {
                    kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
                });
            }
            let id = receipt.target.clone();
            let _ = file.unlock();
            return Ok(WriteOutcome::from_replay(id));
        }

        let text = load_text(&draft.path)?;
        let (anchor, context, snippet) = capture_annotation(&draft, &text)?;
        let id = ThreadId(format!(
            "{now}-{}-{}",
            std::process::id(),
            self.threads.len() + self.deleted.len() + 1
        ));
        let receipt = IdempotencyReceipt {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Start,
            intent,
            target: id.clone(),
            resolution: None,
        };
        let event = Event::Annotate {
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
            auto_resolve: AutoResolve::Disabled,
            receipt: Some(receipt),
        };
        self.append_locked(&mut file, event)?;
        let _ = file.unlock();
        Ok(WriteOutcome::applied(id))
    }

    /// Append `reply` to the thread `id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn reply(&mut self, id: &ThreadId, mut reply: Reply) -> Result<(), StoreError> {
        if reply.author().is_user() {
            reply.resolution_proposed = false;
            return self.commit(Event::Reply {
                v: FORMAT_VERSION,
                thread: id.clone(),
                reply,
                submission: UserSubmit::Normal,
                relocation: None,
                receipt: None,
            });
        }
        let resolution = if reply.proposes_resolution() {
            ResolutionIntent::Resolve
        } else {
            ResolutionIntent::NotRequested
        };
        reply.resolution_proposed = false;
        self.agent_reply_command(
            id,
            AgentReplyCommand {
                reply,
                resolution,
                relocation: None,
                head: None,
                idempotency: None,
            },
            |_| Err(StoreError::message("an ordinary reply has no relocation")),
        )
        .map(|_| ())
    }

    /// Append a user reply and atomically apply submission state.
    ///
    /// A reply to a resolved thread reopens it in the same event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the store cannot
    /// be appended to.
    pub fn reply_user(
        &mut self,
        id: &ThreadId,
        created: u64,
        body: impl Into<String>,
        submission: UserSubmit,
    ) -> Result<(), StoreError> {
        self.commit(Event::Reply {
            v: FORMAT_VERSION,
            thread: id.clone(),
            reply: Reply::new(Author::User, created, body),
            submission,
            relocation: None,
            receipt: None,
        })
    }

    /// Append a user reply only while the thread remains unresolved.
    ///
    /// The lifecycle check and append happen under one store lock.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the store cannot
    /// be read or appended to.
    pub fn reply_user_if_unresolved(
        &mut self,
        id: &ThreadId,
        created: u64,
        body: impl Into<String>,
        submission: UserSubmit,
    ) -> Result<UserWriteOutcome, StoreError> {
        let mut file = self.lock_for_write()?;
        let Some(thread) = self.thread(id) else {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::UnknownThread(id.clone()),
            });
        };
        let lifecycle = thread.lifecycle();
        if lifecycle == Lifecycle::Resolved {
            let _ = file.unlock();
            return Ok(UserWriteOutcome::ReopenRequired);
        }
        let result = self.append_locked(
            &mut file,
            Event::Reply {
                v: FORMAT_VERSION,
                thread: id.clone(),
                reply: Reply::new(Author::User, created, body),
                submission,
                relocation: None,
                receipt: None,
            },
        );
        let _ = file.unlock();
        result?;
        Ok(UserWriteOutcome::Applied)
    }

    /// Apply one agent reply and all related state in one append event.
    ///
    /// Idempotent replay is recognized under the store lock before thread
    /// lifecycle or relocation validation.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the author is not an agent, idempotency
    /// input conflicts, the thread is missing or resolved, relocation is
    /// invalid, source loading fails, or the append fails.
    pub fn agent_reply<F>(
        &mut self,
        id: &ThreadId,
        command: AgentReplyCommand,
        load_text: F,
    ) -> Result<WriteOutcome<ResolutionOutcome>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        self.agent_reply_command(id, command, load_text)
    }

    fn agent_reply_command<F>(
        &mut self,
        id: &ThreadId,
        mut command: AgentReplyCommand,
        load_text: F,
    ) -> Result<WriteOutcome<ResolutionOutcome>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        if command.reply.author().is_user() {
            return Err(StoreError::message(
                "an agent reply requires an agent author",
            ));
        }
        command.reply.resolution_proposed = false;
        let lines = command.relocation.map(normalize_range);
        let intent = agent_reply_intent(id, &command.reply, command.resolution, lines)?;
        let receipt_key = if let Some(idempotency) = &command.idempotency {
            validate_caller(&idempotency.caller, &idempotency.key)?;
            Some(ReceiptKey {
                key: idempotency.key.clone(),
                caller: idempotency.caller.clone(),
                operation: IdempotentOperation::Reply,
            })
        } else {
            None
        };

        let mut file = self.lock_for_write()?;
        if let Some(receipt_key) = &receipt_key
            && let Some(receipt) = self.receipts.get(receipt_key)
        {
            ensure_intent(receipt, &intent)?;
            if self.thread(&receipt.target).is_none() {
                return Err(StoreError {
                    kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
                });
            }
            let outcome = receipt.resolution.ok_or_else(|| {
                StoreError::message("reply receipt is missing its resolution outcome")
            })?;
            let _ = file.unlock();
            return Ok(WriteOutcome::from_replay(outcome));
        }

        let thread = self.thread(id).ok_or_else(|| StoreError {
            kind: ErrorKind::UnknownThread(id.clone()),
        })?;
        if thread.status() != Status::Open {
            return Err(StoreError::message(format!(
                "{id} is resolved; only the user can reopen it"
            )));
        }
        let relocation =
            capture_reply_relocation(thread, lines, command.reply.created(), load_text)?;
        let outcome = match command.resolution {
            ResolutionIntent::NotRequested => ResolutionOutcome::NotRequested,
            ResolutionIntent::Resolve if thread.auto_resolve().is_enabled() => {
                ResolutionOutcome::Resolved
            }
            ResolutionIntent::Resolve => ResolutionOutcome::ResolutionProposed,
        };
        command.reply.resolution_proposed = outcome == ResolutionOutcome::ResolutionProposed;
        let receipt = command.idempotency.map(|idempotency| IdempotencyReceipt {
            key: idempotency.key,
            caller: idempotency.caller,
            operation: IdempotentOperation::Reply,
            intent,
            target: id.clone(),
            resolution: Some(outcome),
        });
        self.append_locked(
            &mut file,
            Event::AgentReply {
                v: FORMAT_VERSION,
                thread: id.clone(),
                reply: command.reply,
                relocation,
                resolution: outcome,
                commit: (outcome == ResolutionOutcome::Resolved)
                    .then_some(command.head)
                    .flatten(),
                receipt,
            },
        )?;
        let _ = file.unlock();
        Ok(WriteOutcome::applied(outcome))
    }

    /// Append a reply and optional relocation under one durable receipt.
    ///
    /// The loader is called only for a new reply that supplies a relocation
    /// range. A replay therefore does not read, validate, or move the file's
    /// current contents again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key or caller identity is invalid, the
    /// target was deleted or resolved before a new reply, a matching key
    /// carries different arguments, a relocation cannot be captured, or the
    /// event cannot be persisted.
    pub fn reply_idempotent<F>(
        &mut self,
        id: &ThreadId,
        reply: Reply,
        lines: Option<LineRange>,
        key: &str,
        load_text: F,
    ) -> Result<WriteOutcome<()>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        let caller = caller_for(reply.author(), key)?.to_owned();
        self.reply_idempotent_for_caller(id, reply, lines, &caller, key, load_text)
    }

    /// Append a reply and optional relocation under an explicit caller scope.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key or caller identity is invalid, the
    /// target was deleted or resolved before a new reply, a matching key
    /// carries different arguments, a relocation cannot be captured, or the
    /// event cannot be persisted.
    pub fn reply_idempotent_for_caller<F>(
        &mut self,
        id: &ThreadId,
        reply: Reply,
        lines: Option<LineRange>,
        caller: &str,
        key: &str,
        load_text: F,
    ) -> Result<WriteOutcome<()>, StoreError>
    where
        F: FnOnce(&Path) -> Result<String, StoreError>,
    {
        if !reply.author().is_user() {
            let resolution = if reply.proposes_resolution() {
                ResolutionIntent::Resolve
            } else {
                ResolutionIntent::NotRequested
            };
            let command = AgentReplyCommand {
                reply,
                resolution,
                relocation: lines,
                head: None,
                idempotency: Some(Idempotency {
                    caller: caller.to_owned(),
                    key: key.to_owned(),
                }),
            };
            let outcome = self.agent_reply_command(id, command, load_text)?;
            return Ok(if outcome.replayed() {
                WriteOutcome::from_replay(())
            } else {
                WriteOutcome::applied(())
            });
        }

        validate_caller(caller, key)?;
        let lines = lines.map(normalize_range);
        let intent = reply_intent(id, &reply, lines)?;
        let receipt_key = ReceiptKey {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Reply,
        };
        let mut file = self.lock_for_write()?;
        if let Some(receipt) = self.receipts.get(&receipt_key) {
            ensure_intent(receipt, &intent)?;
            if self.thread(&receipt.target).is_none() {
                return Err(StoreError {
                    kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
                });
            }
            let _ = file.unlock();
            return Ok(WriteOutcome::from_replay(()));
        }

        let thread = self.thread(id).ok_or_else(|| StoreError {
            kind: ErrorKind::UnknownThread(id.clone()),
        })?;
        let relocation = capture_reply_relocation(thread, lines, reply.created(), load_text)?;
        let receipt = IdempotencyReceipt {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Reply,
            intent,
            target: id.clone(),
            resolution: None,
        };
        self.append_locked(
            &mut file,
            Event::Reply {
                v: FORMAT_VERSION,
                thread: id.clone(),
                reply,
                submission: UserSubmit::Normal,
                relocation,
                receipt: Some(receipt),
            },
        )?;
        let _ = file.unlock();
        Ok(WriteOutcome::applied(()))
    }

    /// Check whether a keyed thread start was already completed.
    ///
    /// This reloads the shared store under a shared file lock, so callers can
    /// make the replay decision before reading mutable source state. It never
    /// loads the source file and returns `None` when the key is new.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key, caller, path, or request intent is
    /// invalid, the stored request conflicts, the original thread was
    /// deleted, or the store cannot be read.
    pub fn probe_start_idempotency(
        &self,
        draft: &Draft,
        key: &str,
    ) -> Result<Option<ThreadId>, StoreError> {
        let caller = caller_for(&draft.author, key)?;
        self.probe_start_idempotency_for_caller(draft, caller, key)
    }

    /// Check whether a keyed thread start was already completed for an
    /// explicit caller scope.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key, caller, path, or stored intent is
    /// invalid, the request conflicts, the original thread was deleted, or
    /// the store cannot be read.
    pub fn probe_start_idempotency_for_caller(
        &self,
        draft: &Draft,
        caller: &str,
        key: &str,
    ) -> Result<Option<ThreadId>, StoreError> {
        validate_caller(caller, key)?;
        let receipt_key = ReceiptKey {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Start,
        };
        self.probe_idempotency(&receipt_key, &start_intent(draft)?)
    }

    /// Check whether a keyed reply was already completed.
    ///
    /// This has the same shared-lock and source-free behavior as
    /// [`Store::probe_start_idempotency`], allowing callers to skip mutable
    /// placement or lifecycle validation for a replay.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key, caller, thread, or request intent
    /// is invalid, the stored request conflicts, the original thread was
    /// deleted, or the store cannot be read.
    pub fn probe_reply_idempotency(
        &self,
        id: &ThreadId,
        reply: &Reply,
        lines: Option<LineRange>,
        key: &str,
    ) -> Result<Option<ThreadId>, StoreError> {
        let caller = caller_for(reply.author(), key)?;
        self.probe_reply_idempotency_for_caller(id, reply, lines, caller, key)
    }

    /// Check whether a keyed reply was already completed for an explicit
    /// caller scope.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the key, caller, thread, or stored intent
    /// is invalid, the request conflicts, the original thread was deleted,
    /// or the store cannot be read.
    pub fn probe_reply_idempotency_for_caller(
        &self,
        id: &ThreadId,
        reply: &Reply,
        lines: Option<LineRange>,
        caller: &str,
        key: &str,
    ) -> Result<Option<ThreadId>, StoreError> {
        validate_caller(caller, key)?;
        let lines = lines.map(normalize_range);
        let receipt_key = ReceiptKey {
            key: key.to_owned(),
            caller: caller.to_owned(),
            operation: IdempotentOperation::Reply,
        };
        self.probe_idempotency(&receipt_key, &reply_intent(id, reply, lines)?)
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
        self.edit_user(id, target, body, now, UserSubmit::Normal)
    }

    /// Edit a user message and atomically apply submission state.
    ///
    /// Editing a resolved thread reopens it in the same event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread or message is unknown, the
    /// message is agent-authored, or the append fails.
    pub fn edit_user(
        &mut self,
        id: &ThreadId,
        target: MessageTarget,
        body: impl Into<String>,
        now: u64,
        submission: UserSubmit,
    ) -> Result<(), StoreError> {
        self.commit(Event::Edit {
            v: FORMAT_VERSION,
            thread: id.clone(),
            target,
            body: body.into(),
            created: now,
            submission,
        })
    }

    /// Edit a user message only while the thread remains unresolved.
    ///
    /// The lifecycle check and append happen under one store lock.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread or message is unknown, the
    /// message is agent-authored, or the store cannot be read or appended to.
    pub fn edit_user_if_unresolved(
        &mut self,
        id: &ThreadId,
        target: MessageTarget,
        body: impl Into<String>,
        now: u64,
        submission: UserSubmit,
    ) -> Result<UserWriteOutcome, StoreError> {
        let mut file = self.lock_for_write()?;
        let Some(thread) = self.thread(id) else {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::UnknownThread(id.clone()),
            });
        };
        let lifecycle = thread.lifecycle();
        if lifecycle == Lifecycle::Resolved {
            let _ = file.unlock();
            return Ok(UserWriteOutcome::ReopenRequired);
        }
        let result = self.append_locked(
            &mut file,
            Event::Edit {
                v: FORMAT_VERSION,
                thread: id.clone(),
                target,
                body: body.into(),
                created: now,
                submission,
            },
        );
        let _ = file.unlock();
        result?;
        Ok(UserWriteOutcome::Applied)
    }

    /// Resolve the thread `id`, as the user, fixing it to `head`, the
    /// workspace's `HEAD` commit (ADR 0072), in the same append event.
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
        self.commit(Event::Resolve {
            v: FORMAT_VERSION,
            thread: id.clone(),
            created: now,
            commit: head.map(str::to_owned),
        })
    }

    /// Reopen the thread `id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the file cannot
    /// be appended to.
    pub fn reopen(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
        self.thread_mut(id)?;
        self.commit(Event::Reopen {
            v: FORMAT_VERSION,
            thread: id.clone(),
            created: now,
        })
    }

    /// Set one-shot auto-resolve permission on an unresolved thread.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is missing, resolved, or the
    /// append fails.
    pub fn set_auto_resolve(
        &mut self,
        id: &ThreadId,
        value: AutoResolve,
        now: u64,
    ) -> Result<(), StoreError> {
        self.commit(Event::SetAutoResolve {
            v: FORMAT_VERSION,
            thread: id.clone(),
            value,
            created: now,
        })
    }

    /// Toggle one-shot auto-resolve permission under the write lock.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is missing, resolved, or the
    /// append fails.
    pub fn toggle_auto_resolve(
        &mut self,
        id: &ThreadId,
        now: u64,
    ) -> Result<AutoResolve, StoreError> {
        let mut file = self.lock_for_write()?;
        let value = self
            .thread(id)
            .ok_or_else(|| StoreError {
                kind: ErrorKind::UnknownThread(id.clone()),
            })?
            .auto_resolve()
            .toggled();
        let result = self.append_locked(
            &mut file,
            Event::SetAutoResolve {
                v: FORMAT_VERSION,
                thread: id.clone(),
                value,
                created: now,
            },
        );
        let _ = file.unlock();
        result?;
        Ok(value)
    }

    /// Move the thread `id` onto `range` of `text`, the lines that replaced
    /// the ones it was written on (ADR 0019).
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
        let mut file = self.lock_for_write()?;
        if let Some(id) = event.thread_id()
            && self.thread(id).is_none()
        {
            let id = id.clone();
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::UnknownThread(id),
            });
        }
        let result = self.append_locked(&mut file, event);
        let _ = file.unlock();
        result
    }

    fn probe_idempotency(
        &self,
        receipt_key: &ReceiptKey,
        intent: &str,
    ) -> Result<Option<ThreadId>, StoreError> {
        let store = Self::open(&self.path)?;
        let Some(receipt) = store.receipts.get(receipt_key) else {
            return Ok(None);
        };
        ensure_intent(receipt, intent)?;
        if store.thread(&receipt.target).is_none() {
            return Err(StoreError {
                kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
            });
        }
        Ok(Some(receipt.target.clone()))
    }

    fn append_locked(&mut self, file: &mut File, event: Event) -> Result<(), StoreError> {
        let mut line = serde_json::to_string(&event).map_err(|error| StoreError {
            kind: ErrorKind::Parse(0, error.to_string()),
        })?;
        let before = (
            self.threads.clone(),
            self.deleted.clone(),
            self.receipts.clone(),
            self.cursor,
            self.agent_activity.clone(),
        );
        let cursor = ActivityCursor(self.cursor.0 + 1);
        if let Err(error) = self.apply(event, cursor) {
            (
                self.threads,
                self.deleted,
                self.receipts,
                self.cursor,
                self.agent_activity,
            ) = before;
            return Err(error);
        }
        line.push('\n');
        // One write for line and newline together: the lock prevents
        // cooperating writers from interleaving events, while syncing makes a
        // successful receipt survive a response-loss retry.
        if let Err(error) = file
            .write_all(line.as_bytes())
            .and_then(|()| file.sync_all())
        {
            (
                self.threads,
                self.deleted,
                self.receipts,
                self.cursor,
                self.agent_activity,
            ) = before;
            return Err(StoreError::io(&self.path, error));
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "one arm per event kind")]
    fn apply(&mut self, event: Event, cursor: ActivityCursor) -> Result<(), StoreError> {
        if let Some(receipt) = event.receipt() {
            let key = receipt_key(receipt);
            if let Some(existing) = self.receipts.get(&key) {
                if existing == receipt {
                    self.cursor = cursor;
                    return Ok(());
                }
                return Err(StoreError {
                    kind: ErrorKind::IdempotencyConflict(receipt.key.clone()),
                });
            }
        }
        if let Some(id) = event.thread_id()
            && self.deleted.contains(id)
        {
            self.cursor = cursor;
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
                auto_resolve,
                receipt,
                ..
            } => {
                if range.is_some() != anchor.is_some() || range.is_some() != context.is_some() {
                    return Err(StoreError {
                        kind: ErrorKind::AnnotationShape,
                    });
                }
                let activity = (!author.is_user()).then(|| AgentActivity {
                    cursor,
                    thread: id.clone(),
                    target: MessageTarget::Comment,
                    author: author.clone(),
                    path: path.clone(),
                    range,
                    body: comment.clone(),
                    created,
                    resolution: ResolutionOutcome::NotRequested,
                });
                self.threads.push(Thread {
                    id,
                    path,
                    range,
                    snippet,
                    anchor,
                    created,
                    modified: created,
                    author,
                    comment,
                    replies: Vec::new(),
                    lifecycle: Lifecycle::Active,
                    auto_resolve,
                    reanchored_at: None,
                    commit,
                    comment_edited: None,
                    reopened: None,
                    context,
                });
                if let Some(activity) = activity {
                    self.agent_activity.push(activity);
                }
                if let Some(receipt) = receipt {
                    self.receipts.insert(receipt_key(&receipt), receipt);
                }
            }
            Event::Reply {
                thread,
                reply,
                submission,
                relocation,
                receipt,
                ..
            } => {
                let mut activity = None;
                {
                    let thread = self.thread_mut(&thread)?;
                    let reply_index = thread.replies.len();
                    if let Some(relocation) = relocation {
                        thread.range = Some(relocation.range);
                        thread.anchor = Some(relocation.anchor);
                        thread.reanchored_at = Some(relocation.created);
                        thread.context = Some(relocation.context);
                    }
                    thread.modified = thread.modified.max(reply.created);
                    if reply.author.is_user() {
                        if thread.lifecycle == Lifecycle::Resolved {
                            thread.reopened = Some(reply.created);
                        }
                        thread.lifecycle = Lifecycle::Active;
                        apply_user_submission(thread, submission);
                    } else {
                        thread.auto_resolve = AutoResolve::Disabled;
                        thread.lifecycle = if reply.resolution_proposed {
                            Lifecycle::ResolutionProposed
                        } else {
                            Lifecycle::Active
                        };
                        activity = Some(AgentActivity {
                            cursor,
                            thread: thread.id.clone(),
                            target: MessageTarget::Reply(reply_index),
                            author: reply.author.clone(),
                            path: thread.path.clone(),
                            range: thread.range,
                            body: reply.body.clone(),
                            created: reply.created,
                            resolution: if reply.resolution_proposed {
                                ResolutionOutcome::ResolutionProposed
                            } else {
                                ResolutionOutcome::NotRequested
                            },
                        });
                    }
                    thread.replies.push(reply);
                };
                if let Some(activity) = activity {
                    self.agent_activity.push(activity);
                }
                if let Some(receipt) = receipt {
                    self.receipts.insert(receipt_key(&receipt), receipt);
                }
            }
            Event::AgentReply {
                thread,
                reply,
                relocation,
                resolution,
                commit,
                receipt,
                ..
            } => {
                let activity;
                {
                    let thread = self.thread_mut(&thread)?;
                    let reply_index = thread.replies.len();
                    if let Some(relocation) = relocation {
                        thread.range = Some(relocation.range);
                        thread.anchor = Some(relocation.anchor);
                        thread.reanchored_at = Some(relocation.created);
                        thread.context = Some(relocation.context);
                    }
                    thread.modified = thread.modified.max(reply.created);
                    thread.auto_resolve = AutoResolve::Disabled;
                    thread.lifecycle = match resolution {
                        ResolutionOutcome::NotRequested => Lifecycle::Active,
                        ResolutionOutcome::ResolutionProposed => Lifecycle::ResolutionProposed,
                        ResolutionOutcome::Resolved => Lifecycle::Resolved,
                    };
                    if resolution == ResolutionOutcome::Resolved
                        && let Some(commit) = commit
                    {
                        thread.commit = Some(commit);
                    }
                    activity = AgentActivity {
                        cursor,
                        thread: thread.id.clone(),
                        target: MessageTarget::Reply(reply_index),
                        author: reply.author.clone(),
                        path: thread.path.clone(),
                        range: thread.range,
                        body: reply.body.clone(),
                        created: reply.created,
                        resolution,
                    };
                    thread.replies.push(reply);
                };
                self.agent_activity.push(activity);
                if let Some(receipt) = receipt {
                    self.receipts.insert(receipt_key(&receipt), receipt);
                }
            }
            Event::Edit {
                thread,
                target,
                body,
                created,
                submission,
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
                thread.modified = thread.modified.max(created);
                if thread.lifecycle == Lifecycle::Resolved {
                    thread.lifecycle = Lifecycle::Active;
                    thread.reopened = Some(created);
                }
                apply_user_submission(thread, submission);
            }
            Event::Resolve {
                thread,
                created,
                commit,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.modified = thread.modified.max(created);
                thread.lifecycle = Lifecycle::Resolved;
                thread.auto_resolve = AutoResolve::Disabled;
                if let Some(commit) = commit {
                    thread.commit = Some(commit);
                }
            }
            Event::Reopen {
                thread, created, ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.modified = thread.modified.max(created);
                thread.lifecycle = Lifecycle::Active;
                thread.auto_resolve = AutoResolve::Disabled;
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
                thread.modified = thread.modified.max(created);
                thread.range = Some(range);
                thread.anchor = Some(anchor);
                thread.reanchored_at = Some(created);
                thread.context = Some(context);
            }
            Event::Move {
                thread,
                path,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.modified = thread.modified.max(created);
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
                thread.modified = thread.modified.max(created);
                thread.commit = Some(commit);
            }
            Event::SetAutoResolve {
                thread,
                value,
                created,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                if thread.lifecycle == Lifecycle::Resolved {
                    return Err(StoreError::message(format!(
                        "{} is resolved; reopen it before changing auto-resolve",
                        thread.id
                    )));
                }
                thread.modified = thread.modified.max(created);
                thread.auto_resolve = value;
            }
        }
        self.cursor = cursor;
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

fn caller_for<'a>(author: &'a Author, key: &str) -> Result<&'a str, StoreError> {
    let caller = author.id().ok_or(StoreError {
        kind: ErrorKind::IdempotencyCaller,
    })?;
    validate_caller(caller, key)?;
    Ok(caller)
}

fn validate_caller(caller: &str, key: &str) -> Result<(), StoreError> {
    validate_key(key)?;
    if caller.trim().is_empty() {
        return Err(StoreError {
            kind: ErrorKind::IdempotencyCaller,
        });
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), StoreError> {
    if key.trim().is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(StoreError {
            kind: ErrorKind::InvalidIdempotencyKey,
        });
    }
    Ok(())
}

fn receipt_key(receipt: &IdempotencyReceipt) -> ReceiptKey {
    ReceiptKey {
        key: receipt.key.clone(),
        caller: receipt.caller.clone(),
        operation: receipt.operation,
    }
}

fn ensure_intent(receipt: &IdempotencyReceipt, intent: &str) -> Result<(), StoreError> {
    if receipt.intent == intent {
        Ok(())
    } else {
        Err(StoreError {
            kind: ErrorKind::IdempotencyConflict(receipt.key.clone()),
        })
    }
}

fn normalize_range(range: LineRange) -> LineRange {
    LineRange::new(range.start(), range.end())
}

fn auto_resolve_for_new_thread(submission: UserSubmit) -> AutoResolve {
    match submission {
        UserSubmit::Normal => AutoResolve::Disabled,
        UserSubmit::EnableAutoResolve => AutoResolve::Enabled,
    }
}

fn apply_user_submission(thread: &mut Thread, submission: UserSubmit) {
    if submission == UserSubmit::EnableAutoResolve {
        thread.auto_resolve = AutoResolve::Enabled;
    }
}

fn normalize_repo_path(path: &Path) -> Result<PathBuf, StoreError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(StoreError {
                        kind: ErrorKind::InvalidPath(path.to_path_buf()),
                    });
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(StoreError {
                    kind: ErrorKind::InvalidPath(path.to_path_buf()),
                });
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(StoreError {
            kind: ErrorKind::InvalidPath(path.to_path_buf()),
        });
    }
    Ok(normalized)
}

fn start_intent(draft: &Draft) -> Result<String, StoreError> {
    let intent = StartIntent {
        operation: IdempotentOperation::Start,
        path: normalize_repo_path(&draft.path)?,
        range: draft.range.map(normalize_range),
        body: &draft.comment,
    };
    serde_json::to_string(&intent).map_err(|error| StoreError {
        kind: ErrorKind::Parse(0, error.to_string()),
    })
}

fn reply_intent(
    id: &ThreadId,
    reply: &Reply,
    lines: Option<LineRange>,
) -> Result<String, StoreError> {
    let intent = ReplyIntent {
        operation: IdempotentOperation::Reply,
        thread: id,
        body: reply.body(),
        resolve: reply.proposes_resolution(),
        lines,
    };
    serde_json::to_string(&intent).map_err(|error| StoreError {
        kind: ErrorKind::Parse(0, error.to_string()),
    })
}

fn agent_reply_intent(
    id: &ThreadId,
    reply: &Reply,
    resolution: ResolutionIntent,
    lines: Option<LineRange>,
) -> Result<String, StoreError> {
    let intent = ReplyIntent {
        operation: IdempotentOperation::Reply,
        thread: id,
        body: reply.body(),
        resolve: resolution == ResolutionIntent::Resolve,
        lines,
    };
    serde_json::to_string(&intent).map_err(|error| StoreError {
        kind: ErrorKind::Parse(0, error.to_string()),
    })
}

fn capture_reply_relocation<F>(
    thread: &Thread,
    lines: Option<LineRange>,
    created: u64,
    load_text: F,
) -> Result<Option<ReplyRelocation>, StoreError>
where
    F: FnOnce(&Path) -> Result<String, StoreError>,
{
    let Some(range) = lines else {
        return Ok(None);
    };
    if thread.is_on_file() {
        return Err(StoreError {
            kind: ErrorKind::OnFile(thread.id.clone()),
        });
    }
    let text = load_text(thread.path())?;
    if thread.range() == Some(range)
        && matches!(
            thread.locate(&text),
            Placement::Anchored(current) | Placement::Edited(current) if current == range
        )
    {
        return Ok(None);
    }
    let anchor = Anchor::capture(&text, range).ok_or(StoreError {
        kind: ErrorKind::BadRange(range),
    })?;
    let context = Context::capture(&text, range).ok_or(StoreError {
        kind: ErrorKind::BadRange(range),
    })?;
    Ok(Some(ReplyRelocation {
        range,
        anchor,
        created,
        context,
    }))
}

fn capture_annotation(
    draft: &Draft,
    text: &str,
) -> Result<(Option<Anchor>, Option<Context>, String), StoreError> {
    match draft.range {
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
            Ok((Some(anchor), Some(context), snippet))
        }
        None => Ok((None, None, String::new())),
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
    InvalidIdempotencyKey,
    IdempotencyCaller,
    IdempotencyConflict(String),
    IdempotencyDeleted(ThreadId),
    InvalidPath(PathBuf),
    Message(String),
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

    /// Create an error for a source or validation failure discovered by a
    /// caller-supplied idempotent write loader.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Message(message.into()),
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
            ErrorKind::InvalidIdempotencyKey => write!(
                f,
                "idempotency key must contain non-whitespace text and be at most \
                 {MAX_IDEMPOTENCY_KEY_BYTES} bytes"
            ),
            ErrorKind::IdempotencyCaller => {
                f.write_str("idempotency keys require a nonempty authenticated caller identity")
            }
            ErrorKind::IdempotencyConflict(key) => {
                write!(
                    f,
                    "idempotency key {key:?} conflicts with a different request"
                )
            }
            ErrorKind::IdempotencyDeleted(id) => write!(
                f,
                "idempotency key cannot replay: original thread {id} was deleted"
            ),
            ErrorKind::InvalidPath(path) => {
                write!(
                    f,
                    "path {} is not a repository-relative path",
                    path.display()
                )
            }
            ErrorKind::Message(message) => f.write_str(message),
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
