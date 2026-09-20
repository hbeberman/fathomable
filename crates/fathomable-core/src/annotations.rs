// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Annotation threads, content anchors, and the append-only JSONL store.
//!
//! A [`Thread`] is one user comment on a [`LineRange`] of a workspace file
//! plus its replies (ADR 0005). Threads are keyed to content, not position:
//! an [`Anchor`] records a short hash of every annotated line and of the
//! lines immediately above and below, and [`Thread::locate`] finds the range
//! again after the file changes. When the lines are gone the thread is
//! [`Placement::Detached`] at its last known range rather than lost. Each
//! line thread carries bounded [`Context`] evidence for offline anchoring.
//! Immutable [`Origin`] provenance is separate from mutable placement,
//! resolution history, and orthogonal archival state. A file-wide thread has
//! no range, anchor, or context.
//!
//! Every change is one JSON line appended to `threads.jsonl` under the
//! workspace's state directory (ADR 0013); [`Store::open`] folds the file
//! back into threads. Normal iteration excludes archived history, while
//! [`Store::thread`] can inspect an archived ID. Timestamps are supplied by
//! the caller so the module stays pure and testable. New comment, reply, and
//! edit bodies are limited to [`MAX_MESSAGE_BYTES`]; older larger events
//! remain readable.
//!
//! # Examples
//!
//! ```no_run
//! use std::path::Path;
//! use fathomable_core::annotations::{Author, Draft, LineRange, Store};
//!
//! let mut store = Store::open("private-state/threads.jsonl")?;
//! let text = "# Title\n\nalpha\nbeta\n";
//! let draft = Draft::new(Author::User, Path::new("README.md"), LineRange::new(3, 4), "rename these");
//! let id = store.annotate(draft, text, 1_700_000_000)?;
//! assert_eq!(store.thread(&id).map(|t| t.snippet()), Some("alpha\nbeta"));
//! # Ok::<(), fathomable_core::annotations::StoreError>(())
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::workspace::CommitId;
use sha2::{Digest, Sha256};

/// The format version written in every event line; [`Store::open`]
/// refuses a file of another (ADR 0062).
pub(crate) const FORMAT_VERSION: u32 = 5;

/// Maximum size of a persisted idempotency key, in bytes.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// Maximum size of a persisted thread comment or reply, in UTF-8 bytes.
pub const MAX_MESSAGE_BYTES: usize = 1024;

/// Maximum number of bytes retained for immutable origin snippets.
pub const MAX_ORIGIN_EVIDENCE_BYTES: usize = 16 * 1024;

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
///
/// Deserialization rejects zero or reversed bounds instead of normalizing
/// them as [`Self::new`] does. Serialized ranges must already be valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct LineRange {
    start: usize,
    end: usize,
}

impl<'de> Deserialize<'de> for LineRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename = "LineRange")]
        struct Bounds {
            start: usize,
            end: usize,
        }

        let Bounds { start, end } = Bounds::deserialize(deserializer)?;
        if start == 0 {
            return Err(serde::de::Error::custom(
                "line range start must be at least 1",
            ));
        }
        if end < start {
            return Err(serde::de::Error::custom(
                "line range end must not precede its start",
            ));
        }
        Ok(Self { start, end })
    }
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

/// The version that supplied a thread's original source evidence.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OriginVersion {
    /// An immutable Git commit.
    Commit {
        /// The resolved object ID.
        id: String,
    },
    /// The checkout's final working-tree content.
    WorkingTree {
        /// The checkout `HEAD` observed with the content, when available.
        observed_head: Option<String>,
    },
    /// The checkout's staged index content.
    Index {
        /// The checkout `HEAD` observed with the index, when available.
        observed_head: Option<String>,
    },
    /// Content captured at an explicit workspace review point.
    ReviewPoint {
        /// Stable review-point identifier.
        id: String,
        /// Git baseline used by the point, when available.
        base: Option<String>,
    },
    /// The empty tree used before a repository's root commit.
    EmptyTree,
    /// No human comparison endpoint was supplied.
    #[default]
    Unknown,
}

impl OriginVersion {
    /// Construct a commit version.
    #[must_use]
    pub fn commit(id: impl Into<String>) -> Self {
        Self::Commit { id: id.into() }
    }

    /// Construct a working-tree version.
    #[must_use]
    pub fn working_tree(observed_head: Option<String>) -> Self {
        Self::WorkingTree { observed_head }
    }

    /// Construct an index version.
    #[must_use]
    pub fn index(observed_head: Option<String>) -> Self {
        Self::Index { observed_head }
    }

    /// Construct a review-point version.
    #[must_use]
    pub fn review_point(id: impl Into<String>, base: Option<String>) -> Self {
        Self::ReviewPoint {
            id: id.into(),
            base,
        }
    }

    /// The immutable commit ID when this is a commit version.
    #[must_use]
    pub fn commit_id(&self) -> Option<&str> {
        match self {
            Self::Commit { id } => Some(id),
            Self::WorkingTree { .. }
            | Self::Index { .. }
            | Self::ReviewPoint { .. }
            | Self::EmptyTree
            | Self::Unknown => None,
        }
    }
}

/// Which side of a human comparison supplied the original lines.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginSide {
    /// Lines from the comparison base.
    Base,
    /// Lines from the comparison target.
    Target,
    /// No human comparison side was supplied.
    #[default]
    Unspecified,
}

/// The endpoint pair a person was viewing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComparisonFacts {
    base: OriginVersion,
    target: OriginVersion,
}

impl ComparisonFacts {
    /// Construct a comparison of `base` to `target`.
    #[must_use]
    pub fn new(base: OriginVersion, target: OriginVersion) -> Self {
        Self { base, target }
    }

    /// The selected base endpoint.
    #[must_use]
    pub fn base(&self) -> &OriginVersion {
        &self.base
    }

    /// The selected target endpoint.
    #[must_use]
    pub fn target(&self) -> &OriginVersion {
        &self.target
    }
}

/// Facts observed for a working-tree source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingTreeState {
    /// The path matched the observed committed baseline.
    Clean,
    /// Existing content changed in the working tree.
    Modified,
    /// The path exists only in the working tree.
    Added,
    /// The baseline path is absent from the working tree.
    Deleted,
}

/// Facts observed for a working-tree source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkingTreeFacts {
    observed_head: Option<String>,
    dirty: bool,
    added: bool,
    deleted: bool,
    content: Option<ContentIdentity>,
}

impl WorkingTreeFacts {
    /// Construct working-tree facts.
    #[must_use]
    pub fn new(
        observed_head: Option<String>,
        state: WorkingTreeState,
        content: Option<ContentIdentity>,
    ) -> Self {
        let (dirty, added, deleted) = match state {
            WorkingTreeState::Clean => (false, false, false),
            WorkingTreeState::Modified => (true, false, false),
            WorkingTreeState::Added => (true, true, false),
            WorkingTreeState::Deleted => (true, false, true),
        };
        Self {
            observed_head,
            dirty,
            added,
            deleted,
            content,
        }
    }

    /// The observed checkout `HEAD`, when available.
    #[must_use]
    pub fn observed_head(&self) -> Option<&str> {
        self.observed_head.as_deref()
    }

    /// Whether the working file differed from its observed `HEAD`.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the path was added relative to the observed `HEAD`.
    #[must_use]
    pub fn is_added(&self) -> bool {
        self.added
    }

    /// Whether the path was deleted relative to the observed `HEAD`.
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// The captured content identity, when content was available.
    #[must_use]
    pub fn content(&self) -> Option<&ContentIdentity> {
        self.content.as_ref()
    }
}

/// Facts observed for staged index content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    /// The path has no staged change.
    Unchanged,
    /// The path has staged content.
    Staged,
}

/// Facts observed for staged index content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexFacts {
    observed_head: Option<String>,
    staged: bool,
    content: Option<ContentIdentity>,
}

impl IndexFacts {
    /// Construct index facts.
    #[must_use]
    pub fn new(
        observed_head: Option<String>,
        state: IndexState,
        content: Option<ContentIdentity>,
    ) -> Self {
        Self {
            observed_head,
            staged: state == IndexState::Staged,
            content,
        }
    }

    /// The observed checkout `HEAD`, when available.
    #[must_use]
    pub fn observed_head(&self) -> Option<&str> {
        self.observed_head.as_deref()
    }

    /// Whether the path was staged at capture time.
    #[must_use]
    pub fn is_staged(&self) -> bool {
        self.staged
    }

    /// The captured content identity, when content was available.
    #[must_use]
    pub fn content(&self) -> Option<&ContentIdentity> {
        self.content.as_ref()
    }
}

/// Facts identifying an explicit review-point source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReviewPointFacts {
    id: String,
    base: Option<String>,
    content: Option<ContentIdentity>,
}

impl ReviewPointFacts {
    /// Construct review-point facts.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        base: Option<String>,
        content: Option<ContentIdentity>,
    ) -> Self {
        Self {
            id: id.into(),
            base,
            content,
        }
    }

    /// The stable review-point identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The Git baseline used by the point, when available.
    #[must_use]
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// The captured content identity, when content was available.
    #[must_use]
    pub fn content(&self) -> Option<&ContentIdentity> {
        self.content.as_ref()
    }
}

/// A stable identity for captured content, not a reconstructable snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentIdentity {
    hash: String,
    bytes: usize,
}

impl ContentIdentity {
    /// Hash `text` without retaining the complete file.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            hash: short_hash(text.as_bytes()),
            bytes: text.len(),
        }
    }

    /// The short SHA-256 identity.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Number of bytes represented by the identity.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Optional provenance supplied at a thread's input boundary.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    version: OriginVersion,
    #[serde(default)]
    side: OriginSide,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comparison: Option<ComparisonFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_tree: Option<WorkingTreeFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index: Option<IndexFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review_point: Option<ReviewPointFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<ContentIdentity>,
    #[serde(default)]
    evidence_truncated: bool,
}

impl Provenance {
    /// Construct provenance for a source version and comparison side.
    #[must_use]
    pub fn new(version: OriginVersion, side: OriginSide) -> Self {
        Self {
            version,
            side,
            ..Self::default()
        }
    }

    /// Set the source version.
    #[must_use]
    pub fn with_version(mut self, version: OriginVersion) -> Self {
        self.version = version;
        self
    }

    /// Set the source side.
    #[must_use]
    pub fn with_side(mut self, side: OriginSide) -> Self {
        self.side = side;
        self
    }

    /// Set the selected comparison facts.
    #[must_use]
    pub fn with_comparison(mut self, comparison: ComparisonFacts) -> Self {
        self.comparison = Some(comparison);
        self
    }

    /// Set working-tree facts.
    #[must_use]
    pub fn with_working_tree(mut self, facts: WorkingTreeFacts) -> Self {
        self.working_tree = Some(facts);
        self
    }

    /// Set index facts.
    #[must_use]
    pub fn with_index(mut self, facts: IndexFacts) -> Self {
        self.index = Some(facts);
        self
    }

    /// Set review-point facts.
    #[must_use]
    pub fn with_review_point(mut self, facts: ReviewPointFacts) -> Self {
        self.review_point = Some(facts);
        self
    }

    /// Set the captured content identity.
    #[must_use]
    pub fn with_content(mut self, content: ContentIdentity) -> Self {
        self.content = Some(content);
        self
    }

    /// Mark the bounded evidence as truncated.
    #[must_use]
    pub fn truncated(mut self) -> Self {
        self.evidence_truncated = true;
        self
    }

    /// The source version supplying the lines.
    #[must_use]
    pub fn version(&self) -> &OriginVersion {
        &self.version
    }

    /// The comparison side supplying the lines.
    #[must_use]
    pub fn side(&self) -> OriginSide {
        self.side
    }

    /// The human comparison, when one was supplied.
    #[must_use]
    pub fn comparison(&self) -> Option<&ComparisonFacts> {
        self.comparison.as_ref()
    }

    /// Working-tree facts, when supplied.
    #[must_use]
    pub fn working_tree(&self) -> Option<&WorkingTreeFacts> {
        self.working_tree.as_ref()
    }

    /// Index facts, when supplied.
    #[must_use]
    pub fn index(&self) -> Option<&IndexFacts> {
        self.index.as_ref()
    }

    /// Review-point facts, when supplied.
    #[must_use]
    pub fn review_point(&self) -> Option<&ReviewPointFacts> {
        self.review_point.as_ref()
    }

    /// Content identity, when supplied.
    #[must_use]
    pub fn content(&self) -> Option<&ContentIdentity> {
        self.content.as_ref()
    }

    /// Whether retained evidence was bounded or truncated.
    #[must_use]
    pub fn evidence_truncated(&self) -> bool {
        self.evidence_truncated
    }
}

/// Immutable source evidence captured when a thread is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<Context>,
    #[serde(default)]
    provenance: Provenance,
}

impl Origin {
    /// Capture immutable source evidence for a thread.
    #[must_use]
    pub fn new(
        path: &Path,
        range: Option<LineRange>,
        snippet: impl AsRef<str>,
        anchor: Option<Anchor>,
        context: Option<Context>,
    ) -> Self {
        let (snippet, truncated) = bound_evidence(snippet.as_ref());
        let context_truncated = context.as_ref().is_some_and(Context::is_truncated);
        let provenance = Provenance::default().with_content(ContentIdentity::from_text(&snippet));
        Self {
            path: path.to_path_buf(),
            range,
            snippet,
            anchor,
            context,
            provenance: if truncated || context_truncated {
                provenance.truncated()
            } else {
                provenance
            },
        }
    }

    /// Attach input-bound provenance facts without changing source evidence.
    #[must_use]
    pub fn with_provenance(mut self, mut provenance: Provenance) -> Self {
        if provenance.content.is_none() {
            provenance.content = Some(ContentIdentity::from_text(&self.snippet));
        }
        if self.snippet.len() > MAX_ORIGIN_EVIDENCE_BYTES {
            provenance.evidence_truncated = true;
        }
        self.provenance = provenance;
        self
    }

    /// The original workspace-relative path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The original line range, or `None` for a file-wide thread.
    #[must_use]
    pub fn range(&self) -> Option<LineRange> {
        self.range
    }

    /// The bounded original snippet.
    #[must_use]
    pub fn snippet(&self) -> &str {
        &self.snippet
    }

    /// The original content anchor, when this is a line thread.
    #[must_use]
    pub fn anchor(&self) -> Option<&Anchor> {
        self.anchor.as_ref()
    }

    /// The original bounded context window, when this is a line thread.
    #[must_use]
    pub fn context(&self) -> Option<&Context> {
        self.context.as_ref()
    }

    /// The immutable provenance facts.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// The source version that supplied the original lines.
    #[must_use]
    pub fn version(&self) -> &OriginVersion {
        self.provenance.version()
    }

    /// The side that supplied the original lines.
    #[must_use]
    pub fn side(&self) -> OriginSide {
        self.provenance.side()
    }

    /// The human comparison, when supplied.
    #[must_use]
    pub fn comparison(&self) -> Option<&ComparisonFacts> {
        self.provenance.comparison()
    }

    /// Working-tree facts, when supplied.
    #[must_use]
    pub fn working_tree(&self) -> Option<&WorkingTreeFacts> {
        self.provenance.working_tree()
    }

    /// Index facts, when supplied.
    #[must_use]
    pub fn index(&self) -> Option<&IndexFacts> {
        self.provenance.index()
    }

    /// Review-point facts, when supplied.
    #[must_use]
    pub fn review_point(&self) -> Option<&ReviewPointFacts> {
        self.provenance.review_point()
    }

    /// The immutable content identity.
    #[must_use]
    pub fn content(&self) -> Option<&ContentIdentity> {
        self.provenance.content()
    }

    /// Whether the source evidence was bounded or truncated.
    #[must_use]
    pub fn evidence_truncated(&self) -> bool {
        self.provenance.evidence_truncated()
    }
}

/// Mutable evidence describing where a thread is currently projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementEvidence {
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<Anchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<Context>,
    #[serde(default)]
    version: OriginVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at: Option<u64>,
}

impl PlacementEvidence {
    /// Construct current placement from immutable origin evidence.
    #[must_use]
    pub fn from_origin(origin: &Origin, version: OriginVersion) -> Self {
        Self {
            path: origin.path.clone(),
            range: origin.range,
            anchor: origin.anchor.clone(),
            context: origin.context.clone(),
            version,
            checkout: None,
            observed_at: None,
        }
    }

    /// The currently projected path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The currently projected range.
    #[must_use]
    pub fn range(&self) -> Option<LineRange> {
        self.range
    }

    /// The current placement anchor.
    #[must_use]
    pub fn anchor(&self) -> Option<&Anchor> {
        self.anchor.as_ref()
    }

    /// The current placement context.
    #[must_use]
    pub fn context(&self) -> Option<&Context> {
        self.context.as_ref()
    }

    /// The version or checkout qualified by this placement.
    #[must_use]
    pub fn version(&self) -> &OriginVersion {
        &self.version
    }

    /// The checkout that supplied this placement evidence, when known.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The time of the latest explicit placement update.
    #[must_use]
    pub fn observed_at(&self) -> Option<u64> {
        self.observed_at
    }
}

/// Version and checkout qualification for newly captured placement evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementContext {
    version: OriginVersion,
    checkout: Option<String>,
}

impl PlacementContext {
    /// Qualify placement evidence with its source version.
    #[must_use]
    pub fn new(version: OriginVersion) -> Self {
        Self {
            version,
            checkout: None,
        }
    }

    /// Qualify placement evidence with its checkout.
    #[must_use]
    pub fn at_checkout(mut self, checkout: impl Into<String>) -> Self {
        self.checkout = Some(checkout.into());
        self
    }
}

fn bound_evidence(text: &str) -> (String, bool) {
    if text.len() <= MAX_ORIGIN_EVIDENCE_BYTES {
        return (text.to_owned(), false);
    }
    let mut end = MAX_ORIGIN_EVIDENCE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
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

/// Context recorded for one resolution event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionContext {
    actor: Author,
    checkout: Option<String>,
    version: Option<OriginVersion>,
    head: Option<String>,
}

impl ResolutionContext {
    /// Construct a resolution context for `actor` and `head`.
    #[must_use]
    pub fn new(actor: Author, head: Option<String>) -> Self {
        let version = head.clone().map(OriginVersion::commit);
        Self {
            actor,
            checkout: None,
            version,
            head,
        }
    }

    /// Add the bound checkout identity.
    #[must_use]
    pub fn at_checkout(mut self, checkout: impl Into<String>) -> Self {
        self.checkout = Some(checkout.into());
        self
    }

    /// Add the version supplying the resolution context.
    #[must_use]
    pub fn at_version(mut self, version: OriginVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// The actor that resolved the thread.
    #[must_use]
    pub fn actor(&self) -> &Author {
        &self.actor
    }

    /// The bound checkout identity, when supplied.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The resolution version, when supplied.
    #[must_use]
    pub fn version(&self) -> Option<&OriginVersion> {
        self.version.as_ref()
    }

    /// The observed `HEAD`, when available.
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }
}

/// One immutable resolution event in a thread's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionRecord {
    created: u64,
    actor: Author,
    checkout: Option<String>,
    version: Option<OriginVersion>,
    head: Option<String>,
    #[serde(default)]
    ordinal: u64,
}

impl ResolutionRecord {
    fn from_context(context: ResolutionContext, created: u64, ordinal: u64) -> Self {
        Self {
            created,
            actor: context.actor,
            checkout: context.checkout,
            version: context.version,
            head: context.head,
            ordinal,
        }
    }

    /// When this resolution happened, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Who resolved the thread.
    #[must_use]
    pub fn actor(&self) -> &Author {
        &self.actor
    }

    /// The bound checkout identity, when supplied.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The version supplying the resolution context, when supplied.
    #[must_use]
    pub fn version(&self) -> Option<&OriginVersion> {
        self.version.as_ref()
    }

    /// The observed `HEAD`, when available.
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// Append-log order used to break equal-time ordering ties.
    #[must_use]
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }
}

/// Context recorded when a thread is archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveContext {
    actor: Author,
    created: u64,
    checkout: Option<String>,
    version: Option<OriginVersion>,
}

impl ArchiveContext {
    /// Construct archive context for `actor` at `created`.
    #[must_use]
    pub fn new(actor: Author, created: u64) -> Self {
        Self {
            actor,
            created,
            checkout: None,
            version: None,
        }
    }

    /// Add the bound checkout identity.
    #[must_use]
    pub fn at_checkout(mut self, checkout: impl Into<String>) -> Self {
        self.checkout = Some(checkout.into());
        self
    }

    /// Add the version visible when archiving.
    #[must_use]
    pub fn at_version(mut self, version: OriginVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// The actor that archived the thread.
    #[must_use]
    pub fn actor(&self) -> &Author {
        &self.actor
    }

    /// The archive event time.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// The bound checkout identity, when supplied.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The visible version, when supplied.
    #[must_use]
    pub fn version(&self) -> Option<&OriginVersion> {
        self.version.as_ref()
    }
}

/// One immutable archival event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRecord {
    created: u64,
    actor: Author,
    checkout: Option<String>,
    version: Option<OriginVersion>,
    #[serde(default)]
    ordinal: u64,
}

impl ArchiveRecord {
    fn from_context(context: ArchiveContext, ordinal: u64) -> Self {
        Self {
            created: context.created,
            actor: context.actor,
            checkout: context.checkout,
            version: context.version,
            ordinal,
        }
    }

    /// When the archive event happened, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Who archived the thread.
    #[must_use]
    pub fn actor(&self) -> &Author {
        &self.actor
    }

    /// The bound checkout identity, when supplied.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The visible version, when supplied.
    #[must_use]
    pub fn version(&self) -> Option<&OriginVersion> {
        self.version.as_ref()
    }

    /// Append-log order for this archive event.
    #[must_use]
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }
}

/// One immutable restore event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRecord {
    created: u64,
    actor: Author,
    checkout: Option<String>,
    version: Option<OriginVersion>,
    #[serde(default)]
    ordinal: u64,
}

impl RestoreRecord {
    fn from_context(context: ArchiveContext, ordinal: u64) -> Self {
        Self {
            created: context.created,
            actor: context.actor,
            checkout: context.checkout,
            version: context.version,
            ordinal,
        }
    }

    /// When the restore event happened, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// Who restored the thread.
    #[must_use]
    pub fn actor(&self) -> &Author {
        &self.actor
    }

    /// The bound checkout identity, when supplied.
    #[must_use]
    pub fn checkout(&self) -> Option<&str> {
        self.checkout.as_deref()
    }

    /// The visible version, when supplied.
    #[must_use]
    pub fn version(&self) -> Option<&OriginVersion> {
        self.version.as_ref()
    }

    /// Append-log order for this restore event.
    #[must_use]
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }
}

/// A discussion with immutable origin, current placement, replies, and status.
///
/// Serializes as a plain object so it can travel over the session socket
/// (ADR 0014); the JSONL file stores events, not threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    id: ThreadId,
    /// Immutable source evidence captured at creation.
    origin: Origin,
    /// Mutable projection evidence for the currently selected placement.
    placement: PlacementEvidence,
    /// Current projected workspace-relative path.
    path: PathBuf,
    /// Current projected range; `None` for a file-wide thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    /// Bounded immutable source excerpt captured at creation.
    snippet: String,
    /// Current projected content anchor.
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
    /// Current commit context used for contextual placement, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    /// When the user last edited the comment (ADR 0058).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment_edited: Option<u64>,
    /// When the user last reopened the thread (ADR 0058).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reopened: Option<u64>,
    /// Current placement context window (ADR 0038).
    #[serde(skip)]
    context: Option<Context>,
    /// Every successful resolution, including resolutions before a reopen.
    #[serde(default)]
    resolution_history: Vec<ResolutionRecord>,
    /// Whether the thread is outside the normal board iteration.
    #[serde(default)]
    archived: bool,
    /// Durable archive events for history inspection.
    #[serde(default)]
    archive_history: Vec<ArchiveRecord>,
    /// Durable restore events for history inspection.
    #[serde(default)]
    restore_history: Vec<RestoreRecord>,
    /// Append-log revision used for atomic board-slate validation.
    #[serde(default)]
    revision: u64,
}

impl Thread {
    /// The thread id.
    #[must_use]
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    /// Immutable source evidence captured when this thread was created.
    #[must_use]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    /// Immutable provenance facts captured at creation.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        self.origin.provenance()
    }

    /// The source version supplying the original lines.
    #[must_use]
    pub fn origin_version(&self) -> &OriginVersion {
        self.origin.version()
    }

    /// The comparison side supplying the original lines.
    #[must_use]
    pub fn origin_side(&self) -> OriginSide {
        self.origin.side()
    }

    /// The human comparison whose lines were shown, when supplied.
    #[must_use]
    pub fn comparison(&self) -> Option<&ComparisonFacts> {
        self.origin.comparison()
    }

    /// The immutable content identity, when available.
    #[must_use]
    pub fn content_identity(&self) -> Option<&ContentIdentity> {
        self.origin.content()
    }

    /// Current placement evidence, separate from immutable origin.
    #[must_use]
    pub fn placement_evidence(&self) -> &PlacementEvidence {
        &self.placement
    }

    /// Who wrote the comment that opened the thread (ADR 0061).
    #[must_use]
    pub fn author(&self) -> &Author {
        &self.author
    }

    /// Stored workspace-relative placement path.
    ///
    /// A viewer may derive a checkout-local path without rewriting this
    /// shared value. The immutable creation path is [`Thread::origin`].
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current projected range; `None` for a file-wide thread.
    ///
    /// This placement may change after relocation or projection. The
    /// immutable creation range is [`Thread::origin`]'s range.
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

    /// Where the current placement sits: `path:lines`, or just `path`.
    #[must_use]
    pub fn place(&self) -> String {
        match self.range {
            Some(range) => format!("{}:{range}", self.path.display()),
            None => self.path.display().to_string(),
        }
    }

    /// The bounded source excerpt captured at creation, without a newline.
    ///
    /// This remains an immutable origin fact; current placement evidence is
    /// available through [`Thread::placement_evidence`].
    #[must_use]
    pub fn snippet(&self) -> &str {
        &self.snippet
    }

    /// The current content anchor; `None` for a file-wide thread.
    ///
    /// Relocation can replace this placement anchor. The immutable creation
    /// anchor is available through [`Thread::origin`].
    #[must_use]
    pub fn anchor(&self) -> Option<&Anchor> {
        self.anchor.as_ref()
    }

    /// The current placement context window (ADR 0038), or `None` for a
    /// file-wide thread.
    ///
    /// Relocation can replace this context. The immutable creation context
    /// is available through [`Thread::origin`].
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
    /// relocation, and archive history.
    #[must_use]
    pub fn modified(&self) -> u64 {
        self.modified
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

    /// The current commit context used for contextual placement, if any.
    ///
    /// Resolution can change this value. The immutable source version
    /// captured at creation is [`Thread::origin_version`].
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

    /// Every successful resolution event in append order.
    #[must_use]
    pub fn resolution_history(&self) -> &[ResolutionRecord] {
        &self.resolution_history
    }

    /// The newest resolution event, if this thread has been resolved.
    #[must_use]
    pub fn latest_resolution(&self) -> Option<&ResolutionRecord> {
        self.resolution_history.last()
    }

    /// Whether the thread is archived out of normal board iteration.
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived
    }

    /// The archive history, oldest first.
    #[must_use]
    pub fn archive_history(&self) -> &[ArchiveRecord] {
        &self.archive_history
    }

    /// The restore history, oldest first.
    #[must_use]
    pub fn restore_history(&self) -> &[RestoreRecord] {
        &self.restore_history
    }

    /// The newest archive event, if any.
    #[must_use]
    pub fn latest_archive(&self) -> Option<&ArchiveRecord> {
        self.archive_history.last()
    }

    /// The newest restore event, if any.
    #[must_use]
    pub fn latest_restore(&self) -> Option<&RestoreRecord> {
        self.restore_history.last()
    }

    /// Append-log revision of the latest event touching this thread.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
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
    provenance: Provenance,
    start_source: Option<StartSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartSource {
    Commit(CommitId),
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
            provenance: Provenance::default(),
            start_source: None,
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
            provenance: Provenance::default(),
            start_source: None,
        }
    }

    /// Record the `HEAD` commit the comment is written against
    /// (ADR 0024), so the thread shows only where that commit is
    /// reachable.
    #[must_use]
    pub fn at_commit(mut self, commit: Option<String>) -> Self {
        if let Some(commit_id) = commit.as_deref() {
            self.provenance.version = OriginVersion::commit(commit_id);
        }
        self.commit = commit;
        self
    }

    /// Select an exact immutable commit as this start's source and retry identity.
    ///
    /// This sets commit provenance with an unspecified comparison side. Later
    /// builder calls that make the provenance disagree are rejected by
    /// [`Store`] before probing or writing a keyed start.
    #[must_use]
    pub fn at_selected_commit(mut self, commit: CommitId) -> Self {
        let id = commit.as_str().to_owned();
        self.commit = Some(id.clone());
        self.provenance.version = OriginVersion::commit(id);
        self.provenance.side = OriginSide::Unspecified;
        self.provenance.comparison = None;
        self.provenance.working_tree = None;
        self.provenance.index = None;
        self.provenance.review_point = None;
        self.start_source = Some(StartSource::Commit(commit));
        self
    }

    /// Supply immutable origin facts captured at the input boundary.
    #[must_use]
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = provenance;
        self
    }

    /// Supply the source version and side for the original evidence.
    #[must_use]
    pub fn at_source(mut self, version: OriginVersion, side: OriginSide) -> Self {
        self.provenance.version = version.clone();
        if let OriginVersion::Commit { id } = version {
            self.commit = Some(id);
        }
        self.provenance.side = side;
        self
    }

    /// Supply the human comparison whose lines are being discussed.
    #[must_use]
    pub fn in_comparison(mut self, comparison: ComparisonFacts) -> Self {
        self.provenance.comparison = Some(comparison);
        self
    }

    /// The provenance facts supplied for this draft.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Record bound working-tree facts for this draft.
    #[must_use]
    pub fn with_working_tree_facts(mut self, facts: WorkingTreeFacts) -> Self {
        self.provenance.version = OriginVersion::working_tree(facts.observed_head.clone());
        self.provenance.working_tree = Some(facts);
        self
    }

    /// Record bound index facts for this draft.
    #[must_use]
    pub fn with_index_facts(mut self, facts: IndexFacts) -> Self {
        self.provenance.version = OriginVersion::index(facts.observed_head.clone());
        self.provenance.index = Some(facts);
        self
    }

    /// Record a review-point source for this draft.
    #[must_use]
    pub fn at_review_point(mut self, facts: ReviewPointFacts, side: OriginSide) -> Self {
        self.provenance.version = OriginVersion::review_point(facts.id.clone(), facts.base.clone());
        self.provenance.side = side;
        self.provenance.review_point = Some(facts);
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
    checkout: Option<String>,
    version: Option<OriginVersion>,
    placement: Option<PlacementContext>,
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
            checkout: None,
            version: None,
            placement: None,
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
        if self.version.is_none() {
            self.version = head.clone().map(OriginVersion::commit);
        }
        self.head = head;
        self
    }

    /// Record the checkout that supplied the resolution context.
    #[must_use]
    pub fn at_checkout(mut self, checkout: impl Into<String>) -> Self {
        self.checkout = Some(checkout.into());
        self
    }

    /// Record the version visible when the agent resolved the thread.
    #[must_use]
    pub fn at_version(mut self, version: OriginVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// Qualify any relocation captured with this reply.
    #[must_use]
    pub fn place_in(mut self, placement: PlacementContext) -> Self {
        self.placement = Some(placement);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<StartIntentSource<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum StartIntentSource<'a> {
    Commit { id: &'a str },
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
    version: OriginVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
}

/// One thread's acknowledged state in a clear-board confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSlateEntry {
    id: ThreadId,
    lifecycle: Lifecycle,
    modified: u64,
    resolution_ordinal: Option<u64>,
    revision: u64,
}

impl BoardSlateEntry {
    /// The acknowledged thread ID.
    #[must_use]
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    /// The lifecycle acknowledged by the user.
    #[must_use]
    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    /// The generic modification time acknowledged by the user.
    #[must_use]
    pub fn modified(&self) -> u64 {
        self.modified
    }

    /// The latest resolution event acknowledged by the user.
    #[must_use]
    pub fn resolution_ordinal(&self) -> Option<u64> {
        self.resolution_ordinal
    }

    /// The append-log revision acknowledged by the user.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// An acknowledged repository-board slate for a later atomic clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSlate {
    entries: Vec<BoardSlateEntry>,
    observed_cursor: ActivityCursor,
}

impl BoardSlate {
    /// The exact non-archived entries acknowledged by the user.
    #[must_use]
    pub fn entries(&self) -> &[BoardSlateEntry] {
        &self.entries
    }

    /// The append cursor observed while acknowledging the slate.
    #[must_use]
    pub fn observed_cursor(&self) -> ActivityCursor {
        self.observed_cursor
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ArchiveEntry {
    thread: ThreadId,
    context: ArchiveContext,
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
        origin: Box<Origin>,
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
        checkout: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<OriginVersion>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<Author>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<OriginVersion>,
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
        version: OriginVersion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<String>,
    },
    /// The user deleted the thread (ADR 0034). A tombstone: the thread
    /// is dropped on load and later events on it are ignored.
    Delete {
        v: u32,
        thread: ThreadId,
        created: u64,
    },
    /// The user changed one-shot auto-resolve permission.
    SetAutoResolve {
        v: u32,
        thread: ThreadId,
        value: AutoResolve,
        created: u64,
    },
    /// Archive one or more threads in one durable board operation.
    ArchiveMany { v: u32, entries: Vec<ArchiveEntry> },
    /// Restore one archived thread without changing its lifecycle.
    Restore {
        v: u32,
        thread: ThreadId,
        context: ArchiveContext,
    },
}

impl Event {
    /// The thread an event acts on; none for the one that creates it.
    fn thread_id(&self) -> Option<&ThreadId> {
        match self {
            Self::Annotate { .. } | Self::ArchiveMany { .. } => None,
            Self::Reply { thread, .. }
            | Self::AgentReply { thread, .. }
            | Self::Edit { thread, .. }
            | Self::Resolve { thread, .. }
            | Self::Reopen { thread, .. }
            | Self::Relocate { thread, .. }
            | Self::Delete { thread, .. }
            | Self::SetAutoResolve { thread, .. }
            | Self::Restore { thread, .. } => Some(thread),
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
            | Self::Delete { .. }
            | Self::SetAutoResolve { .. }
            | Self::ArchiveMany { .. }
            | Self::Restore { .. } => None,
        }
    }

    /// The message body this event would persist, if it has one.
    fn message_body(&self) -> Option<&str> {
        match self {
            Self::Annotate { comment, .. } => Some(comment),
            Self::Reply { reply, .. } | Self::AgentReply { reply, .. } => Some(reply.body()),
            Self::Edit { body, .. } => Some(body),
            Self::Resolve { .. }
            | Self::Reopen { .. }
            | Self::Relocate { .. }
            | Self::Delete { .. }
            | Self::SetAutoResolve { .. }
            | Self::ArchiveMany { .. }
            | Self::Restore { .. } => None,
        }
    }
}

/// The threads of one workspace, backed by an append-only JSONL file.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    /// Whether this handle has observed its backing file.
    backing_observed: bool,
    /// Non-archived repository-board threads, in creation order.
    threads: Vec<Thread>,
    /// Archived threads retained for exact-ID and history inspection.
    archived: Vec<Thread>,
    /// Threads a tombstone removed, so an event that raced the deletion
    /// (a headless reply) is skipped rather than rejected as unknown.
    deleted: HashSet<ThreadId>,
    receipts: HashMap<ReceiptKey, IdempotencyReceipt>,
    cursor: ActivityCursor,
    agent_activity: Vec<AgentActivity>,
}

impl Store {
    /// Open the workspace's XDG store, validating its private directory hierarchy.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for unsafe existing state or the errors of [`Self::open`].
    pub fn open_workspace(dirs: &crate::XdgDirs, key: &Path) -> Result<Self, StoreError> {
        let directory = dirs.workspace_dir(key);
        dirs.prepare_state_dir(&directory)
            .map_err(|error| StoreError::io(&directory, error))?;
        Self::open(dirs.threads_file(key))
    }

    /// Reload a workspace store without creating missing state directories.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for missing or unsafe application-owned directories,
    /// or the errors of [`Self::open`].
    pub fn reload_workspace(dirs: &crate::XdgDirs, key: &Path) -> Result<Self, StoreError> {
        let directory = dirs.workspace_dir(key);
        dirs.validate_state_dir(&directory)
            .map_err(|error| StoreError::io(&directory, error))?;
        Self::open(dirs.threads_file(key))
    }

    /// Load the store at `path`, or start empty when the file is missing.
    ///
    /// Existing files must be private, owned by the effective UID and not
    /// linked. Missing parents are created privately on the first write;
    /// existing caller-owned parents need not be private. Use
    /// [`Self::open_workspace`] to also validate the application-owned XDG tree.
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
            backing_observed: false,
            threads: Vec::new(),
            archived: Vec::new(),
            deleted: HashSet::new(),
            receipts: HashMap::new(),
            cursor: ActivityCursor::default(),
            agent_activity: Vec::new(),
        };
        let mut file = match crate::private_state::open_read(&store.path) {
            Ok(file) => {
                store.backing_observed = true;
                file
            }
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
            backing_observed: self.backing_observed,
            threads: Vec::new(),
            archived: Vec::new(),
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
        self.archived = loaded.archived;
        self.deleted = loaded.deleted;
        self.receipts = loaded.receipts;
        self.cursor = loaded.cursor;
        self.agent_activity = loaded.agent_activity;
        Ok(())
    }

    fn lock_for_write(&mut self) -> Result<File, StoreError> {
        let mut file = crate::private_state::open_append(&self.path)
            .map_err(|error| StoreError::io(&self.path, error))?;
        self.backing_observed = true;
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

    /// Whether this handle has observed its backing file.
    #[must_use]
    pub const fn backing_file_observed(&self) -> bool {
        self.backing_observed
    }

    /// Every non-archived board thread, oldest first.
    #[must_use]
    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    /// Every archived thread, oldest first.
    #[must_use]
    pub fn archived_threads(&self) -> &[Thread] {
        &self.archived
    }

    /// Every thread, including archived history.
    pub fn all_threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads.iter().chain(&self.archived)
    }

    /// Every non-archived thread resolved at least once, newest resolution first.
    ///
    /// Ordering uses the actual latest resolution event and an append ordinal
    /// tie-breaker, never generic thread modification metadata.
    #[must_use]
    pub fn recently_resolved(&self) -> Vec<&Thread> {
        let mut threads: Vec<&Thread> = self
            .threads
            .iter()
            .filter(|thread| thread.status() == Status::Resolved)
            .filter(|thread| thread.latest_resolution().is_some())
            .collect();
        threads.sort_by(|left, right| {
            let left_key = left
                .latest_resolution()
                .map(|record| (record.created(), record.ordinal()))
                .unwrap_or_default();
            let right_key = right
                .latest_resolution()
                .map(|record| (record.created(), record.ordinal()))
                .unwrap_or_default();
            right_key
                .cmp(&left_key)
                .then_with(|| right.id.cmp(&left.id))
        });
        threads
    }

    /// Capture the exact non-archived board state for a later clear.
    #[must_use]
    pub fn board_slate(&self) -> BoardSlate {
        BoardSlate {
            entries: self
                .threads
                .iter()
                .map(|thread| BoardSlateEntry {
                    id: thread.id.clone(),
                    lifecycle: thread.lifecycle,
                    modified: thread.modified,
                    resolution_ordinal: thread.latest_resolution().map(ResolutionRecord::ordinal),
                    revision: thread.revision,
                })
                .collect(),
            observed_cursor: self.cursor,
        }
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
        self.threads
            .iter()
            .chain(&self.archived)
            .find(|thread| thread.id == *id)
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
        validate_start_source(&draft)?;
        let (anchor, context, snippet) = capture_annotation(&draft, text)?;
        let mut file = self.lock_for_write()?;
        let id = ThreadId(format!(
            "{now}-{}-{}",
            std::process::id(),
            self.threads.len() + self.archived.len() + self.deleted.len() + 1
        ));
        let origin = Origin::new(
            &draft.path,
            draft.range,
            &snippet,
            anchor.clone(),
            context.clone(),
        )
        .with_provenance(origin_provenance(&draft, &snippet));
        let event = Event::Annotate {
            v: FORMAT_VERSION,
            id: id.clone(),
            origin: Box::new(origin),
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
            let thread = self.thread(&receipt.target).ok_or_else(|| StoreError {
                kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
            })?;
            ensure_selected_origin(thread, selected_commit(&draft), &receipt.key)?;
            let id = receipt.target.clone();
            let _ = file.unlock();
            return Ok(WriteOutcome::from_replay(id));
        }

        let text = load_text(&draft.path)?;
        let (anchor, context, snippet) = capture_annotation(&draft, &text)?;
        let id = ThreadId(format!(
            "{now}-{}-{}",
            std::process::id(),
            self.threads.len() + self.archived.len() + self.deleted.len() + 1
        ));
        let origin = Origin::new(
            &draft.path,
            draft.range,
            &snippet,
            anchor.clone(),
            context.clone(),
        )
        .with_provenance(origin_provenance(&draft, &snippet));
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
            origin: Box::new(origin),
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
                checkout: None,
                version: None,
                placement: None,
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
        if thread.is_archived() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
            });
        }
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
        if thread.is_archived() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
            });
        }
        if thread.status() != Status::Open {
            let _ = file.unlock();
            return Err(StoreError::message(format!(
                "{id} is resolved; only the user can reopen it"
            )));
        }
        let placement = command.placement.unwrap_or_else(|| {
            let context = PlacementContext::new(OriginVersion::working_tree(command.head.clone()));
            command
                .checkout
                .as_deref()
                .map_or(context.clone(), |checkout| context.at_checkout(checkout))
        });
        let relocation =
            capture_reply_relocation(thread, lines, command.reply.created(), placement, load_text)?;
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
                    .then_some(command.head.clone())
                    .flatten(),
                checkout: (outcome == ResolutionOutcome::Resolved)
                    .then_some(command.checkout)
                    .flatten(),
                version: (outcome == ResolutionOutcome::Resolved)
                    .then_some(command.version)
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
                checkout: None,
                version: None,
                placement: Some(PlacementContext::new(OriginVersion::working_tree(None))),
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
        if thread.is_archived() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
            });
        }
        let relocation = capture_reply_relocation(
            thread,
            lines,
            reply.created(),
            PlacementContext::new(OriginVersion::working_tree(None)),
            load_text,
        )?;
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
        self.probe_idempotency(&receipt_key, &start_intent(draft)?, selected_commit(draft))
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
        self.probe_idempotency(&receipt_key, &reply_intent(id, reply, lines)?, None)
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
        if thread.is_archived() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
            });
        }
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
        self.resolve_with_context(
            id,
            &ResolutionContext::new(Author::User, head.map(str::to_owned)),
            now,
        )
    }

    /// Resolve a thread while recording immutable actor and checkout context.
    ///
    /// The origin and current placement remain unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown, archived, or the
    /// event cannot be appended.
    pub fn resolve_with_context(
        &mut self,
        id: &ThreadId,
        context: &ResolutionContext,
        now: u64,
    ) -> Result<(), StoreError> {
        self.commit(Event::Resolve {
            v: FORMAT_VERSION,
            thread: id.clone(),
            created: now,
            commit: context.head.clone(),
            actor: Some(context.actor.clone()),
            checkout: context.checkout.clone(),
            version: context.version.clone(),
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
        let thread = self.thread(id).ok_or_else(|| StoreError {
            kind: ErrorKind::UnknownThread(id.clone()),
        })?;
        if thread.is_archived() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
            });
        }
        let value = thread.auto_resolve().toggled();
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

    /// Archive one thread without changing its lifecycle or origin.
    ///
    /// Archival disables pending one-shot auto-resolve permission. Repeating
    /// an archive of an already archived thread is an idempotent no-op.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the archive event
    /// cannot be persisted.
    pub fn archive(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
        self.archive_with_context(id, ArchiveContext::new(Author::User, now))
    }

    /// Archive one thread with explicit event context.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the archive event
    /// cannot be persisted.
    pub fn archive_with_context(
        &mut self,
        id: &ThreadId,
        context: ArchiveContext,
    ) -> Result<(), StoreError> {
        let mut file = self.lock_for_write()?;
        if self.thread(id).is_none() {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::UnknownThread(id.clone()),
            });
        }
        if self.archived.iter().any(|thread| thread.id == *id) {
            let _ = file.unlock();
            return Ok(());
        }
        let result = self.append_locked(
            &mut file,
            Event::ArchiveMany {
                v: FORMAT_VERSION,
                entries: vec![ArchiveEntry {
                    thread: id.clone(),
                    context,
                }],
            },
        );
        let _ = file.unlock();
        result
    }

    /// Archive every thread that is still resolved when the write lock is held.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the archive event cannot be persisted.
    pub fn archive_resolved(&mut self, now: u64) -> Result<Vec<ThreadId>, StoreError> {
        self.archive_resolved_with_context(&ArchiveContext::new(Author::User, now))
    }

    /// Archive every thread that is still resolved with explicit context.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the archive event cannot be persisted.
    pub fn archive_resolved_with_context(
        &mut self,
        context: &ArchiveContext,
    ) -> Result<Vec<ThreadId>, StoreError> {
        let mut file = self.lock_for_write()?;
        let ids: Vec<ThreadId> = self
            .threads
            .iter()
            .filter(|thread| thread.status() == Status::Resolved)
            .map(|thread| thread.id.clone())
            .collect();
        if ids.is_empty() {
            let _ = file.unlock();
            return Ok(ids);
        }
        let entries = ids
            .iter()
            .cloned()
            .map(|thread| ArchiveEntry {
                thread,
                context: (*context).clone(),
            })
            .collect();
        let result = self.append_locked(
            &mut file,
            Event::ArchiveMany {
                v: FORMAT_VERSION,
                entries,
            },
        );
        let _ = file.unlock();
        result?;
        Ok(ids)
    }

    /// Clear exactly an acknowledged board slate under one store lock.
    ///
    /// A new thread is not part of the slate and is never swept. If an
    /// acknowledged thread was changed, reopened, archived, or deleted, the
    /// operation fails without archiving any entry.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when an acknowledged entry changed or the
    /// archive event cannot be persisted.
    pub fn clear_board(
        &mut self,
        slate: &BoardSlate,
        now: u64,
    ) -> Result<Vec<ThreadId>, StoreError> {
        self.clear_board_with_context(slate, &ArchiveContext::new(Author::User, now))
    }

    /// Clear an acknowledged board slate with explicit event context.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when an acknowledged entry changed or the
    /// archive event cannot be persisted.
    pub fn clear_board_with_context(
        &mut self,
        slate: &BoardSlate,
        context: &ArchiveContext,
    ) -> Result<Vec<ThreadId>, StoreError> {
        let mut file = self.lock_for_write()?;
        for entry in &slate.entries {
            let Some(thread) = self.threads.iter().find(|thread| thread.id == entry.id) else {
                let _ = file.unlock();
                return Err(StoreError {
                    kind: ErrorKind::SlateChanged(entry.id.clone()),
                });
            };
            if thread.lifecycle != entry.lifecycle
                || thread.modified != entry.modified
                || thread.latest_resolution().map(ResolutionRecord::ordinal)
                    != entry.resolution_ordinal
                || thread.revision != entry.revision
            {
                let _ = file.unlock();
                return Err(StoreError {
                    kind: ErrorKind::SlateChanged(entry.id.clone()),
                });
            }
        }
        let ids: Vec<ThreadId> = slate.entries.iter().map(|entry| entry.id.clone()).collect();
        if ids.is_empty() {
            let _ = file.unlock();
            return Ok(ids);
        }
        let entries = ids
            .iter()
            .cloned()
            .map(|thread| ArchiveEntry {
                thread,
                context: (*context).clone(),
            })
            .collect();
        let result = self.append_locked(
            &mut file,
            Event::ArchiveMany {
                v: FORMAT_VERSION,
                entries,
            },
        );
        let _ = file.unlock();
        result?;
        Ok(ids)
    }

    /// Restore an archived thread while retaining its lifecycle and history.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the restore event
    /// cannot be persisted.
    pub fn restore(&mut self, id: &ThreadId, now: u64) -> Result<(), StoreError> {
        self.restore_with_context(id, ArchiveContext::new(Author::User, now))
    }

    /// Restore an archived thread with explicit event context.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the thread is unknown or the restore event
    /// cannot be persisted.
    pub fn restore_with_context(
        &mut self,
        id: &ThreadId,
        context: ArchiveContext,
    ) -> Result<(), StoreError> {
        let mut file = self.lock_for_write()?;
        if self.archived.iter().all(|thread| thread.id != *id) {
            if self.thread(id).is_none() {
                let _ = file.unlock();
                return Err(StoreError {
                    kind: ErrorKind::UnknownThread(id.clone()),
                });
            }
            let _ = file.unlock();
            return Ok(());
        }
        let result = self.append_locked(
            &mut file,
            Event::Restore {
                v: FORMAT_VERSION,
                thread: id.clone(),
                context,
            },
        );
        let _ = file.unlock();
        result
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
        placement: PlacementContext,
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
            version: placement.version,
            checkout: placement.checkout,
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
        if let Some(id) = event.thread_id()
            && self.archived.iter().any(|thread| thread.id == *id)
        {
            let _ = file.unlock();
            return Err(StoreError {
                kind: ErrorKind::Archived(id.clone()),
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
        selected: Option<&CommitId>,
    ) -> Result<Option<ThreadId>, StoreError> {
        let store = Self::open(&self.path)?;
        let Some(receipt) = store.receipts.get(receipt_key) else {
            return Ok(None);
        };
        ensure_intent(receipt, intent)?;
        let thread = store.thread(&receipt.target).ok_or_else(|| StoreError {
            kind: ErrorKind::IdempotencyDeleted(receipt.target.clone()),
        })?;
        ensure_selected_origin(thread, selected, &receipt.key)?;
        Ok(Some(receipt.target.clone()))
    }

    fn append_locked(&mut self, file: &mut File, event: Event) -> Result<(), StoreError> {
        if let Some(body) = event.message_body()
            && body.len() > MAX_MESSAGE_BYTES
        {
            return Err(StoreError::message(format!(
                "thread message has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
                body.len()
            )));
        }
        let mut line = serde_json::to_string(&event).map_err(|error| StoreError {
            kind: ErrorKind::Parse(0, error.to_string()),
        })?;
        let before = (
            self.threads.clone(),
            self.archived.clone(),
            self.deleted.clone(),
            self.receipts.clone(),
            self.cursor,
            self.agent_activity.clone(),
        );
        let cursor = ActivityCursor(self.cursor.0 + 1);
        if let Err(error) = self.apply(event, cursor) {
            (
                self.threads,
                self.archived,
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
                self.archived,
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
        let touched = event.thread_id().cloned();
        match event {
            Event::Annotate {
                id,
                origin,
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
                let origin = *origin;
                if range.is_some() != anchor.is_some() || range.is_some() != context.is_some() {
                    return Err(StoreError {
                        kind: ErrorKind::AnnotationShape,
                    });
                }
                if origin.path() != path
                    || origin.range() != range
                    || origin.snippet() != snippet
                    || origin.anchor() != anchor.as_ref()
                    || origin.context() != context.as_ref()
                {
                    return Err(StoreError::message(
                        "annotation origin and placement evidence disagree",
                    ));
                }
                let placement_version = commit
                    .as_deref()
                    .map_or_else(|| origin.version().clone(), OriginVersion::commit);
                let placement = PlacementEvidence::from_origin(&origin, placement_version);
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
                    origin,
                    placement,
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
                    resolution_history: Vec::new(),
                    archived: false,
                    archive_history: Vec::new(),
                    restore_history: Vec::new(),
                    revision: cursor.ordinal(),
                });
                order_threads_by_creation(&mut self.threads);
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
                        thread.context = Some(relocation.context.clone());
                        thread.placement.range = thread.range;
                        thread.placement.anchor = thread.anchor.clone();
                        thread.placement.context = thread.context.clone();
                        thread.placement.version = relocation.version;
                        thread.placement.checkout = relocation.checkout;
                        thread.placement.observed_at = Some(relocation.created);
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
                checkout,
                version,
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
                        thread.context = Some(relocation.context.clone());
                        thread.placement.range = thread.range;
                        thread.placement.anchor = thread.anchor.clone();
                        thread.placement.context = thread.context.clone();
                        thread.placement.version = relocation.version;
                        thread.placement.checkout = relocation.checkout;
                        thread.placement.observed_at = Some(relocation.created);
                    }
                    thread.modified = thread.modified.max(reply.created);
                    thread.auto_resolve = AutoResolve::Disabled;
                    thread.lifecycle = match resolution {
                        ResolutionOutcome::NotRequested => Lifecycle::Active,
                        ResolutionOutcome::ResolutionProposed => Lifecycle::ResolutionProposed,
                        ResolutionOutcome::Resolved => Lifecycle::Resolved,
                    };
                    if resolution == ResolutionOutcome::Resolved
                        && let Some(commit_id) = commit.clone()
                    {
                        thread.commit = Some(commit_id);
                    }
                    if resolution == ResolutionOutcome::Resolved {
                        thread
                            .resolution_history
                            .push(ResolutionRecord::from_context(
                                ResolutionContext {
                                    actor: reply.author.clone(),
                                    checkout,
                                    version,
                                    head: commit,
                                },
                                reply.created,
                                cursor.ordinal(),
                            ));
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
                actor,
                checkout,
                version,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.modified = thread.modified.max(created);
                thread.lifecycle = Lifecycle::Resolved;
                thread.auto_resolve = AutoResolve::Disabled;
                if let Some(commit_id) = commit.clone() {
                    thread.commit = Some(commit_id);
                }
                thread
                    .resolution_history
                    .push(ResolutionRecord::from_context(
                        ResolutionContext {
                            actor: actor.unwrap_or(Author::User),
                            checkout,
                            version,
                            head: commit,
                        },
                        created,
                        cursor.ordinal(),
                    ));
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
                version,
                checkout,
                ..
            } => {
                let thread = self.thread_mut(&thread)?;
                thread.modified = thread.modified.max(created);
                thread.range = Some(range);
                thread.anchor = Some(anchor);
                thread.reanchored_at = Some(created);
                thread.context = Some(context.clone());
                thread.placement.range = thread.range;
                thread.placement.anchor = thread.anchor.clone();
                thread.placement.context = thread.context.clone();
                thread.placement.version = version;
                thread.placement.checkout = checkout;
                thread.placement.observed_at = Some(created);
            }
            Event::Delete { thread, .. } => {
                self.thread_mut(&thread)?;
                self.threads.retain(|other| other.id != thread);
                self.archived.retain(|other| other.id != thread);
                self.deleted.insert(thread);
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
            Event::ArchiveMany { entries, .. } => {
                let mut positions = Vec::with_capacity(entries.len());
                for entry in &entries {
                    let Some((position, thread)) = self
                        .threads
                        .iter()
                        .enumerate()
                        .find(|(_, thread)| thread.id == entry.thread)
                    else {
                        return Err(StoreError {
                            kind: ErrorKind::SlateChanged(entry.thread.clone()),
                        });
                    };
                    if thread.is_archived() || positions.iter().any(|(index, _)| *index == position)
                    {
                        return Err(StoreError {
                            kind: ErrorKind::SlateChanged(entry.thread.clone()),
                        });
                    }
                    positions.push((position, entry));
                }
                positions.sort_by_key(|(position, _)| *position);
                let mut moved = Vec::with_capacity(positions.len());
                for (_, entry) in positions.into_iter().rev() {
                    let position = self
                        .threads
                        .iter()
                        .position(|thread| thread.id == entry.thread)
                        .ok_or_else(|| StoreError {
                            kind: ErrorKind::SlateChanged(entry.thread.clone()),
                        })?;
                    let mut thread = self.threads.remove(position);
                    thread.archived = true;
                    thread.auto_resolve = AutoResolve::Disabled;
                    thread.modified = thread.modified.max(entry.context.created);
                    thread.archive_history.push(ArchiveRecord::from_context(
                        entry.context.clone(),
                        cursor.ordinal(),
                    ));
                    thread.revision = cursor.ordinal();
                    moved.push(thread);
                }
                moved.reverse();
                self.archived.extend(moved);
                order_threads_by_creation(&mut self.archived);
            }
            Event::Restore {
                thread: id,
                context,
                ..
            } => {
                let position = self
                    .archived
                    .iter()
                    .position(|thread| thread.id == id)
                    .ok_or_else(|| StoreError {
                        kind: ErrorKind::UnknownThread(id.clone()),
                    })?;
                let mut thread = self.archived.remove(position);
                thread.archived = false;
                thread.auto_resolve = AutoResolve::Disabled;
                thread.modified = thread.modified.max(context.created);
                thread
                    .restore_history
                    .push(RestoreRecord::from_context(context, cursor.ordinal()));
                thread.revision = cursor.ordinal();
                self.threads.push(thread);
                order_threads_by_creation(&mut self.threads);
            }
        }
        if let Some(id) = touched
            && let Ok(thread) = self.thread_mut(&id)
        {
            thread.revision = cursor.ordinal();
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
            .chain(self.archived.iter_mut())
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
    validate_start_source(draft)?;
    let intent = StartIntent {
        operation: IdempotentOperation::Start,
        path: normalize_repo_path(&draft.path)?,
        range: draft.range.map(normalize_range),
        body: &draft.comment,
        source: draft.start_source.as_ref().map(|source| match source {
            StartSource::Commit(id) => StartIntentSource::Commit { id: id.as_str() },
        }),
    };
    serde_json::to_string(&intent).map_err(|error| StoreError {
        kind: ErrorKind::Parse(0, error.to_string()),
    })
}

fn validate_start_source(draft: &Draft) -> Result<(), StoreError> {
    let Some(StartSource::Commit(selected)) = &draft.start_source else {
        return Ok(());
    };
    let agrees = draft.commit.as_deref() == Some(selected.as_str())
        && draft.provenance.version.commit_id() == Some(selected.as_str())
        && draft.provenance.side == OriginSide::Unspecified
        && draft.provenance.comparison.is_none()
        && draft.provenance.working_tree.is_none()
        && draft.provenance.index.is_none()
        && draft.provenance.review_point.is_none();
    if agrees {
        Ok(())
    } else {
        Err(StoreError::message(format!(
            "selected commit {selected} does not agree with the draft origin"
        )))
    }
}

fn selected_commit(draft: &Draft) -> Option<&CommitId> {
    match draft.start_source.as_ref() {
        Some(StartSource::Commit(id)) => Some(id),
        None => None,
    }
}

fn ensure_selected_origin(
    thread: &Thread,
    selected: Option<&CommitId>,
    key: &str,
) -> Result<(), StoreError> {
    let Some(selected) = selected else {
        return Ok(());
    };
    if thread.origin_version().commit_id() == Some(selected.as_str())
        && thread.origin_side() == OriginSide::Unspecified
    {
        Ok(())
    } else {
        Err(StoreError {
            kind: ErrorKind::IdempotencyConflict(key.to_owned()),
        })
    }
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
    placement: PlacementContext,
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
        version: placement.version,
        checkout: placement.checkout,
    }))
}

fn order_threads_by_creation(threads: &mut [Thread]) {
    threads.sort_by(|left, right| {
        left.created
            .cmp(&right.created)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn origin_provenance(draft: &Draft, snippet: &str) -> Provenance {
    let mut provenance = draft.provenance.clone();
    if matches!(provenance.version, OriginVersion::Unknown) {
        provenance.version = draft.commit.as_deref().map_or_else(
            || OriginVersion::working_tree(draft.commit.clone()),
            OriginVersion::commit,
        );
    }
    if provenance.content.is_none() {
        provenance.content = Some(ContentIdentity::from_text(snippet));
    }
    provenance
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
            let (snippet, _) = bound_evidence(&snippet);
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
    Archived(ThreadId),
    SlateChanged(ThreadId),
    Message(String),
}

/// Incompatible on-disk and current thread-store format versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatMismatch {
    found: u32,
    expected: u32,
}

impl FormatMismatch {
    /// Returns the version found in the thread store.
    #[must_use]
    pub const fn found(&self) -> u32 {
        self.found
    }

    /// Returns the version required by this build.
    #[must_use]
    pub const fn expected(&self) -> u32 {
        self.expected
    }
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

    /// Returns the incompatible versions when a format mismatch caused the error.
    #[must_use]
    pub fn format_mismatch(&self) -> Option<FormatMismatch> {
        match &self.kind {
            ErrorKind::Version(_, _, found) => Some(FormatMismatch {
                found: *found,
                expected: FORMAT_VERSION,
            }),
            _ => None,
        }
    }

    /// Whether the cause was an I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(..))
    }

    /// Whether a mutation was refused because its target is archived.
    #[must_use]
    pub fn is_archived(&self) -> bool {
        matches!(self.kind, ErrorKind::Archived(_))
    }

    /// Whether an acknowledged clear-board slate is stale.
    #[must_use]
    pub fn is_slate_changed(&self) -> bool {
        matches!(self.kind, ErrorKind::SlateChanged(_))
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
            ErrorKind::Archived(id) => {
                write!(f, "thread {id} is archived; restore it before writing")
            }
            ErrorKind::SlateChanged(id) => write!(
                f,
                "board slate changed for thread {id}; review the board again"
            ),
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
